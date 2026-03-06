use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::mpsc;
use tracing::{debug, info};

use crate::types::*;

const DEFAULT_OLLAMA_URL: &str = "http://localhost:11434";
const DEFAULT_OLLAMA_MODEL: &str = "qwen3:4b";

/// Ollama provider for local models using raw HTTP NDJSON streaming.
pub struct OllamaProvider {
    client: Client,
    base_url: String,
    model: String,
}

impl OllamaProvider {
    pub fn new(base_url: String, model: String) -> Self {
        let base_url = if base_url.is_empty() { DEFAULT_OLLAMA_URL.to_string() } else { base_url };
        let model = if model.is_empty() { DEFAULT_OLLAMA_MODEL.to_string() } else { model };

        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(300)) // 5 min for local inference
                .build()
                .unwrap_or_default(),
            base_url,
            model,
        }
    }

    /// Build Ollama API messages from our generic format.
    fn build_messages(&self, req: &ChatRequest) -> Vec<OllamaMessage> {
        let mut messages = Vec::new();

        // System message
        if !req.system.is_empty() {
            messages.push(OllamaMessage {
                role: "system".to_string(),
                content: req.system.clone(),
                tool_calls: None,
            });
        }

        // Collect tool call IDs that have responses
        let mut responded_tool_ids = std::collections::HashSet::new();
        for msg in &req.messages {
            if msg.role == "tool"
                && let Some(ref tr_val) = msg.tool_results
                    && let Ok(results) = serde_json::from_value::<Vec<SessionToolResult>>(tr_val.clone()) {
                        for r in &results {
                            responded_tool_ids.insert(r.tool_call_id.clone());
                        }
                    }
        }

        for msg in &req.messages {
            match msg.role.as_str() {
                "user" => {
                    messages.push(OllamaMessage {
                        role: "user".to_string(),
                        content: msg.content.clone(),
                        tool_calls: None,
                    });
                }
                "assistant" => {
                    let mut ollama_tool_calls = Vec::new();

                    if let Some(ref tc_val) = msg.tool_calls
                        && let Ok(tcs) = serde_json::from_value::<Vec<SessionToolCall>>(tc_val.clone()) {
                            for tc in tcs {
                                if !responded_tool_ids.contains(&tc.id) {
                                    continue;
                                }
                                let args: HashMap<String, serde_json::Value> =
                                    serde_json::from_value(tc.input.clone()).unwrap_or_default();
                                ollama_tool_calls.push(OllamaToolCallOut {
                                    function: OllamaFunctionCallOut {
                                        name: tc.name,
                                        arguments: args,
                                    },
                                });
                            }
                        }

                    if !msg.content.is_empty() || !ollama_tool_calls.is_empty() {
                        messages.push(OllamaMessage {
                            role: "assistant".to_string(),
                            content: msg.content.clone(),
                            tool_calls: if ollama_tool_calls.is_empty() { None } else { Some(ollama_tool_calls) },
                        });
                    }
                }
                "system" => {
                    messages.push(OllamaMessage {
                        role: "system".to_string(),
                        content: msg.content.clone(),
                        tool_calls: None,
                    });
                }
                "tool" => {
                    if let Some(ref tr_val) = msg.tool_results
                        && let Ok(results) = serde_json::from_value::<Vec<SessionToolResult>>(tr_val.clone()) {
                            for r in results {
                                messages.push(OllamaMessage {
                                    role: "tool".to_string(),
                                    content: r.content,
                                    tool_calls: None,
                                });
                            }
                        }
                }
                _ => {}
            }
        }

        messages
    }

    /// Handle the NDJSON stream from Ollama.
    async fn handle_stream(
        response: reqwest::Response,
        tx: mpsc::Sender<StreamEvent>,
    ) {
        let mut tool_call_counter = 0;
        let mut byte_stream = response.bytes_stream();
        let mut line_buf = String::new();

        while let Some(chunk_result) = byte_stream.next().await {
            let chunk = match chunk_result {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(StreamEvent::error(format!("stream read error: {e}"))).await;
                    return;
                }
            };

            let text = match std::str::from_utf8(&chunk) {
                Ok(t) => t,
                Err(e) => {
                    let _ = tx.send(StreamEvent::error(format!("invalid utf8: {e}"))).await;
                    return;
                }
            };

            line_buf.push_str(text);

            while let Some(newline_pos) = line_buf.find('\n') {
                let line = line_buf[..newline_pos].to_string();
                line_buf = line_buf[newline_pos + 1..].to_string();

                if line.trim().is_empty() {
                    continue;
                }

                let resp: OllamaStreamResponse = match serde_json::from_str(&line) {
                    Ok(r) => r,
                    Err(e) => {
                        debug!("failed to parse Ollama response: {e}, line: {line}");
                        continue;
                    }
                };

                // Stream text content
                if !resp.message.content.is_empty() {
                    let _ = tx.send(StreamEvent::text(&resp.message.content)).await;
                }

                // Handle tool calls
                if let Some(ref tool_calls) = resp.message.tool_calls {
                    for tc in tool_calls {
                        tool_call_counter += 1;
                        let input = serde_json::to_value(&tc.function.arguments)
                            .unwrap_or(serde_json::Value::Object(Default::default()));
                        let _ = tx.send(StreamEvent::tool_call(ToolCall {
                            id: format!("ollama-call-{}", tool_call_counter),
                            name: tc.function.name.clone(),
                            input,
                        })).await;
                    }
                }

                // Check if done
                if resp.done {
                    let _ = tx.send(StreamEvent::done()).await;
                    return;
                }
            }
        }

        let _ = tx.send(StreamEvent::done()).await;
    }
}

