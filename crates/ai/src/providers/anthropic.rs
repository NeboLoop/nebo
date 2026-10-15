use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{debug, info};

use crate::sse::{self, SseEvent};
use crate::types::*;

/// Anthropic Claude provider using raw HTTP SSE streaming.
pub struct AnthropicProvider {
    client: Client,
    api_key: String,
    model: String,
    base_url: String,
}

/// The thinking budget a pack level's provider effort asks for; 10,000
/// tokens (today's) when none is set.
fn thinking_budget(effort: Option<types::packs::ProviderEffort>) -> i32 {
    use types::packs::ProviderEffort::*;
    match effort {
        Some(Low) => 4_000,
        Some(Medium) | None => 10_000,
        Some(High) => 24_000,
    }
}

impl AnthropicProvider {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            client: crate::http::streaming_client(),
            api_key,
            model,
            base_url: "https://api.anthropic.com".to_string(),
        }
    }

    /// The request as the API receives it, cache markers placed.
    ///
    /// A deferred tool a result loaded (`find_tools`, or the error for a call
    /// made without its definition) is sent with `defer_loading`, and that
    /// result names it with a `tool_reference` block where its text showed
    /// the definition: the API leaves deferred tools out of the cached
    /// prefix and shows the definition where the reference is, so a load
    /// appends to the conversation and the cache before it stays whole.
    /// Without this a load grew the tools array, the head of the prefix, and
    /// the whole conversation was written to the cache again.
    fn api_request(&self, req: &ChatRequest) -> AnthropicApiRequest {
        let model = if req.model.is_empty() { self.model.clone() } else { req.model.clone() };
        let referable: std::collections::HashSet<&str> = if reads_tool_references(&model) {
            req.tools.iter().map(|t| t.name.as_str()).collect()
        } else {
            std::collections::HashSet::new()
        };
        let (mut messages, system_prompt, referenced) = self.build_messages(req, &referable);
        // At least one tool is loaded up front, or the API refuses the
        // request; a reference to a tool that isn't deferred still resolves.
        let deferred = if referenced.len() < req.tools.len() { referenced } else { Default::default() };

        let budget = thinking_budget(req.effort);
        let max_tokens = if req.max_tokens > 0 {
            req.max_tokens
        } else if req.enable_thinking {
            16384
        } else {
            8192
        };
        // The answer needs room beyond the thinking budget.
        let max_tokens = if req.enable_thinking { max_tokens.max(budget + 4096) } else { max_tokens };

        // Build system blocks with caching.
        //
        // When `cache_breakpoints` are provided (byte offsets into `system_prompt`),
        // we split the system prompt at those offsets and mark each prefix block
        // with `cache_control: { type: "ephemeral" }` so that the stable prefix
        // can be served from Anthropic's prompt cache at ~90% discount.
        //
        // Without breakpoints the whole prompt is sent as a single cached block.
        let system_blocks = if !system_prompt.is_empty() {
            if !req.cache_breakpoints.is_empty() {
                let mut blocks = Vec::new();
                let mut cursor = 0usize;
                let prompt_len = system_prompt.len();

                for &bp in &req.cache_breakpoints {
                    // Clamp to prompt length and skip invalid/duplicate offsets
                    let bp = bp.min(prompt_len);
                    if bp <= cursor {
                        continue;
                    }
                    blocks.push(SystemBlock {
                        text: system_prompt[cursor..bp].to_string(),
                        block_type: "text".to_string(),
                        cache_control: Some(CacheControl {
                            cache_type: "ephemeral".to_string(),
                        }),
                    });
                    cursor = bp;
                }

                // Remaining tail (dynamic portion) — no cache_control
                if cursor < prompt_len {
                    blocks.push(SystemBlock {
                        text: system_prompt[cursor..].to_string(),
                        block_type: "text".to_string(),
                        cache_control: None,
                    });
                }

                // Guard: if somehow we produced nothing, fall back to single block
                if blocks.is_empty() {
                    Some(vec![SystemBlock {
                        text: system_prompt,
                        block_type: "text".to_string(),
                        cache_control: Some(CacheControl {
                            cache_type: "ephemeral".to_string(),
                        }),
                    }])
                } else {
                    Some(blocks)
                }
            } else {
                Some(vec![SystemBlock {
                    text: system_prompt,
                    block_type: "text".to_string(),
                    cache_control: Some(CacheControl {
                        cache_type: "ephemeral".to_string(),
                    }),
                }])
            }
        } else {
            None
        };

        // Tools a result loaded go deferred. The last tool loaded up front
        // carries a cache marker, so a tool appended after it (one a request's
        // words loaded) keeps the tools before it cached, when the system
        // prompt's markers and the last message's leave one free.
        let system_markers = system_blocks.iter().flatten().filter(|b| b.cache_control.is_some()).count();
        let tools: Option<Vec<AnthropicTool>> = if req.tools.is_empty() {
            None
        } else {
            let last_loaded = req
                .tools
                .iter()
                .rposition(|t| !deferred.contains(&t.name))
                .filter(|_| system_markers + 2 <= MAX_CACHE_MARKERS);
            let tool_list: Vec<AnthropicTool> = req
                .tools
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let schema = t.input_schema.as_object().cloned().unwrap_or_default();
                    AnthropicTool {
                        name: t.name.clone(),
                        description: t.description.clone(),
                        defer_loading: deferred.contains(&t.name),
                        cache_control: (Some(i) == last_loaded).then(|| CacheControl { cache_type: "ephemeral".to_string() }),
                        input_schema: AnthropicInputSchema {
                            schema_type: "object".to_string(),
                            properties: schema.get("properties").cloned(),
                            required: schema.get("required").and_then(|v| {
                                v.as_array().map(|arr| {
                                    arr.iter()
                                        .filter_map(|v| v.as_str().map(String::from))
                                        .collect()
                                })
                            }),
                        },
                    }
                })
                .collect();
            Some(tool_list)
        };

        // Exactly one message marker, on the last message: a plain-text
        // message becomes a text block to carry it, so each step reads the
        // previous step's whole prefix from the cache.
        mark_last_message(&mut messages);

        // Map the cross-provider ToolChoice to Anthropic's shape (Auto → omitted).
        let tool_choice = match &req.tool_choice {
            ToolChoice::Auto => None,
            ToolChoice::Any => Some(serde_json::json!({"type": "any"})),
            ToolChoice::Tool(name) => Some(serde_json::json!({"type": "tool", "name": name})),
            ToolChoice::None => Some(serde_json::json!({"type": "none"})),
        };

        AnthropicApiRequest {
            model,
            max_tokens,
            messages,
            system: system_blocks,
            tools,
            tool_choice,
            stream: true,
            thinking: if req.enable_thinking {
                Some(ThinkingConfig {
                    thinking_type: "enabled".to_string(),
                    budget_tokens: budget,
                })
            } else {
                None
            },
        }
    }

    /// Build Anthropic API messages from our generic format. A result that
    /// loaded tools in `referable` names them by `tool_reference` in place of
    /// their definitions' text; the names it referenced come back with the
    /// messages.
    fn build_messages(
        &self,
        req: &ChatRequest,
        referable: &std::collections::HashSet<&str>,
    ) -> (Vec<AnthropicMessage>, String, std::collections::HashSet<String>) {
        let mut referenced = std::collections::HashSet::new();
        let mut system_prompt = req.system.clone();
        let mut messages = Vec::new();

        // First pass: collect tool call IDs and tool result IDs for orphan filtering
        let mut all_tool_call_ids = std::collections::HashSet::new();
        let mut responded_tool_ids = std::collections::HashSet::new();

        for msg in &req.messages {
            if msg.role == "assistant"
                && let Some(ref tc_val) = msg.tool_calls
                && let Ok(tcs) = serde_json::from_value::<Vec<SessionToolCall>>(tc_val.clone())
            {
                for tc in &tcs {
                    all_tool_call_ids.insert(tc.id.clone());
                }
            }
            if msg.role == "tool"
                && let Some(ref tr_val) = msg.tool_results
                && let Ok(results) =
                    serde_json::from_value::<Vec<SessionToolResult>>(tr_val.clone())
            {
                for r in &results {
                    responded_tool_ids.insert(r.tool_call_id.clone());
                }
            }
        }

        for msg in &req.messages {
            match msg.role.as_str() {
                "system" => {
                    if system_prompt.is_empty() {
                        system_prompt = msg.content.clone();
                    } else {
                        system_prompt.push_str("\n\n");
                        system_prompt.push_str(&msg.content);
                    }
                }
                "user" => {
                    if msg.content.is_empty() && msg.images.is_none() {
                        continue;
                    }
                    if let Some(ref images) = msg.images {
                        let mut blocks = Vec::new();
                        if !msg.content.is_empty() {
                            blocks.push(ContentBlock::Text {
                                text: msg.content.clone(),
                                cache_control: None,
                            });
                        }
                        for img in images {
                            blocks.push(ContentBlock::Image {
                                source: ImageSource {
                                    source_type: "base64".to_string(),
                                    media_type: img.media_type.clone(),
                                    data: img.data.clone(),
                                },
                                cache_control: None,
                            });
                        }
                        messages.push(AnthropicMessage {
                            role: "user".to_string(),
                            content: AnthropicContent::Blocks(blocks),
                        });
                    } else {
                        messages.push(AnthropicMessage {
                            role: "user".to_string(),
                            content: AnthropicContent::Text(msg.content.clone()),
                        });
                    }
                }
                "assistant" => {
                    // Thinking first, each block as it came, as the API
                    // requires for a turn that continues a tool loop.
                    let mut blocks: Vec<ContentBlock> = msg
                        .thinking
                        .iter()
                        .map(|b| match b.clone() {
                            ThinkingBlock::Thinking { thinking, signature } => ContentBlock::Thinking { thinking, signature },
                            ThinkingBlock::RedactedThinking { data } => ContentBlock::RedactedThinking { data },
                        })
                        .collect();

                    if !msg.content.is_empty() {
                        blocks.push(ContentBlock::Text {
                            text: msg.content.clone(),
                            cache_control: None,
                        });
                    }

                    if let Some(ref tc_val) = msg.tool_calls
                        && let Ok(tcs) =
                            serde_json::from_value::<Vec<SessionToolCall>>(tc_val.clone())
                    {
                        for tc in tcs {
                            if !responded_tool_ids.contains(&tc.id) {
                                continue;
                            }
                            let input: serde_json::Value =
                                serde_json::from_str(&tc.input.to_string())
                                    .unwrap_or(serde_json::Value::Object(Default::default()));
                            blocks.push(ContentBlock::ToolUse {
                                id: tc.id,
                                name: tc.name,
                                input,
                                cache_control: None,
                            });
                        }
                    }

                    if !blocks.is_empty() {
                        messages.push(AnthropicMessage {
                            role: "assistant".to_string(),
                            content: AnthropicContent::Blocks(blocks),
                        });
                    }
                }
                "tool" => {
                    if let Some(ref tr_val) = msg.tool_results
                        && let Ok(results) =
                            serde_json::from_value::<Vec<SessionToolResult>>(tr_val.clone())
                    {
                        let mut blocks = Vec::new();
                        for r in results {
                            if !all_tool_call_ids.contains(&r.tool_call_id)
                                || !responded_tool_ids.contains(&r.tool_call_id)
                            {
                                continue;
                            }
                            let content = result_content(&r, referable, &mut referenced);
                            blocks.push(ContentBlock::ToolResult {
                                tool_use_id: r.tool_call_id,
                                content,
                                is_error: r.is_error,
                                cache_control: None,
                            });
                        }
                        if !blocks.is_empty() {
                            messages.push(AnthropicMessage {
                                role: "user".to_string(),
                                content: AnthropicContent::Blocks(blocks),
                            });
                        }
                    }
                }
                _ => {}
            }
        }

        (messages, system_prompt, referenced)
    }

    /// Handle the SSE stream from Anthropic.
    async fn handle_stream(response: reqwest::Response, tx: mpsc::Sender<StreamEvent>) {
        let mut current_tool_id = String::new();
        let mut current_tool_name = String::new();
        let mut input_buffer = String::new();
        let mut last_stop_reason: Option<String> = None;
        // The thinking block being streamed, handed over whole at its stop.
        let mut thinking: Option<ThinkingBlock> = None;

        let mut byte_stream = response.bytes_stream();
        let mut line_buf = String::new();

        while let Some(chunk_result) = byte_stream.next().await {
            let chunk = match chunk_result {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx
                        .send(StreamEvent::error(format!("stream read error: {e}")))
                        .await;
                    return;
                }
            };

            let text = match std::str::from_utf8(&chunk) {
                Ok(t) => t,
                Err(e) => {
                    let _ = tx
                        .send(StreamEvent::error(format!("invalid utf8: {e}")))
                        .await;
                    return;
                }
            };

            line_buf.push_str(text);

            // Process complete lines
            while let Some(newline_pos) = line_buf.find('\n') {
                let line = line_buf[..newline_pos].to_string();
                line_buf = line_buf[newline_pos + 1..].to_string();

                match sse::parse_sse_line(&line) {
                    SseEvent::Data(data) => {
                        // Parse the Anthropic SSE event
                        let event: AnthropicStreamEvent = match serde_json::from_str(&data) {
                            Ok(e) => e,
                            Err(e) => {
                                debug!(error = %e, data = %::types::redact::redact(&data), "failed to parse Anthropic event");
                                continue;
                            }
                        };

                        match event.event_type.as_str() {
                            "message_start" => {
                                if let Some(msg) = event.message
                                    && let Some(usage) = msg.usage
                                {
                                    let _ = tx
                                        .send(StreamEvent::usage(UsageInfo {
                                            input_tokens: usage.input_tokens,
                                            output_tokens: usage.output_tokens,
                                            cost_microdollars: None,
                                            cache_creation_input_tokens: usage
                                                .cache_creation_input_tokens
                                                .unwrap_or(0),
                                            cache_read_input_tokens: usage
                                                .cache_read_input_tokens
                                                .unwrap_or(0),
                                            overhead_tokens: 0,
                                        }))
                                        .await;
                                }
                            }
                            "message_delta" => {
                                if let Some(ref delta) = event.delta {
                                    if let Some(ref reason) = delta.stop_reason {
                                        last_stop_reason = Some(reason.clone());
                                    }
                                }
                                if let Some(usage) = event.usage {
                                    let _ = tx
                                        .send(StreamEvent::usage(UsageInfo {
                                            input_tokens: usage.input_tokens.unwrap_or(0),
                                            output_tokens: usage.output_tokens.unwrap_or(0),
                                            cost_microdollars: None,
                                            cache_creation_input_tokens: usage
                                                .cache_creation_input_tokens
                                                .unwrap_or(0),
                                            cache_read_input_tokens: usage
                                                .cache_read_input_tokens
                                                .unwrap_or(0),
                                            overhead_tokens: 0,
                                        }))
                                        .await;
                                }
                            }
                            "content_block_start" => {
                                if let Some(block) = event.content_block {
                                    match block.block_type.as_str() {
                                        "tool_use" => {
                                            current_tool_id = block.id.unwrap_or_default();
                                            current_tool_name = block.name.unwrap_or_default();
                                            input_buffer.clear();
                                        }
                                        "thinking" => {
                                            thinking = Some(ThinkingBlock::Thinking {
                                                thinking: block.thinking.unwrap_or_default(),
                                                signature: block.signature.unwrap_or_default(),
                                            });
                                        }
                                        "redacted_thinking" => {
                                            thinking = Some(ThinkingBlock::RedactedThinking {
                                                data: block.data.unwrap_or_default(),
                                            });
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            "content_block_delta" => {
                                if let Some(delta) = event.delta {
                                    match delta.delta_type.as_str() {
                                        "text_delta" => {
                                            if let Some(text) = delta.text {
                                                let _ = tx.send(StreamEvent::text(text)).await;
                                            }
                                        }
                                        "input_json_delta" => {
                                            if let Some(partial) = delta.partial_json {
                                                input_buffer.push_str(&partial);
                                            }
                                        }
                                        "thinking_delta" => {
                                            if let Some(text) = delta.thinking {
                                                if let Some(ThinkingBlock::Thinking { thinking: so_far, .. }) = thinking.as_mut() {
                                                    so_far.push_str(&text);
                                                }
                                                let _ = tx.send(StreamEvent::thinking(text)).await;
                                            }
                                        }
                                        "signature_delta" => {
                                            if let (Some(part), Some(ThinkingBlock::Thinking { signature, .. })) =
                                                (delta.signature, thinking.as_mut())
                                            {
                                                signature.push_str(&part);
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            "content_block_stop" => {
                                if let Some(block) = thinking.take() {
                                    let _ = tx.send(StreamEvent::thinking_block(block)).await;
                                }
                                if !current_tool_id.is_empty() {
                                    let input: serde_json::Value = serde_json::from_str(
                                        &input_buffer,
                                    )
                                    .unwrap_or(serde_json::Value::Object(Default::default()));
                                    let _ = tx
                                        .send(StreamEvent::tool_call(ToolCall {
                                            id: std::mem::take(&mut current_tool_id),
                                            name: std::mem::take(&mut current_tool_name),
                                            input,
                                        }))
                                        .await;
                                    input_buffer.clear();
                                }
                            }
                            "message_stop" => {
                                let done = match last_stop_reason.take() {
                                    Some(reason) => StreamEvent::done_with_reason(reason),
                                    None => StreamEvent::done(),
                                };
                                let _ = tx.send(done).await;
                                return;
                            }
                            "error" => {
                                let msg = event
                                    .error
                                    .map(|e| e.message)
                                    .unwrap_or_else(|| "unknown error".to_string());
                                let _ = tx.send(StreamEvent::error(msg)).await;
                                return;
                            }
                            _ => {}
                        }
                    }
                    SseEvent::Event(_) => {
                        // Anthropic sends "event: <type>" lines before "data: " lines.
                        // We parse the type from the data JSON itself, so we can skip this.
                    }
                    SseEvent::Done => {
                        let done = match last_stop_reason.take() {
                            Some(reason) => StreamEvent::done_with_reason(reason),
                            None => StreamEvent::done(),
                        };
                        let _ = tx.send(done).await;
                        return;
                    }
                    SseEvent::Skip => {}
                }
            }
        }

        let done = match last_stop_reason.take() {
            Some(reason) => StreamEvent::done_with_reason(reason),
            None => StreamEvent::done(),
        };
        let _ = tx.send(done).await;
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn id(&self) -> &str {
        "anthropic"
    }

    fn supports_vision(&self) -> bool {
        true
    }

    async fn stream(&self, req: &ChatRequest) -> Result<EventReceiver, ProviderError> {
        crate::chosen_model(&req.model, &self.model, "anthropic")?;
        let api_req = self.api_request(req);
        let model = api_req.model.as_str();

        info!(
            model = model,
            messages = api_req.messages.len(),
            tools = req.tools.len(),
            "sending Anthropic request"
        );

        let mut request = self
            .client
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json");
        if api_req.tools.iter().flatten().any(|t| t.defer_loading) {
            request = request.header("anthropic-beta", TOOL_REFERENCE_BETA);
        }
        let response = request
            .json(&api_req)
            .send()
            .await
            .map_err(|e| ProviderError::Request(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let retry_after_secs = crate::http::retry_after_secs(response.headers());
            let body = response.text().await.unwrap_or_default();
            if status.as_u16() == 429 {
                return Err(ProviderError::RateLimit { retry_after_secs });
            }
            if status.as_u16() == 401 {
                return Err(ProviderError::Auth(body));
            }
            // Check for context overflow
            if body.contains("context") && body.contains("exceeded") {
                return Err(ProviderError::ContextOverflow);
            }
            return Err(ProviderError::Api {
                code: status.as_u16().to_string(),
                message: body,
                retryable: status.as_u16() >= 500,
            });
        }

        let (tx, rx) = mpsc::channel(100);

        // Extract rate limit metadata from response headers
        let headers = response.headers();
        let remaining_requests = headers
            .get("anthropic-ratelimit-requests-remaining")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let remaining_tokens = headers
            .get("anthropic-ratelimit-tokens-remaining")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let reset_after = headers
            .get("anthropic-ratelimit-requests-reset")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<f64>().ok());

        if remaining_requests.is_some() || remaining_tokens.is_some() {
            let _ = tx
                .send(StreamEvent::rate_limit_info(RateLimitMeta {
                    remaining_requests,
                    remaining_tokens,
                    reset_after_secs: reset_after,
                    retry_after_secs: None,
                    ..Default::default()
                }))
                .await;
        }

        tokio::spawn(Self::handle_stream(response, tx));

        Ok(rx)
    }
}

// --- Anthropic API types ---

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
    /// The deferred tools this result loaded, as their definitions' entries
    /// (`function_entry`); its text shows them in a `<functions>` block.
    #[serde(default, rename = "loadedTools")]
    loaded_tools: Vec<serde_json::Value>,
}

/// The beta a request with deferred tools and `tool_reference` blocks names.
const TOOL_REFERENCE_BETA: &str = "advanced-tool-use-2025-11-20";

/// The cache markers one request may carry.
const MAX_CACHE_MARKERS: usize = 4;

/// Whether `model` reads `tool_reference` blocks and `defer_loading` tools:
/// every Claude model from the 4.5 generation on. Older ones get the
/// definitions in text and in the tools array.
fn reads_tool_references(model: &str) -> bool {
    const BEFORE: [&str; 6] = [
        "claude-3",
        "claude-opus-4-0",
        "claude-opus-4-1",
        "claude-opus-4-2025",
        "claude-sonnet-4-0",
        "claude-sonnet-4-2025",
    ];
    !BEFORE.iter().any(|old| model.starts_with(old))
}

/// A tool result's content. One that loaded tools the request may reference
/// sends its text without their `<functions>` block, then one
/// `tool_reference` per tool: the API shows each definition there. One whose
/// text was cleared since still references its tools. Any other result is
/// sent as stored.
fn result_content(
    r: &SessionToolResult,
    referable: &std::collections::HashSet<&str>,
    referenced: &mut std::collections::HashSet<String>,
) -> ToolResultContent {
    let loaded: Vec<ToolDefinition> = r.loaded_tools.iter().filter_map(definition_of).collect();
    if loaded.is_empty() || !loaded.iter().all(|d| referable.contains(d.name.as_str())) {
        return ToolResultContent::Text(r.content.clone());
    }
    let text = r.content.replacen(&functions_block(&loaded), "", 1);
    let mut blocks = Vec::new();
    if !text.trim().is_empty() {
        blocks.push(ToolResultBlock::Text { text: text.trim().to_string() });
    }
    for def in loaded {
        referenced.insert(def.name.clone());
        blocks.push(ToolResultBlock::ToolReference { tool_name: def.name });
    }
    ToolResultContent::Blocks(blocks)
}

#[derive(Debug, Serialize)]
struct AnthropicApiRequest {
    model: String,
    max_tokens: i32,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<Vec<SystemBlock>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<serde_json::Value>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<ThinkingConfig>,
}

#[derive(Debug, Serialize)]
struct ThinkingConfig {
    #[serde(rename = "type")]
    thinking_type: String,
    budget_tokens: i32,
}

#[derive(Debug, Serialize)]
struct SystemBlock {
    text: String,
    #[serde(rename = "type")]
    block_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

#[derive(Debug, Serialize)]
struct CacheControl {
    #[serde(rename = "type")]
    cache_type: String,
}

#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: String,
    content: AnthropicContent,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum AnthropicContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

#[derive(Debug, Serialize)]
struct ImageSource {
    #[serde(rename = "type")]
    source_type: String,
    media_type: String,
    data: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum ContentBlock {
    #[serde(rename = "thinking")]
    Thinking { thinking: String, signature: String },
    #[serde(rename = "redacted_thinking")]
    RedactedThinking { data: String },
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    #[serde(rename = "image")]
    Image {
        source: ImageSource,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: ToolResultContent,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
}

/// A tool result's content: its text, or blocks when it references tools.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ToolResultContent {
    Text(String),
    Blocks(Vec<ToolResultBlock>),
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum ToolResultBlock {
    #[serde(rename = "text")]
    Text { text: String },
    /// A deferred tool's definition, shown here by the API.
    #[serde(rename = "tool_reference")]
    ToolReference { tool_name: String },
}

/// Put the one message cache marker on the last block of the last message.
fn mark_last_message(messages: &mut [AnthropicMessage]) {
    let Some(last) = messages.last_mut() else {
        return;
    };
    if let AnthropicContent::Text(text) = &mut last.content {
        let text = std::mem::take(text);
        last.content = AnthropicContent::Blocks(vec![ContentBlock::Text { text, cache_control: None }]);
    }
    let AnthropicContent::Blocks(blocks) = &mut last.content else {
        return;
    };
    let cc = Some(CacheControl {
        cache_type: "ephemeral".to_string(),
    });
    match blocks.last_mut() {
        Some(ContentBlock::Text { cache_control, .. })
        | Some(ContentBlock::Image { cache_control, .. })
        | Some(ContentBlock::ToolUse { cache_control, .. })
        | Some(ContentBlock::ToolResult { cache_control, .. }) => *cache_control = cc,
        // A thinking block can't carry a cache marker.
        Some(ContentBlock::Thinking { .. }) | Some(ContentBlock::RedactedThinking { .. }) | None => {}
    }
}

#[derive(Debug, Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: AnthropicInputSchema,
    /// Loaded by a result's `tool_reference`: out of the cached prefix.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    defer_loading: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

#[derive(Debug, Serialize)]
struct AnthropicInputSchema {
    #[serde(rename = "type")]
    schema_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    properties: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    required: Option<Vec<String>>,
}

// --- Streaming event types ---

#[derive(Debug, Deserialize)]
struct AnthropicStreamEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    message: Option<AnthropicMessageStart>,
    #[serde(default)]
    content_block: Option<AnthropicContentBlock>,
    #[serde(default)]
    delta: Option<AnthropicDelta>,
    #[serde(default)]
    usage: Option<AnthropicUsageDelta>,
    #[serde(default)]
    error: Option<AnthropicError>,
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageStart {
    #[serde(default)]
    usage: Option<AnthropicUsage>,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: i32,
    output_tokens: i32,
    #[serde(default)]
    cache_creation_input_tokens: Option<i32>,
    #[serde(default)]
    cache_read_input_tokens: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsageDelta {
    #[serde(default)]
    input_tokens: Option<i32>,
    #[serde(default)]
    output_tokens: Option<i32>,
    #[serde(default)]
    cache_creation_input_tokens: Option<i32>,
    #[serde(default)]
    cache_read_input_tokens: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct AnthropicContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    thinking: Option<String>,
    #[serde(default)]
    signature: Option<String>,
    #[serde(default)]
    data: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicDelta {
    #[serde(rename = "type")]
    delta_type: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    partial_json: Option<String>,
    #[serde(default)]
    thinking: Option<String>,
    #[serde(default)]
    signature: Option<String>,
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicError {
    message: String,
}


#[cfg(test)]
mod tests {
    use super::*;

    /// A pack level's provider effort sets the thinking budget, and the
    /// answer keeps room beyond it; no effort keeps today's 10,000.
    #[test]
    fn provider_effort_sets_the_thinking_budget() {
        let p = AnthropicProvider::new("k".into(), "claude-x".into());
        let req = |effort| ChatRequest { enable_thinking: true, effort, ..ChatRequest::new(RequestTrace::new("test")) };
        let budget = |r: &AnthropicApiRequest| r.thinking.as_ref().map(|t| t.budget_tokens);
        let high = p.api_request(&req(Some(types::packs::ProviderEffort::High)));
        assert_eq!(budget(&high), Some(24_000));
        assert!(high.max_tokens >= 24_000 + 4_096, "{}", high.max_tokens);
        assert_eq!(budget(&p.api_request(&req(Some(types::packs::ProviderEffort::Low)))), Some(4_000));
        let none = p.api_request(&req(None));
        assert_eq!((budget(&none), none.max_tokens), (Some(10_000), 16_384), "today's request, unchanged");
    }

    fn request(messages: Vec<Message>) -> ChatRequest {
        ChatRequest {
            messages,
            system: "SYSTEM".into(),
            cache_breakpoints: vec![6],
            tools: vec![ToolDefinition {
                name: "read_file".into(),
                description: "Reads a file.".into(),
                input_schema: serde_json::json!({"type": "object", "properties": {}}),
            }],
            ..ChatRequest::new(RequestTrace::new("agent_turn"))
        }
    }

    fn user(text: &str) -> Message {
        Message { role: "user".into(), content: text.into(), ..Default::default() }
    }

    fn sent(req: &ChatRequest) -> serde_json::Value {
        let provider = AnthropicProvider::new("key".into(), "claude".into());
        serde_json::to_value(provider.api_request(req)).unwrap()
    }

    fn markers(v: &serde_json::Value) -> usize {
        match v {
            serde_json::Value::Object(o) => o.iter().map(|(k, v)| usize::from(k == "cache_control") + markers(v)).sum(),
            serde_json::Value::Array(a) => a.iter().map(markers).sum(),
            _ => 0,
        }
    }

    /// Cache marker placement: the system prompt's marker, the last tool
    /// loaded up front, and exactly one on the last message, which a
    /// plain-text message carries as a text block.
    #[test]
    fn one_marker_on_the_last_message_even_when_it_is_plain_text() {
        let body = sent(&request(vec![user("Hi"), Message { role: "assistant".into(), content: "Hello.".into(), ..Default::default() }, user("<system-reminder>\nIt is noon.\n</system-reminder>")]));
        assert_eq!(markers(&body), 3, "the system prompt, the tools and the last message: {body}");
        assert!(body["system"][0]["cache_control"].is_object());
        assert!(body["tools"][0]["cache_control"].is_object(), "{body}");
        let last = body["messages"].as_array().unwrap().last().unwrap();
        assert_eq!(last["content"][0]["text"], "<system-reminder>\nIt is noon.\n</system-reminder>");
        assert!(last["content"][0]["cache_control"].is_object(), "{last}");
        assert!(body["messages"][0]["content"].is_string(), "earlier messages stay as they are");
    }

    #[test]
    fn a_tool_result_last_carries_the_marker_on_its_block() {
        let call = Message {
            role: "assistant".into(),
            tool_calls: Some(serde_json::json!([{"id": "t1", "name": "read_file", "input": {}}])),
            ..Default::default()
        };
        let result = Message {
            role: "tool".into(),
            tool_results: Some(serde_json::json!([{"tool_call_id": "t1", "content": "text"}])),
            ..Default::default()
        };
        let body = sent(&request(vec![user("Read it"), call, result]));
        assert_eq!(markers(&body), 3, "{body}");
        let last = body["messages"].as_array().unwrap().last().unwrap();
        assert!(last["content"][0]["cache_control"].is_object(), "{last}");
    }

    fn tool(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.into(),
            description: format!("The {name} tool."),
            input_schema: serde_json::json!({"type": "object", "properties": {"x": {"type": "string"}}}),
        }
    }

    /// A session whose second step loaded `vm` with `find_tools`: the
    /// requests of step one and step two, as the loop sends them (the core,
    /// then the loaded tool).
    fn load_steps(model: &str) -> (serde_json::Value, serde_json::Value) {
        let core = vec![tool("find_tools"), tool("read_file")];
        let first = vec![user("Start a VM")];
        let mut second = first.clone();
        second.push(Message {
            role: "assistant".into(),
            tool_calls: Some(serde_json::json!([{"id": "f1", "name": "find_tools", "input": {"query": "select:vm"}}])),
            ..Default::default()
        });
        let vm = tool("vm");
        second.push(Message {
            role: "tool".into(),
            tool_results: Some(serde_json::json!([{
                "tool_call_id": "f1",
                "content": format!("{}\nLoaded: vm. Call them directly.", functions_block([&vm])),
                "loadedTools": [function_entry(&vm)],
            }])),
            ..Default::default()
        });
        let provider = AnthropicProvider::new("key".into(), model.into());
        let body = |messages, tools| {
            let req = ChatRequest { messages, tools, ..request(Vec::new()) };
            serde_json::to_value(provider.api_request(&req)).unwrap()
        };
        let mut loaded = core.clone();
        loaded.push(vm);
        (body(first, core), body(second, loaded))
    }

    /// A request as the API renders it for the cache: markers aside, and a
    /// message's string content the text block it stands for.
    fn without_markers(v: &serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::Object(o) => o
                .iter()
                .filter(|(k, _)| *k != "cache_control")
                .map(|(k, v)| match (k.as_str(), v) {
                    ("content", serde_json::Value::String(text)) if o.contains_key("role") => {
                        (k.clone(), serde_json::json!([{"type": "text", "text": text}]))
                    }
                    _ => (k.clone(), without_markers(v)),
                })
                .collect(),
            serde_json::Value::Array(a) => a.iter().map(without_markers).collect(),
            other => other.clone(),
        }
    }

    /// A result that loaded a tool names it by `tool_reference` in place of
    /// its definition's text, and the tool goes deferred.
    #[test]
    fn a_loaded_tool_is_referenced_by_its_result_and_deferred() {
        let (_, body) = load_steps("claude-sonnet-4-6");
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools[2]["name"], "vm");
        assert_eq!(tools[2]["defer_loading"], true);
        assert!(tools[2].get("cache_control").is_none(), "a deferred tool can't carry a marker");
        assert!(tools[0].get("defer_loading").is_none());
        assert!(tools[1]["cache_control"].is_object(), "the last tool loaded up front carries it: {body}");
        let result = &body["messages"][2]["content"][0];
        assert_eq!(
            result["content"],
            serde_json::json!([{"type": "text", "text": "Loaded: vm. Call them directly."}, {"type": "tool_reference", "tool_name": "vm"}])
        );
    }

    /// The cache survives a load: the request after it starts with the
    /// request before it, byte for byte (markers aside), in the order the
    /// API reads it: the tools loaded up front, the system prompt, the
    /// messages. Before, the tools array grew at its head of the prefix.
    #[test]
    fn a_load_keeps_the_cached_prefix() {
        let (before, after) = load_steps("claude-opus-4-8");
        let up_front = |b: &serde_json::Value| {
            b["tools"].as_array().unwrap().iter().filter(|t| t.get("defer_loading").is_none()).map(without_markers).collect::<Vec<_>>()
        };
        assert_eq!(up_front(&before), up_front(&after));
        assert_eq!(without_markers(&before["system"]), without_markers(&after["system"]));
        let n = before["messages"].as_array().unwrap().len();
        assert_eq!(
            without_markers(&before["messages"]),
            serde_json::Value::Array(after["messages"].as_array().unwrap()[..n].iter().map(without_markers).collect())
        );
    }

    /// A model from before tool references gets the definition in text and
    /// in the tools array, as it always did.
    #[test]
    fn an_older_model_keeps_definitions_in_text() {
        let (_, body) = load_steps("claude-3-7-sonnet-latest");
        assert!(body["tools"][2].get("defer_loading").is_none());
        assert!(body["messages"][2]["content"][0]["content"].as_str().unwrap().starts_with("<functions>"));
        assert!(!reads_tool_references("claude-opus-4-1-20250805"));
        assert!(!reads_tool_references("claude-sonnet-4-20250514"));
        assert!(reads_tool_references("claude-haiku-4-5"));
        assert!(reads_tool_references("claude-opus-5"));
    }

    /// A result cleared since it loaded its tools (its text saved away)
    /// still references them: the tool stays deferred, the cache whole.
    #[test]
    fn a_cleared_result_still_references_its_tools() {
        let vm = tool("vm");
        let call = Message {
            role: "assistant".into(),
            tool_calls: Some(serde_json::json!([{"id": "f1", "name": "find_tools", "input": {}}])),
            ..Default::default()
        };
        let result = Message {
            role: "tool".into(),
            tool_results: Some(serde_json::json!([{"tool_call_id": "f1", "content": "saved:1", "loadedTools": [function_entry(&vm)]}])),
            ..Default::default()
        };
        let req = ChatRequest { tools: vec![tool("find_tools"), vm], ..request(vec![user("go"), call, result]) };
        let body = sent(&req);
        assert_eq!(body["tools"][1]["defer_loading"], true);
        assert_eq!(body["messages"][2]["content"][0]["content"][1], serde_json::json!({"type": "tool_reference", "tool_name": "vm"}));
    }

    /// A thinking turn goes back with its blocks first, each unchanged:
    /// signed thinking, then redacted thinking, then the text and the call.
    #[test]
    fn an_assistant_turn_sends_its_thinking_blocks_first() {
        let call = Message {
            role: "assistant".into(),
            content: "Reading it.".into(),
            tool_calls: Some(serde_json::json!([{"id": "t1", "name": "read_file", "input": {}}])),
            thinking: vec![
                ThinkingBlock::Thinking { thinking: "look first".into(), signature: "sig".into() },
                ThinkingBlock::RedactedThinking { data: "opaque".into() },
            ],
            ..Default::default()
        };
        let result = Message {
            role: "tool".into(),
            tool_results: Some(serde_json::json!([{"tool_call_id": "t1", "content": "text"}])),
            ..Default::default()
        };
        let body = sent(&request(vec![user("Read it"), call, result]));
        let content = &body["messages"][1]["content"];
        assert_eq!(content[0], serde_json::json!({"type": "thinking", "thinking": "look first", "signature": "sig"}));
        assert_eq!(content[1], serde_json::json!({"type": "redacted_thinking", "data": "opaque"}));
        assert_eq!(content[2]["type"], "text");
        assert_eq!(content[3]["type"], "tool_use");
    }

    /// The stream's thinking comes out as its deltas (for the owner to
    /// watch) and, at the block's stop, as one whole block with its
    /// signature; a redacted block comes out whole.
    #[tokio::test]
    async fn a_streamed_thinking_block_is_handed_over_whole_with_its_signature() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 2048];
            let _ = sock.read(&mut buf).await;
            let events = [
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"look "}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"first"}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-1"}}"#,
                r#"{"type":"content_block_stop","index":0}"#,
                r#"{"type":"content_block_start","index":1,"content_block":{"type":"redacted_thinking","data":"opaque"}}"#,
                r#"{"type":"content_block_stop","index":1}"#,
                r#"{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"t1","name":"read_file"}}"#,
                r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{}"}}"#,
                r#"{"type":"content_block_stop","index":2}"#,
                r#"{"type":"message_stop"}"#,
            ];
            let body: String = events.iter().map(|e| format!("data: {e}\n\n")).collect();
            let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{body}");
            sock.write_all(resp.as_bytes()).await.unwrap();
            sock.flush().await.unwrap();
        });
        let response = reqwest::get(format!("http://{addr}/")).await.unwrap();
        let (tx, mut rx) = mpsc::channel(64);
        AnthropicProvider::handle_stream(response, tx).await;
        server.await.unwrap();
        let mut deltas = String::new();
        let mut blocks = Vec::new();
        let mut order = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            match ev.event_type {
                StreamEventType::Thinking => deltas.push_str(&ev.text),
                StreamEventType::ThinkingBlock => {
                    blocks.extend(ev.block());
                    order.push("block");
                }
                StreamEventType::ToolCall => order.push("call"),
                _ => {}
            }
        }
        assert_eq!(deltas, "look first");
        assert_eq!(
            blocks,
            vec![
                ThinkingBlock::Thinking { thinking: "look first".into(), signature: "sig-1".into() },
                ThinkingBlock::RedactedThinking { data: "opaque".into() },
            ]
        );
        assert_eq!(order, vec!["block", "block", "call"]);
    }

}
