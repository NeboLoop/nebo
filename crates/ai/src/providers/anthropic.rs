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

impl AnthropicProvider {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            model,
            base_url: "https://api.anthropic.com".to_string(),
        }
    }

    /// Build Anthropic API messages from our generic format.
    fn build_messages(&self, req: &ChatRequest) -> (Vec<AnthropicMessage>, String) {
        let mut system_prompt = req.system.clone();
        let mut messages = Vec::new();

        // First pass: collect tool call IDs and tool result IDs for orphan filtering
        let mut all_tool_call_ids = std::collections::HashSet::new();
        let mut responded_tool_ids = std::collections::HashSet::new();

        for msg in &req.messages {
            if msg.role == "assistant"
                && let Some(ref tc_val) = msg.tool_calls
                    && let Ok(tcs) = serde_json::from_value::<Vec<SessionToolCall>>(tc_val.clone()) {
                        for tc in &tcs {
                            all_tool_call_ids.insert(tc.id.clone());
                        }
                    }
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
                            blocks.push(ContentBlock::Text { text: msg.content.clone(), cache_control: None });
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
                    let mut blocks = Vec::new();

                    if !msg.content.is_empty() {
                        blocks.push(ContentBlock::Text { text: msg.content.clone(), cache_control: None });
                    }

                    if let Some(ref tc_val) = msg.tool_calls
                        && let Ok(tcs) = serde_json::from_value::<Vec<SessionToolCall>>(tc_val.clone()) {
                            for tc in tcs {
                                if !responded_tool_ids.contains(&tc.id) {
                                    continue;
                                }
                                let input: serde_json::Value = serde_json::from_str(
                                    &tc.input.to_string()
                                ).unwrap_or(serde_json::Value::Object(Default::default()));
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
                        && let Ok(results) = serde_json::from_value::<Vec<SessionToolResult>>(tr_val.clone()) {
                            let mut blocks = Vec::new();
                            for r in results {
                                if !all_tool_call_ids.contains(&r.tool_call_id) || !responded_tool_ids.contains(&r.tool_call_id) {
                                    continue;
                                }
                                let content = if let Some(ref image_url) = r.image_url {
                                    let (media_type, data) = parse_data_url(image_url);
                                    ToolResultContent::Blocks(vec![
                                        ToolResultContentBlock::Text { text: r.content.clone() },
                                        ToolResultContentBlock::Image {
                                            source: ImageSource {
                                                source_type: "base64".to_string(),
                                                media_type,
                                                data,
                                            },
                                        },
                                    ])
                                } else {
                                    ToolResultContent::Text(r.content.clone())
                                };
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

        // Cache breakpoints: mark the last content block of the last 3 messages
        // with cache_control ephemeral for conversation context caching
        let len = messages.len();
        for i in (0..len).rev().take(3) {
            if let AnthropicContent::Blocks(ref mut blocks) = messages[i].content {
                if let Some(last_block) = blocks.last_mut() {
                    let cc = Some(CacheControl { cache_type: "ephemeral".to_string() });
                    match last_block {
                        ContentBlock::Text { cache_control, .. } => *cache_control = cc,
                        ContentBlock::Image { cache_control, .. } => *cache_control = cc,
                        ContentBlock::ToolUse { cache_control, .. } => *cache_control = cc,
                        ContentBlock::ToolResult { cache_control, .. } => *cache_control = cc,
                    }
                }
            }
        }

        (messages, system_prompt)
    }

    /// Handle the SSE stream from Anthropic.
    async fn handle_stream(
        response: reqwest::Response,
        tx: mpsc::Sender<StreamEvent>,
    ) {
        let mut current_tool_id = String::new();
        let mut current_tool_name = String::new();
        let mut input_buffer = String::new();

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
                                debug!("failed to parse Anthropic event: {e}, data: {data}");
                                continue;
                            }
                        };

                        match event.event_type.as_str() {
                            "message_start" => {
                                if let Some(msg) = event.message
                                    && let Some(usage) = msg.usage {
                                        let _ = tx.send(StreamEvent::usage(UsageInfo {
                                            input_tokens: usage.input_tokens,
                                            output_tokens: usage.output_tokens,
                                            cache_creation_input_tokens: usage.cache_creation_input_tokens.unwrap_or(0),
                                            cache_read_input_tokens: usage.cache_read_input_tokens.unwrap_or(0),
                                        })).await;
                                    }
                            }
                            "message_delta" => {
                                if let Some(usage) = event.usage {
                                    let _ = tx.send(StreamEvent::usage(UsageInfo {
                                        input_tokens: usage.input_tokens.unwrap_or(0),
                                        output_tokens: usage.output_tokens.unwrap_or(0),
                                        cache_creation_input_tokens: usage.cache_creation_input_tokens.unwrap_or(0),
                                        cache_read_input_tokens: usage.cache_read_input_tokens.unwrap_or(0),
                                    })).await;
                                }
                            }
                            "content_block_start" => {
                                if let Some(block) = event.content_block
                                    && block.block_type == "tool_use" {
                                        current_tool_id = block.id.unwrap_or_default();
                                        current_tool_name = block.name.unwrap_or_default();
                                        input_buffer.clear();
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
                                            if let Some(thinking) = delta.thinking {
                                                let _ = tx.send(StreamEvent::thinking(thinking)).await;
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            "content_block_stop" => {
                                if !current_tool_id.is_empty() {
                                    let input: serde_json::Value = serde_json::from_str(&input_buffer)
                                        .unwrap_or(serde_json::Value::Object(Default::default()));
                                    let _ = tx.send(StreamEvent::tool_call(ToolCall {
                                        id: std::mem::take(&mut current_tool_id),
                                        name: std::mem::take(&mut current_tool_name),
                                        input,
                                    })).await;
                                    input_buffer.clear();
                                }
                            }
                            "message_stop" => {
                                let _ = tx.send(StreamEvent::done()).await;
                                return;
                            }
                            "error" => {
                                let msg = event.error.map(|e| e.message).unwrap_or_else(|| "unknown error".to_string());
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
                        let _ = tx.send(StreamEvent::done()).await;
                        return;
                    }
                    SseEvent::Skip => {}
                }
            }
        }

        let _ = tx.send(StreamEvent::done()).await;
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn id(&self) -> &str {
        "anthropic"
    }

    fn supports_tool_result_images(&self) -> bool {
        true
    }

    async fn stream(
        &self,
        req: &ChatRequest,
    ) -> Result<EventReceiver, ProviderError> {
        let (messages, system_prompt) = self.build_messages(req);

        let model = if req.model.is_empty() { &self.model } else { &req.model };

        let max_tokens = if req.max_tokens > 0 {
            req.max_tokens
        } else if req.enable_thinking {
            16384
        } else {
            8192
        };

        // Build system blocks with caching.
        //
        // When `cache_breakpoints` are provided (byte offsets into `system_prompt`),
        // we split the system prompt at those offsets and mark each prefix block
        // with `cache_control: { type: "ephemeral" }` so that the stable prefix
        // can be served from Anthropic's prompt cache at ~90% discount.
        //
        // Fallback: if no breakpoints but `static_system` is set (legacy path),
        // split at the static/dynamic boundary.  Otherwise send the whole prompt
        // as a single cached block.
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
                        cache_control: Some(CacheControl { cache_type: "ephemeral".to_string() }),
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
                        cache_control: Some(CacheControl { cache_type: "ephemeral".to_string() }),
                    }])
                } else {
                    Some(blocks)
                }
            } else if !req.static_system.is_empty() && system_prompt.starts_with(&req.static_system) {
                let dynamic_suffix = system_prompt.strip_prefix(&req.static_system).unwrap_or("");
                let mut blocks = vec![
                    SystemBlock {
                        text: req.static_system.clone(),
                        block_type: "text".to_string(),
                        cache_control: Some(CacheControl { cache_type: "ephemeral".to_string() }),
                    },
                ];
                if !dynamic_suffix.is_empty() {
                    blocks.push(SystemBlock {
                        text: dynamic_suffix.to_string(),
                        block_type: "text".to_string(),
                        cache_control: None,
                    });
                }
                Some(blocks)
            } else {
                Some(vec![SystemBlock {
                    text: system_prompt,
                    block_type: "text".to_string(),
                    cache_control: Some(CacheControl { cache_type: "ephemeral".to_string() }),
                }])
            }
        } else {
            None
        };

        // Build tools with cache_control on the last tool for definition caching
        let tools: Option<Vec<AnthropicTool>> = if req.tools.is_empty() {
            None
        } else {
            let mut tool_list: Vec<AnthropicTool> = req.tools.iter().map(|t| {
                let schema = t.input_schema.as_object().cloned().unwrap_or_default();
                AnthropicTool {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    input_schema: AnthropicInputSchema {
                        schema_type: "object".to_string(),
                        properties: schema.get("properties").cloned(),
                        required: schema.get("required").and_then(|v| {
                            v.as_array().map(|arr| {
                                arr.iter().filter_map(|v| v.as_str().map(String::from)).collect()
                            })
                        }),
                    },
                    cache_control: None,
                }
            }).collect();
            // Mark the last tool with cache_control for tool definition caching
            if let Some(last) = tool_list.last_mut() {
                last.cache_control = Some(CacheControl { cache_type: "ephemeral".to_string() });
            }
            Some(tool_list)
        };

        let api_req = AnthropicApiRequest {
            model: model.to_string(),
            max_tokens,
            messages,
            system: system_blocks,
            tools,
            stream: true,
            thinking: if req.enable_thinking {
                Some(ThinkingConfig { thinking_type: "enabled".to_string(), budget_tokens: 10000 })
            } else {
                None
            },
        };

        info!(model = model, messages = api_req.messages.len(), tools = req.tools.len(), "sending Anthropic request");

        let response = self.client
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&api_req)
            .send()
            .await
            .map_err(|e| ProviderError::Request(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            if status.as_u16() == 429 {
                return Err(ProviderError::RateLimit);
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
    #[serde(default)]
    image_url: Option<String>,
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

/// Anthropic tool_result content: a plain string or an array of content blocks
/// (text + image). The API accepts both formats.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ToolResultContent {
    Text(String),
    Blocks(Vec<ToolResultContentBlock>),
}

/// Content block types allowed inside a tool_result content array.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum ToolResultContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { source: ImageSource },
}

#[derive(Debug, Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: AnthropicInputSchema,
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
}

#[derive(Debug, Deserialize)]
struct AnthropicError {
    message: String,
}

/// Parse a data URL (e.g. `data:image/jpeg;base64,/9j/4AAQ...`) into
/// (media_type, base64_data). Falls back to `image/png` if the URL
/// doesn't match the expected format.
fn parse_data_url(url: &str) -> (String, String) {
    if let Some(rest) = url.strip_prefix("data:") {
        if let Some((header, data)) = rest.split_once(",") {
            let media_type = header.strip_suffix(";base64").unwrap_or(header);
            return (media_type.to_string(), data.to_string());
        }
    }
    // Bare base64 without data URL prefix — assume PNG
    ("image/png".to_string(), url.to_string())
}