#[async_trait]
impl Provider for OllamaProvider {
    fn id(&self) -> &str {
        "ollama"
    }

    async fn stream(
        &self,
        req: &ChatRequest,
    ) -> Result<EventReceiver, ProviderError> {
        let messages = self.build_messages(req);

        let model = if req.model.is_empty() { &self.model } else { &req.model };

        // Build options
        let mut options: HashMap<String, serde_json::Value> = HashMap::new();
        if req.temperature > 0.0 {
            options.insert("temperature".to_string(), serde_json::json!(req.temperature));
        }
        if req.max_tokens > 0 {
            options.insert("num_predict".to_string(), serde_json::json!(req.max_tokens));
        }

        // Build tools
        let tools: Option<Vec<OllamaTool>> = if req.tools.is_empty() {
            None
        } else {
            Some(req.tools.iter().filter_map(|t| {
                let schema: HashMap<String, serde_json::Value> =
                    serde_json::from_value(t.input_schema.clone()).ok()?;
                Some(OllamaTool {
                    tool_type: "function".to_string(),
                    function: OllamaFunctionDef {
                        name: t.name.clone(),
                        description: t.description.clone(),
                        parameters: OllamaFunctionParams {
                            param_type: "object".to_string(),
                            properties: schema.get("properties").cloned(),
                            required: schema.get("required").and_then(|v| {
                                v.as_array().map(|arr| {
                                    arr.iter().filter_map(|v| v.as_str().map(String::from)).collect()
                                })
                            }),
                        },
                    },
                })
            }).collect())
        };

        let api_req = OllamaApiRequest {
            model: model.to_string(),
            messages,
            stream: true,
            options: if options.is_empty() { None } else { Some(options) },
            tools,
        };

        info!(model = model, messages = api_req.messages.len(), tools = req.tools.len(), "sending Ollama request");

        let response = self.client
            .post(format!("{}/api/chat", self.base_url))
            .header("content-type", "application/json")
            .json(&api_req)
            .send()
            .await
            .map_err(|e| ProviderError::Request(format!("Ollama unavailable: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::Api {
                code: status.as_u16().to_string(),
                message: body,
                retryable: status.as_u16() >= 500,
            });
        }

        let (tx, rx) = mpsc::channel(100);
        tokio::spawn(Self::handle_stream(response, tx));

        Ok(rx)
    }
}

