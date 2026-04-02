use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls,
    ChatCompletionRequestAssistantMessage, ChatCompletionRequestAssistantMessageContent,
    ChatCompletionRequestMessage, ChatCompletionRequestMessageContentPartImage,
    ChatCompletionRequestMessageContentPartText, ChatCompletionRequestSystemMessage,
    ChatCompletionRequestSystemMessageContent, ChatCompletionRequestToolMessage,
    ChatCompletionRequestToolMessageContent, ChatCompletionRequestUserMessage,
    ChatCompletionRequestUserMessageContent, ChatCompletionRequestUserMessageContentPart,
    ChatCompletionTool, ChatCompletionTools, CreateChatCompletionRequest,
    CreateChatCompletionStreamResponse, FunctionCall, FunctionObject, ImageUrl,
};
use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::RwLock;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::sse::{SseEvent, parse_sse_line};
use crate::types::*;

/// OpenAI provider using async-openai types with raw reqwest streaming.
///
/// Uses the SDK's typed request/response structs for serialization safety,
/// but makes HTTP requests directly with reqwest to avoid reqwest-eventsource's
/// automatic SSE reconnection (which causes infinite retries on 502 from Janus).
pub struct OpenAIProvider {
    api_key: String,
    model: String,
    base_url: String,
    provider_id: String,
    /// Optional bot ID sent as X-Bot-ID header (used by Janus for per-bot billing).
    bot_id: Option<String>,
    /// Optional lane identifier sent as X-Lane header (used by Janus for routing).
    lane: Option<String>,
    /// HTTP client wrapped in RwLock for connection reset recovery.
    http_client: RwLock<reqwest::Client>,
}

