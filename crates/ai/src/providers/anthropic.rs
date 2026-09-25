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
            client: crate::http::streaming_client(),
            api_key,
            model,
            base_url: "https://api.anthropic.com".to_string(),
        }
    }

    /// The request as the API receives it, cache markers placed.
    fn api_request(&self, req: &ChatRequest) -> AnthropicApiRequest {
        let (mut messages, system_prompt) = self.build_messages(req);

        let model = if req.model.is_empty() {
            self.model.clone()
        } else {
            req.model.clone()
        };

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

        // Build tools with cache_control on the last tool for definition caching
        let tools: Option<Vec<AnthropicTool>> = if req.tools.is_empty() {
            None
        } else {
            let tool_list: Vec<AnthropicTool> = req
                .tools
                .iter()
                .map(|t| {
                    let schema = t.input_schema.as_object().cloned().unwrap_or_default();
                    AnthropicTool {
                        name: t.name.clone(),
                        description: t.description.clone(),
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
        // message becomes a text block to carry it (Claude Code 2.1.280,
        // `addCacheBreakpoints` / `userMessageToMessageParam` in
        // `src/services/api/claude.ts:588-620,3063-3091`). The tools carry
        // none: they come before the system prompt, whose marker covers them.
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
                    budget_tokens: 10000,
                })
            } else {
                None
            },
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
                            let content = if let Some((media_type, data)) = r
                                .image_url
                                .as_deref()
                                .and_then(crate::types::image_source_to_base64)
                            {
                                ToolResultContent::Blocks(vec![
                                    ToolResultContentBlock::Text {
                                        text: r.content.clone(),
                                    },
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

        (messages, system_prompt)
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
                                debug!("failed to parse Anthropic event: {e}, data: {data}");
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

    fn supports_tool_result_images(&self) -> bool {
        true
    }

    fn supports_vision(&self) -> bool {
        true
    }

    async fn stream(&self, req: &ChatRequest) -> Result<EventReceiver, ProviderError> {
        let api_req = self.api_request(req);
        let model = api_req.model.as_str();

        info!(
            model = model,
            messages = api_req.messages.len(),
            tools = req.tools.len(),
            "sending Anthropic request"
        );

        let response = self
            .client
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

/// Anthropic tool_result content: a plain string or an array of content blocks
/// (text + image). The API accepts both formats.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ToolResultContent {
    Text(String),
    Blocks(Vec<ToolResultContentBlock>),
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

    /// Claude Code's placement: the system prompt's marker, and exactly one
    /// on the last message, which a plain-text message carries as a text
    /// block. Before: none when the last row was plain text, and one on the
    /// last tool.
    #[test]
    fn one_marker_on_the_last_message_even_when_it_is_plain_text() {
        let body = sent(&request(vec![user("Hi"), Message { role: "assistant".into(), content: "Hello.".into(), ..Default::default() }, user("<system-reminder>\nIt is noon.\n</system-reminder>")]));
        assert_eq!(markers(&body), 2, "the system prompt and the last message: {body}");
        assert!(body["system"][0]["cache_control"].is_object());
        assert!(body["tools"][0].get("cache_control").is_none(), "the tools carry none");
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
        assert_eq!(markers(&body), 2, "{body}");
        let last = body["messages"].as_array().unwrap().last().unwrap();
        assert!(last["content"][0]["cache_control"].is_object(), "{last}");
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