/// Check if Ollama is running at the given base URL.
pub async fn check_ollama_available(base_url: &str) -> bool {
    let base_url = if base_url.is_empty() { DEFAULT_OLLAMA_URL } else { base_url };
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .unwrap_or_default();

    client.get(format!("{base_url}/api/tags"))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

/// List available models from Ollama.
pub async fn list_ollama_models(base_url: &str) -> Result<Vec<String>, ProviderError> {
    let base_url = if base_url.is_empty() { DEFAULT_OLLAMA_URL } else { base_url };
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    let resp = client.get(format!("{base_url}/api/tags"))
        .send()
        .await
        .map_err(|e| ProviderError::Request(e.to_string()))?;

    if !resp.status().is_success() {
        return Err(ProviderError::Request("failed to list models".to_string()));
    }

    let body: OllamaTagsResponse = resp.json().await
        .map_err(|e| ProviderError::Request(e.to_string()))?;

    Ok(body.models.into_iter().map(|m| m.name).collect())
}

/// Ensure a model is available locally, pulling it if needed.
pub async fn ensure_ollama_model(base_url: &str, model: &str) -> Result<(), ProviderError> {
    if model.is_empty() {
        return Ok(());
    }

    let models = list_ollama_models(base_url).await?;
    for m in &models {
        if m == model || m.starts_with(&format!("{model}:")) || m.trim_end_matches(":latest") == model {
            return Ok(());
        }
    }

    info!(model = model, "model not found locally, pulling");

    let base_url = if base_url.is_empty() { DEFAULT_OLLAMA_URL } else { base_url };
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(1800)) // 30 min for large models
        .build()
        .unwrap_or_default();

    let resp = client.post(format!("{base_url}/api/pull"))
        .json(&serde_json::json!({ "name": model, "stream": false }))
        .send()
        .await
        .map_err(|e| ProviderError::Request(format!("failed to pull {model}: {e}")))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(ProviderError::Request(format!("failed to pull {model}: {body}")));
    }

    info!(model = model, "model ready");
    Ok(())
}

// --- Ollama API types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionToolCall {
    id: String,
    name: String,
    input: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionToolResult {
    tool_call_id: String,
    content: String,
    #[serde(default)]
    is_error: bool,
}

#[derive(Debug, Serialize)]
struct OllamaApiRequest {
    model: String,
    messages: Vec<OllamaMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<HashMap<String, serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OllamaTool>>,
}

#[derive(Debug, Serialize)]
struct OllamaMessage {
    role: String,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OllamaToolCallOut>>,
}

#[derive(Debug, Serialize)]
struct OllamaToolCallOut {
    function: OllamaFunctionCallOut,
}

#[derive(Debug, Serialize)]
struct OllamaFunctionCallOut {
    name: String,
    arguments: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct OllamaTool {
    #[serde(rename = "type")]
    tool_type: String,
    function: OllamaFunctionDef,
}

#[derive(Debug, Serialize)]
struct OllamaFunctionDef {
    name: String,
    description: String,
    parameters: OllamaFunctionParams,
}

#[derive(Debug, Serialize)]
struct OllamaFunctionParams {
    #[serde(rename = "type")]
    param_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    properties: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    required: Option<Vec<String>>,
}

// --- Streaming response ---

#[derive(Debug, Deserialize)]
struct OllamaStreamResponse {
    #[serde(default)]
    message: OllamaResponseMessage,
    #[serde(default)]
    done: bool,
}

#[derive(Debug, Default, Deserialize)]
struct OllamaResponseMessage {
    #[serde(default)]
    content: String,
    #[serde(default)]
    tool_calls: Option<Vec<OllamaToolCallIn>>,
}

#[derive(Debug, Deserialize)]
struct OllamaToolCallIn {
    function: OllamaFunctionCallIn,
}

#[derive(Debug, Deserialize)]
struct OllamaFunctionCallIn {
    name: String,
    #[serde(default)]
    arguments: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct OllamaTagsResponse {
    #[serde(default)]
    models: Vec<OllamaModelInfo>,
}

#[derive(Debug, Deserialize)]
struct OllamaModelInfo {
    name: String,
}
