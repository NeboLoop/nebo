//! Local inference via llama.cpp FFI.
//!
//! Only compiled when the `local-inference` feature is enabled.
//! Requires the `llama-cpp-2` crate, which needs llama.cpp C library at build time.
//!
//! This module provides `stream_local()` which:
//! 1. Loads a GGUF model file
//! 2. Tokenizes the prompt
//! 3. Runs inference on a blocking thread
//! 4. Streams tokens via mpsc channel
//! 5. Extracts tool calls from the output

use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::types::*;
use super::local::LocalProvider;

impl LocalProvider {
    /// Stream inference from a local GGUF model via llama.cpp.
    pub(crate) async fn stream_local(
        &self,
        req: &ChatRequest,
    ) -> Result<EventReceiver, ProviderError> {
        use llama_cpp_2::context::params::LlamaContextParams;
        use llama_cpp_2::llama_backend::LlamaBackend;
        use llama_cpp_2::llama_batch::LlamaBatch;
        use llama_cpp_2::model::params::LlamaModelParams;
        use llama_cpp_2::model::LlamaModel;
        use llama_cpp_2::token::data_array::LlamaTokenDataArray;

        let (tx, rx) = mpsc::channel::<StreamEvent>(64);

        // Build the prompt
        let system = if !req.tools.is_empty() {
            self.build_system_with_tools(&req.system, &req.tools)
        } else {
            req.system.clone()
        };

        let mut prompt = String::new();
        if !system.is_empty() {
            prompt.push_str(&format!("<|system|>\n{}\n<|end|>\n", system));
        }
        for msg in &req.messages {
            prompt.push_str(&format!("<|{}|>\n{}\n<|end|>\n", msg.role, msg.content));
        }
        prompt.push_str("<|assistant|>\n");

        let model_path = self.model_path.clone();
        let max_tokens = if req.max_tokens > 0 {
            req.max_tokens as usize
        } else {
            2048
        };
        let temperature = if req.temperature > 0.0 {
            req.temperature as f32
        } else {
            0.7
        };
        let tools = req.tools.clone();

        // Acquire mutex to ensure single-threaded inference
        let _guard = self
            .mu
            .lock()
            .map_err(|_| ProviderError::Request("inference lock poisoned".into()))?;

        // Run inference on a blocking thread
        let tx_clone = tx.clone();
        tokio::task::spawn_blocking(move || {
            let backend = match LlamaBackend::init() {
                Ok(b) => b,
                Err(e) => {
                    let _ = tx_clone.blocking_send(StreamEvent::error(format!(
                        "failed to init llama backend: {}",
                        e
                    )));
                    let _ = tx_clone.blocking_send(StreamEvent::done());
                    return;
                }
            };

            let model_params = LlamaModelParams::default();
            let model = match LlamaModel::load_from_file(&backend, &model_path, &model_params) {
                Ok(m) => m,
                Err(e) => {
                    let _ = tx_clone.blocking_send(StreamEvent::error(format!(
                        "failed to load model {}: {}",
                        model_path, e
                    )));
                    let _ = tx_clone.blocking_send(StreamEvent::done());
                    return;
                }
            };

            info!(model = %model_path, "loaded GGUF model for local inference");

            let ctx_params = LlamaContextParams::default().with_n_ctx(
                std::num::NonZeroU32::new(4096).unwrap(),
            );
            let mut ctx = match model.new_context(&backend, ctx_params) {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx_clone.blocking_send(StreamEvent::error(format!(
                        "failed to create context: {}",
                        e
                    )));
                    let _ = tx_clone.blocking_send(StreamEvent::done());
                    return;
                }
            };

            // Tokenize the prompt
            let tokens = match model.str_to_token(&prompt, llama_cpp_2::model::AddBos::Always) {
                Ok(t) => t,
                Err(e) => {
                    let _ = tx_clone.blocking_send(StreamEvent::error(format!(
                        "tokenization failed: {}",
                        e
                    )));
                    let _ = tx_clone.blocking_send(StreamEvent::done());
                    return;
                }
            };

            debug!(
                prompt_tokens = tokens.len(),
                max_tokens,
                "starting local inference"
            );

            // Feed prompt tokens
            let mut batch = LlamaBatch::new(tokens.len().max(1), 1);
            for (i, &token) in tokens.iter().enumerate() {
                let is_last = i == tokens.len() - 1;
                batch.add(token, i as i32, &[0], is_last).unwrap_or(());
            }

            if ctx.decode(&mut batch).is_err() {
                let _ = tx_clone.blocking_send(StreamEvent::error(
                    "failed to process prompt".to_string(),
                ));
                let _ = tx_clone.blocking_send(StreamEvent::done());
                return;
            }

            // Generate tokens
            let mut full_response = String::new();
            let mut n_cur = tokens.len();

            for _ in 0..max_tokens {
                // Sample next token
                let candidates = ctx.candidates_ith(batch.n_tokens() - 1);
                let mut candidates_array = LlamaTokenDataArray::from_iter(
                    candidates.iter().cloned(),
                    false,
                );

                // Apply temperature
                candidates_array.sample_temp(temperature);
                candidates_array.sample_softmax();

                let new_token = candidates_array.data[0].id();

                // Check for EOS
                if model.is_eog_token(new_token) {
                    break;
                }

                // Decode token to text
                let token_str = model.token_to_str(new_token, llama_cpp_2::model::Special::Tokenize)
                    .unwrap_or_default();

                full_response.push_str(&token_str);

                // Stream the token
                if tx_clone.blocking_send(StreamEvent::text(&token_str)).is_err() {
                    break; // Receiver dropped
                }

                // Prepare next batch
                batch.clear();
                batch.add(new_token, n_cur as i32, &[0], true).unwrap_or(());
                n_cur += 1;

                if ctx.decode(&mut batch).is_err() {
                    warn!("decode error during generation");
                    break;
                }
            }

            // Extract tool calls from the full response
            let local = LocalProvider::new(&model_path, "");
            let tool_calls = local.extract_tool_calls(&full_response, &tools);

            for tc in tool_calls {
                let _ = tx_clone.blocking_send(StreamEvent::tool_call(tc));
            }

            // Send usage info
            let _ = tx_clone.blocking_send(StreamEvent {
                event_type: StreamEventType::Usage,
                text: String::new(),
                tool_call: None,
                error: None,
                usage: Some(UsageInfo {
                    input_tokens: tokens.len() as i32,
                    output_tokens: n_cur.saturating_sub(tokens.len()) as i32,
                    ..Default::default()
                }),
                rate_limit: None,
                widgets: None,
                provider_metadata: None,
                stop_reason: None,
            });

            let _ = tx_clone.blocking_send(StreamEvent::done());
        });

        Ok(rx)
    }
}