impl OpenAIProvider {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            api_key,
            model,
            base_url: "https://api.openai.com/v1".to_string(),
            provider_id: "openai".to_string(),
            bot_id: None,
            lane: None,
            http_client: RwLock::new(reqwest::Client::new()),
        }
    }

    /// Create with a custom base URL for OpenAI-compatible APIs.
    pub fn with_base_url(api_key: String, model: String, base_url: String) -> Self {
        Self {
            api_key,
            model,
            base_url,
            provider_id: "openai".to_string(),
            bot_id: None,
            lane: None,
            http_client: RwLock::new(reqwest::Client::new()),
        }
    }

    /// Override the provider ID (e.g., "janus", "deepseek").
    pub fn set_provider_id(&mut self, id: impl Into<String>) {
        self.provider_id = id.into();
    }

    /// Set the bot ID for X-Bot-ID header (used by Janus for per-bot billing).
    pub fn set_bot_id(&mut self, id: impl Into<String>) {
        self.bot_id = Some(id.into());
    }

    /// Set the lane for X-Lane header (used by Janus for routing).
    pub fn set_lane(&mut self, lane: impl Into<String>) {
        self.lane = Some(lane.into());
    }

    /// Build async-openai messages from our generic format.
    fn build_messages(&self, req: &ChatRequest) -> Vec<ChatCompletionRequestMessage> {
        // Build indexes for history sanitisation (same as Go buildMessages):
        // - respondedToolIDs: tool_call_ids that have a matching tool-result message
        // - issuedToolIDs: tool_call_ids that appear in an assistant tool_calls field
        let mut responded_tool_ids = HashSet::new();
        let mut issued_tool_ids = HashSet::new();

        for msg in &req.messages {
            if msg.role == "tool"
                && let Some(ref tr_val) = msg.tool_results
                && let Ok(results) =
                    serde_json::from_value::<Vec<SessionToolResult>>(tr_val.clone())
            {
                for r in &results {
                    responded_tool_ids.insert(r.tool_call_id.clone());
                }
            }
            if msg.role == "assistant"
                && let Some(ref tc_val) = msg.tool_calls
                && let Ok(tcs) = serde_json::from_value::<Vec<SessionToolCall>>(tc_val.clone())
            {
                for tc in &tcs {
                    issued_tool_ids.insert(tc.id.clone());
                }
            }
        }

        let mut messages = Vec::new();
        let mut skipped_orphans = 0u32;

        // Add system message
        if !req.system.is_empty() {
            messages.push(ChatCompletionRequestMessage::System(
                ChatCompletionRequestSystemMessage {
                    content: ChatCompletionRequestSystemMessageContent::Text(req.system.clone()),
                    name: None,
                },
            ));
        }

        for msg in &req.messages {
            match msg.role.as_str() {
                "user" => {
                    if msg.content.is_empty() && msg.images.is_none() {
                        continue;
                    }
                    if let Some(ref images) = msg.images {
                        let mut parts: Vec<ChatCompletionRequestUserMessageContentPart> = Vec::new();
                        if !msg.content.is_empty() {
                            parts.push(ChatCompletionRequestUserMessageContentPart::Text(
                                ChatCompletionRequestMessageContentPartText { text: msg.content.clone() },
                            ));
                        }
                        for img in images {
                            let url = format!("data:{};base64,{}", img.media_type, img.data);
                            parts.push(ChatCompletionRequestUserMessageContentPart::ImageUrl(
                                ChatCompletionRequestMessageContentPartImage {
                                    image_url: ImageUrl { url, detail: None },
                                },
                            ));
                        }
                        messages.push(ChatCompletionRequestMessage::User(
                            ChatCompletionRequestUserMessage {
                                content: ChatCompletionRequestUserMessageContent::Array(parts),
                                name: None,
                            },
                        ));
                    } else {
                        messages.push(ChatCompletionRequestMessage::User(
                            ChatCompletionRequestUserMessage {
                                content: ChatCompletionRequestUserMessageContent::Text(
                                    msg.content.clone(),
                                ),
                                name: None,
                            },
                        ));
                    }
                }
                "assistant" => {
                    let mut tool_calls = Vec::new();

                    if let Some(ref tc_val) = msg.tool_calls
                        && let Ok(tcs) =
                            serde_json::from_value::<Vec<SessionToolCall>>(tc_val.clone())
                    {
                        for tc in tcs {
                            if !responded_tool_ids.contains(&tc.id) {
                                skipped_orphans += 1;
                                continue;
                            }
                            tool_calls.push(ChatCompletionMessageToolCalls::Function(
                                ChatCompletionMessageToolCall {
                                    id: tc.id,
                                    function: FunctionCall {
                                        name: tc.name,
                                        arguments: tc.input.to_string(),
                                    },
                                },
                            ));
                        }
                    }

                    if !msg.content.is_empty() || !tool_calls.is_empty() {
                        // Some gateways reject null content with tool_calls
                        let content = if msg.content.is_empty() && !tool_calls.is_empty() {
                            Some(ChatCompletionRequestAssistantMessageContent::Text(
                                " ".to_string(),
                            ))
                        } else if !msg.content.is_empty() {
                            Some(ChatCompletionRequestAssistantMessageContent::Text(
                                msg.content.clone(),
                            ))
                        } else {
                            None
                        };

                        messages.push(ChatCompletionRequestMessage::Assistant(
                            ChatCompletionRequestAssistantMessage {
                                content,
                                tool_calls: if tool_calls.is_empty() {
                                    None
                                } else {
                                    Some(tool_calls)
                                },
                                ..Default::default()
                            },
                        ));
                    }
                }
                "tool" => {
                    if let Some(ref tr_val) = msg.tool_results
                        && let Ok(results) =
                            serde_json::from_value::<Vec<SessionToolResult>>(tr_val.clone())
                    {
                        for r in results {
                            if issued_tool_ids.contains(&r.tool_call_id)
                                && responded_tool_ids.contains(&r.tool_call_id)
                            {
                                messages.push(ChatCompletionRequestMessage::Tool(
                                    ChatCompletionRequestToolMessage {
                                        content: ChatCompletionRequestToolMessageContent::Text(
                                            r.content,
                                        ),
                                        tool_call_id: r.tool_call_id,
                                    },
                                ));
                            }
                        }
                    }
                }
                "system" => {
                    if msg.content.is_empty() {
                        continue;
                    }
                    messages.push(ChatCompletionRequestMessage::System(
                        ChatCompletionRequestSystemMessage {
                            content: ChatCompletionRequestSystemMessageContent::Text(
                                msg.content.clone(),
                            ),
                            name: None,
                        },
                    ));
                }
                _ => {}
            }
        }

        if skipped_orphans > 0 {
            debug!(skipped_orphans, "cleaned orphaned tool_calls from history");
        }

        messages
    }

    /// Handle raw SSE byte stream, converting to our StreamEvent types.
    ///
    /// Uses our own SSE parser + SDK response types for deserialization.
    /// No reqwest-eventsource — no automatic reconnection on errors.
    ///
    /// Handles Janus-specific quirks from the Go implementation:
    /// - Breaks on finish_reason (Janus may not send [DONE] sentinel)
    /// - Deduplicates tool names/arguments (Janus sends complete values in every chunk)
    /// - Fallback tool emission from accumulator at end of stream
    async fn handle_stream(response: reqwest::Response, tx: mpsc::Sender<StreamEvent>) {
        let mut byte_stream = response.bytes_stream();
        let mut line_buf = String::new();

        // Accumulate tool calls by index
        let mut tool_calls: HashMap<u32, AccumulatedToolCall> = HashMap::new();
        let mut emitted_tool_calls = HashSet::new();
        // Janus dedup: track which tool indices already have name/complete args
        let mut seen_tool_name: HashSet<u32> = HashSet::new();
        let mut seen_tool_args: HashSet<u32> = HashSet::new();

        let mut text_chunks = 0u32;
        let mut chunk_count = 0u32;
        let mut finished = false;
        let mut last_finish_reason: Option<String> = None;
        let mut last_provider_metadata: Option<HashMap<String, String>> = None;

        'outer: while let Some(result) = byte_stream.next().await {
            let bytes = match result {
                Ok(b) => b,
                Err(e) => {
                    warn!(error = %e, "stream read error");
                    let _ = tx
                        .send(StreamEvent::error(format!("stream read error: {e}")))
                        .await;
                    break;
                }
            };

            let text = String::from_utf8_lossy(&bytes);
            line_buf.push_str(&text);

            // Process complete lines
            while let Some(newline_pos) = line_buf.find('\n') {
                let line = line_buf[..newline_pos].to_string();
                line_buf = line_buf[newline_pos + 1..].to_string();

                match parse_sse_line(&line) {
                    SseEvent::Done => {
                        finished = true;
                        break 'outer;
                    }
                    SseEvent::Data(data) => {
                        // Pre-parse as Value to check for errors and extract provider_metadata
                        let raw_val = serde_json::from_str::<serde_json::Value>(&data).ok();

                        // Check for OpenAI-compatible error responses (e.g. from Janus)
                        if let Some(ref val) = raw_val {
                            if let Some(err_obj) = val.get("error") {
                                let msg = err_obj
                                    .get("message")
                                    .and_then(|m| m.as_str())
                                    .unwrap_or("unknown provider error");
                                let code = err_obj
                                    .get("code")
                                    .and_then(|c| c.as_str())
                                    .unwrap_or("");
                                let err_type = err_obj
                                    .get("type")
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("");
                                warn!(
                                    error = msg,
                                    code = code,
                                    error_type = err_type,
                                    raw = %err_obj,
                                    "provider returned error in SSE stream"
                                );
                                let _ = tx
                                    .send(StreamEvent::error(msg.to_string()))
                                    .await;
                                finished = true;
                                break 'outer;
                            }
                        }

                        // Extract provider_metadata from Janus for tool stickiness
                        if let Some(ref val) = raw_val {
                            if let Some(pm) = val.get("provider_metadata") {
                                if let Ok(meta) = serde_json::from_value::<HashMap<String, String>>(pm.clone()) {
                                    last_provider_metadata = Some(meta);
                                }
                            }
                        }

                        let response: CreateChatCompletionStreamResponse =
                            match serde_json::from_str(&data) {
                                Ok(r) => r,
                                Err(e) => {
                                    warn!(error = %e, data = &data, "failed to parse SSE chunk");
                                    continue;
                                }
                            };

                        chunk_count += 1;

                        if chunk_count == 1 {
                            debug!(
                                model = %response.model,
                                choices = response.choices.len(),
                                "first stream chunk"
                            );
                        }

                        for choice in &response.choices {
                            // Stream text content
                            if let Some(content) = choice.delta.content.as_deref() {
                                if !content.is_empty() {
                                    text_chunks += 1;
                                    let _ = tx.send(StreamEvent::text(content)).await;
                                }
                            }

                            // Accumulate tool calls by index, with Janus deduplication
                            if let Some(ref tcs) = choice.delta.tool_calls {
                                for tc in tcs {
                                    let idx = tc.index;
                                    let entry = tool_calls
                                        .entry(idx)
                                        .or_insert_with(|| AccumulatedToolCall {
                                            id: String::new(),
                                            name: String::new(),
                                            arguments: String::new(),
                                        });

                                    if let Some(id) = tc.id.as_deref() {
                                        if !id.is_empty() {
                                            entry.id = id.to_string();
                                        }
                                    }

                                    if let Some(ref func) = tc.function {
                                        // Dedup tool name: Janus sends name in every chunk
                                        if let Some(name) = func.name.as_deref() {
                                            if !name.is_empty()
                                                && !seen_tool_name.contains(&idx)
                                            {
                                                entry.name = name.to_string();
                                                seen_tool_name.insert(idx);
                                            }
                                        }

                                        // Dedup tool args: Janus sends complete JSON in every chunk
                                        if let Some(args) = func.arguments.as_deref() {
                                            if !args.is_empty() {
                                                if seen_tool_args.contains(&idx) {
                                                    // Already have complete args, skip duplicate
                                                } else if serde_json::from_str::<
                                                    serde_json::Value,
                                                >(
                                                    args
                                                )
                                                .is_ok()
                                                {
                                                    // Complete JSON in one chunk (Janus style)
                                                    entry.arguments = args.to_string();
                                                    seen_tool_args.insert(idx);
                                                } else {
                                                    // Partial JSON (standard OpenAI streaming)
                                                    entry.arguments.push_str(args);
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            // Check finish reason — break.
                            // Janus may not send [DONE] sentinel after finish_reason,
                            // which would block until TCP timeout (~120s).
                            if let Some(ref reason) = choice.finish_reason {
                                debug!(
                                    finish_reason = ?reason,
                                    text_chunks,
                                    chunk_count,
                                    "stream finished"
                                );
                                last_finish_reason = Some(serde_json::to_value(reason)
                                    .ok()
                                    .and_then(|v| v.as_str().map(String::from))
                                    .unwrap_or_else(|| format!("{:?}", reason).to_lowercase()));
                                finished = true;
                                break 'outer;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        if text_chunks == 0 && tool_calls.is_empty() {
            warn!(
                finished,
                chunk_count, "stream completed with no text and no tool calls"
            );
        }

        // Emit accumulated tool calls (fallback for Janus single-chunk tool calls)
        for tc in tool_calls.values() {
            if !tc.id.is_empty() && !tc.name.is_empty() && !emitted_tool_calls.contains(&tc.id) {
                emitted_tool_calls.insert(tc.id.clone());
                let input: serde_json::Value = serde_json::from_str(&tc.arguments)
                    .unwrap_or(serde_json::Value::Object(Default::default()));
                let _ = tx
                    .send(StreamEvent::tool_call(ToolCall {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        input,
                    }))
                    .await;
            }
        }

        let mut done_event = match last_finish_reason {
            Some(reason) => StreamEvent::done_with_reason(reason),
            None => StreamEvent::done(),
        };
        done_event.provider_metadata = last_provider_metadata;
        let _ = tx.send(done_event).await;
    }
}

impl ConnectionResetter for OpenAIProvider {
    fn reset_connections(&self) {
        let mut lock = self.http_client.write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *lock = reqwest::Client::new();
        info!(provider = %self.provider_id, "reset HTTP connections");
    }
}

#[async_trait]
impl Provider for OpenAIProvider {
    fn id(&self) -> &str {
        &self.provider_id
    }

    async fn stream(&self, req: &ChatRequest) -> Result<EventReceiver, ProviderError> {
        let messages = self.build_messages(req);

        let model = if req.model.is_empty() {
            &self.model
        } else {
            &req.model
        };

        // Build tools
        let tools: Option<Vec<ChatCompletionTools>> = if req.tools.is_empty() {
            None
        } else {
            Some(
                req.tools
                    .iter()
                    .map(|t| {
                        ChatCompletionTools::Function(ChatCompletionTool {
                            function: FunctionObject {
                                name: t.name.clone(),
                                description: Some(t.description.clone()),
                                parameters: Some(t.input_schema.clone()),
                                strict: None,
                            },
                        })
                    })
                    .collect(),
            )
        };

        let api_req = CreateChatCompletionRequest {
            model: model.to_string(),
            messages,
            stream: Some(true),
            max_completion_tokens: if req.max_tokens > 0 {
                Some(req.max_tokens as u32)
            } else {
                None
            },
            temperature: if req.temperature > 0.0 {
                Some(req.temperature as f32)
            } else {
                None
            },
            tools,
            ..Default::default()
        };

        info!(
            model = model,
            messages = api_req.messages.len(),
            tools = req.tools.len(),
            "sending OpenAI request"
        );

        // Serialize request, injecting metadata for Janus tool stickiness
        let mut body_val = serde_json::to_value(&api_req)
            .map_err(|e| ProviderError::Request(format!("serialize error: {e}")))?;
        if let Some(ref meta) = req.metadata {
            if let serde_json::Value::Object(ref mut map) = body_val {
                map.insert("metadata".to_string(), serde_json::to_value(meta).unwrap());
            }
        }

        // Debug: log the full request body on first few requests to diagnose Janus errors
        if let Ok(body_json) = serde_json::to_string(&body_val) {
            debug!(body = %body_json, "OpenAI request body");
        }

        let url = format!("{}/chat/completions", self.base_url);
        let mut headers = reqwest::header::HeaderMap::new();
        if !self.api_key.is_empty() {
            headers.insert(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.api_key)
                    .parse()
                    .expect("valid auth header"),
            );
        }
        if let Some(ref bot_id) = self.bot_id {
            headers.insert(
                reqwest::header::HeaderName::from_static("x-bot-id"),
                bot_id.parse().expect("valid X-Bot-ID header"),
            );
        }
        if let Some(ref lane) = self.lane {
            if let Ok(val) = lane.parse() {
                headers.insert(
                    reqwest::header::HeaderName::from_static("x-lane"),
                    val,
                );
            }
        }

        let client = self.http_client.read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let response = client
            .post(&url)
            .headers(headers)
            .json(&body_val)
            .send()
            .await
            .map_err(|e| ProviderError::Request(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            warn!(
                status = status.as_u16(),
                body = %body,
                url = %url,
                model = model,
                "provider HTTP error"
            );
            return Err(map_http_error(status.as_u16(), &body));
        }

        let (tx, rx) = mpsc::channel(100);

        // Extract rate limit metadata from response headers
        let resp_headers = response.headers();
        let remaining_requests = resp_headers
            .get("x-ratelimit-remaining-requests")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let remaining_tokens = resp_headers
            .get("x-ratelimit-remaining-tokens")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let reset_after = resp_headers
            .get("x-ratelimit-reset-requests")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<f64>().ok());

        // Janus session rate limit headers
        let session_limit = resp_headers
            .get("x-ratelimit-session-limit-tokens")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let session_remaining = resp_headers
            .get("x-ratelimit-session-remaining-tokens")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let session_reset = resp_headers
            .get("x-ratelimit-session-reset")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.to_string());

        // Janus weekly rate limit headers
        let weekly_limit = resp_headers
            .get("x-ratelimit-weekly-limit-tokens")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let weekly_remaining = resp_headers
            .get("x-ratelimit-weekly-remaining-tokens")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let weekly_reset = resp_headers
            .get("x-ratelimit-weekly-reset")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.to_string());

        // Use session remaining if available (tighter constraint), else standard
        let effective_remaining = session_remaining.or(remaining_tokens);

        let has_any = remaining_requests.is_some()
            || effective_remaining.is_some()
            || weekly_remaining.is_some()
            || session_limit.is_some()
            || weekly_limit.is_some();

        if has_any {
            let _ = tx
                .send(StreamEvent::rate_limit_info(RateLimitMeta {
                    remaining_requests,
                    remaining_tokens: effective_remaining,
                    reset_after_secs: reset_after,
                    retry_after_secs: None,
                    session_limit_tokens: session_limit,
                    session_remaining_tokens: session_remaining,
                    session_reset_at: session_reset,
                    weekly_limit_tokens: weekly_limit,
                    weekly_remaining_tokens: weekly_remaining,
                    weekly_reset_at: weekly_reset,
                }))
                .await;
        }

        tokio::spawn(Self::handle_stream(response, tx));

        Ok(rx)
    }
}

/// Map HTTP error status + body to our ProviderError type.
fn map_http_error(status: u16, body: &str) -> ProviderError {
    // Try to parse as OpenAI error JSON: {"error":{"message":"...", "code":"..."}}
    let (msg, code) = if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        let err = &v["error"];
        (
            err["message"].as_str().unwrap_or(body).to_string(),
            err["code"].as_str().unwrap_or("").to_string(),
        )
    } else {
        (body.to_string(), String::new())
    };

    match status {
        429 => ProviderError::RateLimit,
        401 => ProviderError::Auth(msg),
        _ => {
            // Rate limit by code/message
            if code == "rate_limit_exceeded"
                || msg.contains("rate limit")
                || msg.contains("429")
            {
                return ProviderError::RateLimit;
            }
            // Auth
            if code == "invalid_api_key"
                || code == "authentication_error"
                || msg.contains("Incorrect API key")
                || msg.contains("unauthorized")
            {
                return ProviderError::Auth(msg);
            }
            // Context overflow
            if code == "context_length_exceeded"
                || (msg.contains("context") && msg.contains("exceeded"))
                || msg.contains("maximum context length")
            {
                return ProviderError::ContextOverflow;
            }

            ProviderError::Api {
                code,
                message: msg,
                retryable: status >= 500,
            }
        }
    }
}

// --- Helper types (kept for history deserialization and tool accumulation) ---

struct AccumulatedToolCall {
    id: String,
    name: String,
    arguments: String,
}

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
