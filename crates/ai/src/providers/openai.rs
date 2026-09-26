use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls,
    ChatCompletionRequestAssistantMessage, ChatCompletionRequestAssistantMessageContent,
    ChatCompletionRequestMessage, ChatCompletionRequestMessageContentPartImage,
    ChatCompletionRequestMessageContentPartText, ChatCompletionRequestSystemMessage,
    ChatCompletionRequestSystemMessageContent, ChatCompletionRequestToolMessage,
    ChatCompletionRequestToolMessageContent, ChatCompletionRequestUserMessage,
    ChatCompletionRequestUserMessageContent, ChatCompletionRequestUserMessageContentPart,
    ChatCompletionStreamOptions, ChatCompletionTool, ChatCompletionTools,
    CreateChatCompletionRequest, CreateChatCompletionStreamResponse, FunctionCall, FunctionObject,
    ImageUrl,
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
    api_key: ApiKey,
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
    pub fn new(api_key: impl Into<ApiKey>, model: String) -> Self {
        Self {
            api_key: api_key.into(),
            model,
            base_url: "https://api.openai.com/v1".to_string(),
            provider_id: "openai".to_string(),
            bot_id: None,
            lane: None,
            http_client: RwLock::new(crate::http::streaming_client()),
        }
    }

    /// Create with a custom base URL for OpenAI-compatible APIs.
    pub fn with_base_url(api_key: impl Into<ApiKey>, model: String, base_url: String) -> Self {
        Self {
            api_key: api_key.into(),
            model,
            base_url,
            provider_id: "openai".to_string(),
            bot_id: None,
            lane: None,
            http_client: RwLock::new(crate::http::streaming_client()),
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

        // Add system message(s).
        // When cache_breakpoints are provided, split the system prompt at those
        // offsets into separate system messages. Proxies like Janus automatically
        // wrap each system message with `cache_control: ephemeral`, so sending
        // the stable prefix as its own message enables provider-side prefix
        // caching (DashScope, OpenAI, etc.). The mutable tail changes per turn
        // without busting the cached prefix.
        if !req.system.is_empty() {
            if req.cache_breakpoints.is_empty() {
                messages.push(ChatCompletionRequestMessage::System(
                    ChatCompletionRequestSystemMessage {
                        content: ChatCompletionRequestSystemMessageContent::Text(
                            req.system.clone(),
                        ),
                        name: None,
                    },
                ));
            } else {
                let prompt = &req.system;
                let prompt_len = prompt.len();
                let mut cursor = 0usize;
                for &bp in &req.cache_breakpoints {
                    let bp = bp.min(prompt_len);
                    if bp <= cursor {
                        continue;
                    }
                    messages.push(ChatCompletionRequestMessage::System(
                        ChatCompletionRequestSystemMessage {
                            content: ChatCompletionRequestSystemMessageContent::Text(
                                prompt[cursor..bp].to_string(),
                            ),
                            name: None,
                        },
                    ));
                    cursor = bp;
                }
                // Remaining tail (mutable portion)
                if cursor < prompt_len {
                    messages.push(ChatCompletionRequestMessage::System(
                        ChatCompletionRequestSystemMessage {
                            content: ChatCompletionRequestSystemMessageContent::Text(
                                prompt[cursor..].to_string(),
                            ),
                            name: None,
                        },
                    ));
                }
            }
        }

        for msg in &req.messages {
            match msg.role.as_str() {
                "user" => {
                    if msg.content.is_empty() && msg.images.is_none() {
                        continue;
                    }
                    if let Some(ref images) = msg.images {
                        let mut parts: Vec<ChatCompletionRequestUserMessageContentPart> =
                            Vec::new();
                        if !msg.content.is_empty() {
                            parts.push(ChatCompletionRequestUserMessageContentPart::Text(
                                ChatCompletionRequestMessageContentPartText {
                                    text: msg.content.clone(),
                                },
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
                                        tool_call_id: r.tool_call_id.clone(),
                                    },
                                ));
                                // The OpenAI tool role is text-only, so a tool
                                // result's screenshot rides a follow-up user
                                // message — the standard OpenAI-compat pattern.
                                // Without this, every screenshot on an
                                // OpenAI-shaped provider (incl. Janus) was
                                // silently dropped and the model went blind.
                                if let Some(ref img) = r.image_url {
                                    if let Some((media_type, data)) =
                                        crate::types::image_source_to_base64(img)
                                    {
                                        let url =
                                            format!("data:{};base64,{}", media_type, data);
                                        messages.push(ChatCompletionRequestMessage::User(
                                            ChatCompletionRequestUserMessage {
                                                content:
                                                    ChatCompletionRequestUserMessageContent::Array(vec![
                                                        ChatCompletionRequestUserMessageContentPart::Text(
                                                            ChatCompletionRequestMessageContentPartText {
                                                                text: format!(
                                                                    "[Image returned by tool call {}]",
                                                                    r.tool_call_id
                                                                ),
                                                            },
                                                        ),
                                                        ChatCompletionRequestUserMessageContentPart::ImageUrl(
                                                            ChatCompletionRequestMessageContentPartImage {
                                                                image_url: ImageUrl { url, detail: None },
                                                            },
                                                        ),
                                                    ]),
                                                name: None,
                                            },
                                        ));
                                    }
                                }
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

        // Tool calls assembled from their chunks, in arrival order.
        let mut tool_calls = ToolCallAccumulator::default();

        let mut text_chunks = 0u32;
        let mut chunk_count = 0u32;
        let mut finished = false;
        // True once we've already surfaced an error to the caller, so the
        // post-loop truncation guard doesn't double-report.
        let mut errored = false;
        let mut last_finish_reason: Option<String> = None;
        let mut last_provider_metadata: Option<HashMap<String, String>> = None;
        // Latest usage seen on the stream. Usage counters are CUMULATIVE:
        // vanilla OpenAI sends one usage chunk at the end, but proxies (Janus)
        // may attach the running totals to every chunk. Emitting an event per
        // chunk made consumers that sum per-event (workflow engine, chat run
        // totals) count a ~200-token turn as tens of thousands — so we hold
        // the latest values and emit exactly ONE Usage event after the loop.
        let mut latest_usage: Option<UsageInfo> = None;

        'outer: while let Some(result) = byte_stream.next().await {
            let bytes = match result {
                Ok(b) => b,
                Err(e) => {
                    warn!(error = %e, "stream read error");
                    let _ = tx
                        .send(StreamEvent::error(format!("stream read error: {e}")))
                        .await;
                    errored = true;
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
                                let code =
                                    err_obj.get("code").and_then(|c| c.as_str()).unwrap_or("");
                                let err_type =
                                    err_obj.get("type").and_then(|t| t.as_str()).unwrap_or("");
                                warn!(
                                    error = msg,
                                    code = code,
                                    error_type = err_type,
                                    raw = %err_obj,
                                    "provider returned error in SSE stream"
                                );
                                // A rate limit inside the stream (Janus sends
                                // 200 + SSE headers before its per-user check
                                // runs) carries how long to wait as
                                // `retry_after`, a Go duration. Hand it to the
                                // runner on the rate-limit event it already
                                // reads, ahead of the error that ends the turn.
                                if err_type == "rate_limit_error" {
                                    let wait = err_obj
                                        .get("retry_after")
                                        .and_then(|v| match v {
                                            serde_json::Value::String(s) => go_duration_secs(s),
                                            serde_json::Value::Number(n) => n.as_u64(),
                                            _ => None,
                                        });
                                    if wait.is_some() {
                                        let _ = tx
                                            .send(StreamEvent::rate_limit_info(RateLimitMeta {
                                                retry_after_secs: wait,
                                                ..Default::default()
                                            }))
                                            .await;
                                    }
                                }
                                let _ = tx.send(StreamEvent::error(msg.to_string())).await;
                                finished = true;
                                errored = true;
                                break 'outer;
                            }
                        }

                        // Extract provider_metadata from Janus for tool stickiness
                        if let Some(ref val) = raw_val {
                            if let Some(pm) = val.get("provider_metadata") {
                                if let Ok(meta) =
                                    serde_json::from_value::<HashMap<String, String>>(pm.clone())
                                {
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

                            // Assemble tool calls (see `ToolCallAccumulator`).
                            if let Some(ref tcs) = choice.delta.tool_calls {
                                for tc in tcs {
                                    let func = tc.function.as_ref();
                                    tool_calls.absorb(
                                        tc.index,
                                        tc.id.as_deref(),
                                        func.and_then(|f| f.name.as_deref()),
                                        func.and_then(|f| f.arguments.as_deref()),
                                    );
                                }
                                // Each call goes to the runner as soon as its
                                // input is complete, so safe calls start while
                                // the model is still writing the rest.
                                for tc in tool_calls.take_complete() {
                                    let _ = tx.send(tool_call_event(tc)).await;
                                }
                            }

                            // Check finish reason — mark finished but don't break yet.
                            // Continue processing remaining lines in buffer to catch
                            // the usage chunk (include_usage sends it as a separate line
                            // often in the same TCP packet).
                            if let Some(ref reason) = choice.finish_reason {
                                debug!(
                                    finish_reason = ?reason,
                                    text_chunks,
                                    chunk_count,
                                    "stream finished"
                                );
                                last_finish_reason = Some(
                                    serde_json::to_value(reason)
                                        .ok()
                                        .and_then(|v| v.as_str().map(String::from))
                                        .unwrap_or_else(|| format!("{:?}", reason).to_lowercase()),
                                );
                                finished = true;
                            }
                        }

                        // Capture usage (include_usage sends it on the final
                        // chunk; Janus may send running totals on every chunk).
                        // Latest-wins here; ONE event is emitted after the loop.
                        if let Some(ref usage) = response.usage {
                            let cached = usage
                                .prompt_tokens_details
                                .as_ref()
                                .and_then(|d| d.cached_tokens)
                                .unwrap_or(0) as i32;
                            // Janus adds `usage.cost_micro` (microdollars for the
                            // model it routed to). The typed chunk cannot carry
                            // it, so read it off the raw JSON beside it.
                            let cost_micro = serde_json::from_str::<serde_json::Value>(&data)
                                .ok()
                                .and_then(|v| v.pointer("/usage/cost_micro").and_then(|c| c.as_i64()));
                            latest_usage = Some(usage_from_openai(
                                usage.prompt_tokens as i32,
                                usage.completion_tokens as i32,
                                cached,
                                cost_micro,
                            ));
                        }

                        // Break after processing this chunk if we saw finish_reason.
                        // Janus may not send [DONE], so we break here to avoid
                        // hanging until TCP timeout (~120s).
                        if finished {
                            break 'outer;
                        }
                    }
                    _ => {}
                }
            }
        }

        // Truncation guard. A complete OpenAI-compatible stream always ends with
        // a chunk carrying finish_reason (and usually a [DONE] sentinel), both of
        // which set `finished`. If the byte stream reached EOF without either and
        // we didn't already surface an error, the connection was cut mid-response
        // — e.g. an edge/proxy idle timeout during a long tool-call generation.
        // Emitting a clean done() here would let the runner treat a truncated
        // (often tool-call-less) turn as finished. Surface a retryable error
        // instead so the runner's transient-retry path re-runs the turn.
        if !finished && !errored {
            warn!(
                chunk_count,
                text_chunks,
                tool_calls = tool_calls.len(),
                "stream truncated before completion (EOF with no finish_reason / [DONE])"
            );
            let _ = tx
                .send(StreamEvent::error(
                    "stream truncated before completion (unexpected EOF before finish_reason)"
                        .to_string(),
                ))
                .await;
            return;
        }

        if text_chunks == 0 && tool_calls.is_empty() {
            warn!(
                finished,
                chunk_count, "stream completed with no text and no tool calls"
            );
        }

        // The calls whose input never completed, in the order they arrived.
        for tc in tool_calls.finish() {
            let _ = tx.send(tool_call_event(tc)).await;
        }

        // The one Usage event for this stream — final cumulative totals.
        if let Some(usage) = latest_usage {
            let _ = tx.send(StreamEvent::usage(usage)).await;
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
        let mut lock = self
            .http_client
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *lock = crate::http::streaming_client();
        info!(provider = %self.provider_id, "reset HTTP connections");
    }
}

#[async_trait]
impl Provider for OpenAIProvider {
    fn id(&self) -> &str {
        &self.provider_id
    }

    fn supports_vision(&self) -> bool {
        true
    }

    fn supports_tool_result_images(&self) -> bool {
        // Tool-result screenshots ride a follow-up user message (the tool
        // role is text-only in the OpenAI schema) — see convert_messages.
        // Without this override every screenshot on an OpenAI-shaped
        // provider (incl. Janus, i.e. every cloud bot) detoured to the
        // blind sidecar.
        true
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
            stream_options: Some(ChatCompletionStreamOptions {
                include_usage: Some(true),
                include_obfuscation: None,
            }),
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
        // Map the cross-provider ToolChoice to OpenAI's `tool_choice` (Auto → omitted).
        let tool_choice_val = match &req.tool_choice {
            ToolChoice::Auto => None,
            ToolChoice::Any => Some(serde_json::json!("required")),
            ToolChoice::Tool(name) => {
                Some(serde_json::json!({"type": "function", "function": {"name": name}}))
            }
            ToolChoice::None => Some(serde_json::json!("none")),
        };
        if let Some(tc) = tool_choice_val {
            if let serde_json::Value::Object(ref mut map) = body_val {
                map.insert("tool_choice".to_string(), tc);
            }
        }

        // LLM payload breakdown — logged on every request so we can MEASURE where the
        // input tokens actually go (tool defs vs system vs conversation) instead of
        // assuming. Set NEBO_LLM_DUMP=<dir> to ALSO write the full request JSON there.
        {
            let dump = std::env::var("NEBO_LLM_DUMP").unwrap_or_default();
            let sz = |v: &serde_json::Value| serde_json::to_string(v).map(|s| s.len()).unwrap_or(0);
            let tools_chars = body_val.get("tools").map(&sz).unwrap_or(0);
            let msgs = body_val.get("messages");
            let msgs_chars = msgs.map(&sz).unwrap_or(0);
            let sys_chars = msgs
                .and_then(|m| m.as_array())
                .and_then(|a| {
                    a.iter()
                        .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("system"))
                })
                .map(&sz)
                .unwrap_or(0);
            let total_chars = sz(&body_val);
            let tool_count = body_val
                .get("tools")
                .and_then(|t| t.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            // Biggest tool defs first — the clearest signal for tool-def bloat.
            let top_tools = body_val
                .get("tools")
                .and_then(|t| t.as_array())
                .map(|arr| {
                    let mut s: Vec<(String, usize)> = arr
                        .iter()
                        .map(|t| {
                            let n = t
                                .pointer("/function/name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("?")
                                .to_string();
                            (n, serde_json::to_string(t).map(|x| x.len()).unwrap_or(0))
                        })
                        .collect();
                    s.sort_by(|a, b| b.1.cmp(&a.1));
                    s.into_iter()
                        .take(10)
                        .map(|(n, c)| format!("{n}={c}c"))
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            info!(
                target: "llm_dump",
                tool_count,
                tools_chars,
                tools_tok_est = tools_chars / 4,
                system_chars = sys_chars,
                system_tok_est = sys_chars / 4,
                messages_chars = msgs_chars,
                total_chars,
                total_tok_est = total_chars / 4,
                top_tools = %top_tools,
                "LLM request payload breakdown"
            );
            if dump != "1" && !dump.is_empty() {
                let dir = std::path::Path::new(&dump);
                let _ = std::fs::create_dir_all(dir);
                let fname = format!("llm-m{}-t{tool_count}-{total_chars}c.json", api_req.messages.len());
                if let Ok(pretty) = serde_json::to_string_pretty(&body_val) {
                    let _ = std::fs::write(dir.join(fname), pretty);
                }
            }
        }

        // Debug: log the full request body on first few requests to diagnose Janus errors
        if let Ok(body_json) = serde_json::to_string(&body_val) {
            debug!(body = %body_json, "OpenAI request body");
        }

        let url = format!("{}/chat/completions", self.base_url);
        let mut headers = reqwest::header::HeaderMap::new();
        let api_key = self.api_key.current();
        if !api_key.is_empty() {
            headers.insert(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", api_key)
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
                headers.insert(reqwest::header::HeaderName::from_static("x-lane"), val);
            }
        }
        // Purpose + trace headers — let Janus group usage by what the call was
        // for and attribute it to the agent/run/workflow/action/step behind it.
        headers.extend(req.trace.headers());

        let client = self
            .http_client
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let response = client
            .post(&url)
            .headers(headers)
            .json(&body_val)
            .send()
            .await
            .map_err(|e| ProviderError::Request(format!("{} (model: {} · {})", e, model, url)))?;

        if !response.status().is_success() {
            let status = response.status();
            let retry_after = crate::http::retry_after_secs(response.headers());
            let body = response.text().await.unwrap_or_default();
            warn!(
                status = status.as_u16(),
                body = %body,
                url = %url,
                model = model,
                "provider HTTP error"
            );
            return Err(map_http_error(status.as_u16(), &body, &model, &url, retry_after));
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

        // Janus session rate limit headers (try -credits first, fall back to -tokens for rollout)
        let session_limit = resp_headers
            .get("x-ratelimit-session-limit-credits")
            .or_else(|| resp_headers.get("x-ratelimit-session-limit-tokens"))
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let session_remaining = resp_headers
            .get("x-ratelimit-session-remaining-credits")
            .or_else(|| resp_headers.get("x-ratelimit-session-remaining-tokens"))
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let session_reset = resp_headers
            .get("x-ratelimit-session-reset")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.to_string());

        // Janus weekly rate limit headers (try -credits first, fall back to -tokens)
        let weekly_limit = resp_headers
            .get("x-ratelimit-weekly-limit-credits")
            .or_else(|| resp_headers.get("x-ratelimit-weekly-limit-tokens"))
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let weekly_remaining = resp_headers
            .get("x-ratelimit-weekly-remaining-credits")
            .or_else(|| resp_headers.get("x-ratelimit-weekly-remaining-tokens"))
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let weekly_reset = resp_headers
            .get("x-ratelimit-weekly-reset")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.to_string());

        // Janus budget pool headers
        let budget_free = resp_headers
            .get("x-budget-free-available")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let budget_gift = resp_headers
            .get("x-budget-gift-available")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let budget_credits_cents = resp_headers
            .get("x-budget-credits-cents")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let budget_active_pool = resp_headers
            .get("x-budget-active-pool")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.to_string());

        // Use session remaining if available (tighter constraint), else standard
        let effective_remaining = session_remaining.or(remaining_tokens);

        let has_any = remaining_requests.is_some()
            || effective_remaining.is_some()
            || weekly_remaining.is_some()
            || session_limit.is_some()
            || weekly_limit.is_some()
            || budget_free.is_some()
            || budget_gift.is_some()
            || budget_active_pool.is_some();

        if has_any {
            let _ = tx
                .send(StreamEvent::rate_limit_info(RateLimitMeta {
                    remaining_requests,
                    remaining_tokens: effective_remaining,
                    reset_after_secs: reset_after,
                    retry_after_secs: None,
                    session_limit_credits: session_limit,
                    session_remaining_credits: session_remaining,
                    session_reset_at: session_reset,
                    weekly_limit_credits: weekly_limit,
                    weekly_remaining_credits: weekly_remaining,
                    weekly_reset_at: weekly_reset,
                    budget_free_available: budget_free,
                    budget_gift_available: budget_gift,
                    budget_credits_cents,
                    budget_active_pool,
                }))
                .await;
        }

        tokio::spawn(Self::handle_stream(response, tx));

        Ok(rx)
    }
}

/// Whole seconds (rounded up) in a Go `time.Duration` string such as `5s`,
/// `1.5s`, `5h0m0s` or `250ms` — the form Janus writes `retry_after` in.
fn go_duration_secs(s: &str) -> Option<u64> {
    let mut total = 0f64;
    let mut num = String::new();
    let mut unit = String::new();
    let flush = |num: &mut String, unit: &mut String, total: &mut f64| -> Option<()> {
        let n: f64 = num.parse().ok()?;
        let per_sec = match unit.as_str() {
            "h" => 3600.0,
            "m" => 60.0,
            "s" => 1.0,
            "ms" => 0.001,
            "us" | "µs" => 0.000_001,
            "ns" => 0.000_000_001,
            _ => return None,
        };
        *total += n * per_sec;
        num.clear();
        unit.clear();
        Some(())
    };
    for c in s.trim().chars() {
        if c.is_ascii_digit() || c == '.' {
            if !unit.is_empty() {
                flush(&mut num, &mut unit, &mut total)?;
            }
            num.push(c);
        } else {
            if num.is_empty() {
                return None;
            }
            unit.push(c);
        }
    }
    if num.is_empty() && unit.is_empty() && total == 0.0 {
        return None;
    }
    if !num.is_empty() {
        flush(&mut num, &mut unit, &mut total)?;
    }
    Some(total.ceil() as u64)
}

/// Map HTTP error status + body to our ProviderError type.
fn map_http_error(
    status: u16,
    body: &str,
    model: &str,
    url: &str,
    retry_after_secs: Option<u64>,
) -> ProviderError {
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
        // Janus quota denial (all spend pools empty) — terminal, not a transient
        // rate limit: retrying can never succeed until the user adds balance.
        429 if code == "USAGE_LIMIT_EXCEEDED" => ProviderError::Api {
            code,
            message: format!("USAGE_LIMIT_EXCEEDED: {msg}"),
            retryable: false,
        },
        429 => ProviderError::RateLimit { retry_after_secs },
        401 => ProviderError::Auth(msg),
        _ => {
            // Rate limit by code/message
            if code == "rate_limit_exceeded" || msg.contains("rate limit") || msg.contains("429") {
                return ProviderError::RateLimit { retry_after_secs };
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

            // Include HTTP status, model, and endpoint for diagnostics
            let detailed = format!(
                "{} (HTTP {} · model: {} · {})",
                msg, status, model, url
            );
            ProviderError::Api {
                code,
                message: detailed,
                retryable: status >= 500,
            }
        }
    }
}

/// A finished call as the runner's event. Unparseable arguments = the stream
/// was cut off mid-payload. An empty-object fallback silently forwarded the
/// lie; wrap the raw text instead so the registry's truncation corrective can
/// name the cutoff and teach chunked writes (same shape the gateway's salvage
/// uses — ONE downstream detector).
fn tool_call_event(tc: AccumulatedToolCall) -> StreamEvent {
    let input: serde_json::Value = serde_json::from_str(&tc.arguments)
        .unwrap_or_else(|_| serde_json::json!({ "_raw": tc.arguments }));
    StreamEvent::tool_call(ToolCall {
        id: tc.id,
        name: tc.name,
        input,
    })
}

// --- Helper types (kept for history deserialization and tool accumulation) ---

#[derive(Clone)]
struct AccumulatedToolCall {
    index: u32,
    id: String,
    name: String,
    arguments: String,
    /// The arguments arrived whole in one chunk; repeats of it are ignored.
    arguments_whole: bool,
    /// Already handed to the runner by `take_complete`.
    emitted: bool,
}

impl AccumulatedToolCall {
    /// An id, a name and arguments that parse as a whole JSON object: nothing
    /// a later chunk adds can change the input.
    fn is_complete(&self) -> bool {
        !self.id.is_empty()
            && !self.name.is_empty()
            && serde_json::from_str::<serde_json::Value>(&self.arguments)
                .is_ok_and(|v| v.is_object())
    }
}

/// A streamed response's tool calls, assembled from their chunks in the
/// order they arrived.
///
/// Two streaming shapes reach this parser. Standard OpenAI sends a call's id
/// and name on its first chunk and its arguments as fragments on later
/// chunks that carry only the call's `index`. Janus sends each call whole
/// (id, name, complete arguments), may repeat it, and numbers every call
/// `index: 0`. So a chunk with an id not seen yet is a new call even at an
/// index already in use, and a chunk without one continues the latest call
/// at its index. Keying on the index alone merged every Janus call into the
/// first: 3,988 of 3,988 stored assistant messages carried one call.
#[derive(Default)]
struct ToolCallAccumulator {
    calls: Vec<AccumulatedToolCall>,
}

impl ToolCallAccumulator {
    fn absorb(&mut self, index: u32, id: Option<&str>, name: Option<&str>, arguments: Option<&str>) {
        let id = id.filter(|s| !s.is_empty());
        let latest_at_index = self.calls.iter().rposition(|c| c.index == index);
        let pos = match id {
            Some(id) => self
                .calls
                .iter()
                .position(|c| c.id == id)
                // The call's id arrived after an id-less first chunk.
                .or_else(|| latest_at_index.filter(|&i| self.calls[i].id.is_empty())),
            None => latest_at_index,
        };
        let pos = pos.unwrap_or_else(|| {
            self.calls.push(AccumulatedToolCall {
                index,
                id: String::new(),
                name: String::new(),
                arguments: String::new(),
                arguments_whole: false,
                emitted: false,
            });
            self.calls.len() - 1
        });
        let call = &mut self.calls[pos];
        if let Some(id) = id {
            call.id = id.to_string();
        }
        if let Some(name) = name.filter(|n| !n.is_empty())
            && call.name.is_empty()
        {
            call.name = name.to_string();
        }
        if let Some(args) = arguments.filter(|a| !a.is_empty())
            && !call.arguments_whole
        {
            if call.arguments.is_empty() && serde_json::from_str::<serde_json::Value>(args).is_ok() {
                call.arguments = args.to_string();
                call.arguments_whole = true;
            } else {
                call.arguments.push_str(args);
            }
        }
    }

    /// The calls whose input is now complete and not yet handed over, in
    /// arrival order: a call waits while an earlier one is still open, so the
    /// runner receives them in the order the model wrote them.
    fn take_complete(&mut self) -> Vec<AccumulatedToolCall> {
        let mut out = Vec::new();
        for call in self.calls.iter_mut().filter(|c| !c.emitted) {
            if !call.is_complete() {
                break;
            }
            call.emitted = true;
            out.push(call.clone());
        }
        out
    }

    /// The calls not yet handed over that have an id and a name, in arrival
    /// order.
    fn finish(self) -> Vec<AccumulatedToolCall> {
        self.calls
            .into_iter()
            .filter(|c| !c.emitted && !c.id.is_empty() && !c.name.is_empty())
            .collect()
    }

    fn len(&self) -> usize {
        self.calls.len()
    }

    fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }
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

/// OpenAI-protocol usage into Nebo's `UsageInfo`.
///
/// OpenAI counts cached tokens as a SUBSET of `prompt_tokens`; Anthropic (and
/// everything downstream in Nebo: cost math, the usage panel, the runner's
/// context calibration that sums input + cache_read + cache_creation) treats
/// them as DISJOINT. Convert at the boundary, once, or a 74% cache hit reads
/// as a context 74% larger than it is and compaction fires early.
fn usage_from_openai(prompt_tokens: i32, completion_tokens: i32, cached_tokens: i32, cost_micro: Option<i64>) -> UsageInfo {
    let cached = cached_tokens.clamp(0, prompt_tokens.max(0));
    UsageInfo {
        cost_microdollars: cost_micro.filter(|c| *c > 0),
        input_tokens: prompt_tokens - cached,
        output_tokens: completion_tokens,
        cache_read_input_tokens: cached,
        ..Default::default()
    }
}

#[cfg(test)]
mod usage_semantics_tests {
    use super::*;

    /// A provider reporting 183,008 prompt tokens of which 136,064 were cached
    /// sent 46,944 uncached tokens, not 319,072.
    #[test]
    fn openai_cached_tokens_are_a_subset_of_prompt_tokens() {
        let u = usage_from_openai(183_008, 20, 136_064, None);
        assert_eq!(u.input_tokens, 46_944);
        assert_eq!(u.cache_read_input_tokens, 136_064);
        assert_eq!(u.input_tokens + u.cache_read_input_tokens, 183_008, "the sum is the prompt");
        // Providers without a breakdown are unchanged.
        let u = usage_from_openai(100, 5, 0, None);
        assert_eq!((u.input_tokens, u.cache_read_input_tokens), (100, 0));
        // A malformed breakdown never goes negative.
        let u = usage_from_openai(10, 1, 50, None);
        assert_eq!((u.input_tokens, u.cache_read_input_tokens), (0, 10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    // Serves one SSE response: a single content chunk, then closes the
    // connection WITHOUT a finish_reason or [DONE] sentinel — exactly what an
    // edge/proxy idle-timeout cut looks like (a graceful close → clean EOF).
    // handle_stream must surface a retryable error, not a clean Done.
    #[tokio::test]
    async fn truncated_stream_surfaces_retryable_error() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 2048];
            let _ = sock.read(&mut buf).await; // drain the request
            let chunk = "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"On it\"},\"finish_reason\":null}]}\n\n";
            // Close-delimited body (no Content-Length, Connection: close): the
            // connection close IS the end of the stream → clean EOF for the client.
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{chunk}"
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
            sock.flush().await.unwrap();
            // Drop the socket — no finish_reason, no [DONE].
        });

        let response = reqwest::get(format!("http://{addr}/")).await.unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        OpenAIProvider::handle_stream(response, tx).await;
        server.await.unwrap();

        let mut saw_error = false;
        let mut saw_done = false;
        while let Ok(ev) = rx.try_recv() {
            match ev.event_type {
                StreamEventType::Error => saw_error = true,
                StreamEventType::Done => saw_done = true,
                _ => {}
            }
        }
        assert!(saw_error, "truncated stream must emit a retryable Error event");
        assert!(!saw_done, "truncated stream must NOT emit a clean Done");
    }

    fn calls(acc: ToolCallAccumulator) -> Vec<(String, String, String)> {
        acc.finish().into_iter().map(|c| (c.id, c.name, c.arguments)).collect()
    }

    /// Janus sends each call whole, may repeat a chunk, and numbers every
    /// call index 0: every call survives, once each, in arrival order.
    #[test]
    fn janus_calls_at_one_index_stay_separate() {
        let mut acc = ToolCallAccumulator::default();
        acc.absorb(0, Some("call_a"), Some("read"), Some(r#"{"path":"/a"}"#));
        acc.absorb(0, Some("call_a"), Some("read"), Some(r#"{"path":"/a"}"#));
        acc.absorb(0, Some("call_b"), Some("grep"), Some(r#"{"q":"x"}"#));
        acc.absorb(0, Some("call_c"), Some("read"), Some(r#"{"path":"/c"}"#));
        assert_eq!(
            calls(acc),
            vec![
                ("call_a".into(), "read".into(), r#"{"path":"/a"}"#.into()),
                ("call_b".into(), "grep".into(), r#"{"q":"x"}"#.into()),
                ("call_c".into(), "read".into(), r#"{"path":"/c"}"#.into()),
            ]
        );
    }

    /// Janus numbering each whole call by its real position (0, 1, …),
    /// repeats included: every call survives, once each, in order.
    #[test]
    fn janus_calls_at_their_own_index_stay_separate() {
        let mut acc = ToolCallAccumulator::default();
        acc.absorb(0, Some("call_a"), Some("read"), Some(r#"{"path":"/a"}"#));
        acc.absorb(1, Some("call_b"), Some("grep"), Some(r#"{"q":"x"}"#));
        acc.absorb(1, Some("call_b"), Some("grep"), Some(r#"{"q":"x"}"#));
        assert_eq!(
            calls(acc),
            vec![
                ("call_a".into(), "read".into(), r#"{"path":"/a"}"#.into()),
                ("call_b".into(), "grep".into(), r#"{"q":"x"}"#.into()),
            ]
        );
    }

    /// Standard OpenAI: id and name on a call's first chunk, argument
    /// fragments on id-less chunks carrying its index, calls interleaved.
    #[test]
    fn openai_fragments_join_their_own_call() {
        let mut acc = ToolCallAccumulator::default();
        acc.absorb(0, Some("call_a"), Some("read"), Some(""));
        acc.absorb(1, Some("call_b"), Some("grep"), Some(""));
        acc.absorb(0, None, None, Some(r#"{"pa"#));
        acc.absorb(1, None, None, Some(r#"{"q":"#));
        acc.absorb(0, None, None, Some(r#"th":"/a"}"#));
        acc.absorb(1, None, None, Some("1"));
        acc.absorb(1, None, None, Some("}"));
        assert_eq!(
            calls(acc),
            vec![
                ("call_a".into(), "read".into(), r#"{"path":"/a"}"#.into()),
                ("call_b".into(), "grep".into(), r#"{"q":1}"#.into()),
            ]
        );
    }

    /// A call missing its id or name never reaches the runner.
    #[test]
    fn incomplete_calls_are_dropped() {
        let mut acc = ToolCallAccumulator::default();
        acc.absorb(0, None, Some("read"), Some("{}"));
        acc.absorb(1, Some("call_b"), None, Some("{}"));
        assert!(calls(acc).is_empty());
    }

    /// A call is handed over once its arguments form a whole object, and not
    /// again; one still open holds back the calls after it.
    #[test]
    fn a_call_is_taken_once_its_input_is_complete() {
        let mut acc = ToolCallAccumulator::default();
        acc.absorb(0, Some("call_a"), Some("read"), Some(""));
        acc.absorb(0, None, None, Some(r#"{"pa"#));
        assert!(acc.take_complete().is_empty());
        acc.absorb(0, None, None, Some(r#"th":"/a"}"#));
        let taken: Vec<_> = acc.take_complete().into_iter().map(|c| c.id).collect();
        assert_eq!(taken, vec!["call_a"]);
        acc.absorb(0, Some("call_a"), Some("read"), Some(r#"{"path":"/a"}"#));
        assert!(acc.take_complete().is_empty(), "a repeat is not a second call");
        acc.absorb(1, Some("call_b"), Some("grep"), Some(r#"{"q":"#));
        acc.absorb(2, Some("call_c"), Some("read"), Some(r#"{"path":"/c"}"#));
        assert!(acc.take_complete().is_empty(), "call_c waits for call_b");
        acc.absorb(1, None, None, Some("1}"));
        let taken: Vec<_> = acc.take_complete().into_iter().map(|c| c.id).collect();
        assert_eq!(taken, vec!["call_b", "call_c"]);
        assert!(calls(acc).is_empty(), "nothing is handed over twice");
    }

    /// Over the wire, a streamed multi-call response: the first call reaches
    /// the runner while the stream is still open, before the model has
    /// finished writing the second.
    #[tokio::test]
    async fn each_call_is_emitted_while_the_stream_is_still_open() {
        let chunk = |tc: &str| {
            format!(
                "data: {{\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"m\",\"choices\":[{{\"index\":0,\"delta\":{{\"tool_calls\":[{tc}]}},\"finish_reason\":null}}]}}\n\n"
            )
        };
        let head = [
            chunk(r#"{"index":0,"id":"call_a","type":"function","function":{"name":"read","arguments":""}}"#),
            chunk(r#"{"index":0,"function":{"arguments":"{\"path\":"}}"#),
            chunk(r#"{"index":0,"function":{"arguments":"\"/a\"}"}}"#),
            chunk(r#"{"index":1,"id":"call_b","type":"function","function":{"name":"grep","arguments":"{\"q\":"}}"#),
        ]
        .concat();
        let tail = [
            chunk(r#"{"index":1,"function":{"arguments":"\"x\"}"}}"#),
            "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n".to_string(),
        ]
        .concat();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 2048];
            let _ = sock.read(&mut buf).await;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{head}"
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
            sock.flush().await.unwrap();
            // The model is still writing call_b until the test releases it.
            let _ = tokio::time::timeout(std::time::Duration::from_secs(10), release_rx).await;
            sock.write_all(tail.as_bytes()).await.unwrap();
            sock.flush().await.unwrap();
        });
        let response = reqwest::get(format!("http://{addr}/")).await.unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        let reader = tokio::spawn(OpenAIProvider::handle_stream(response, tx));

        let first = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while let Some(ev) = rx.recv().await {
                if let Some(tc) = ev.tool_call {
                    return Some(tc);
                }
            }
            None
        })
        .await
        .expect("call_a must arrive while the stream is still open")
        .expect("a tool call");
        assert_eq!((first.id.as_str(), first.input.clone()), ("call_a", serde_json::json!({"path": "/a"})));

        release_tx.send(()).unwrap();
        reader.await.unwrap();
        server.await.unwrap();
        let mut rest = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let Some(tc) = ev.tool_call {
                rest.push((tc.id, tc.input));
            }
        }
        assert_eq!(rest, vec![("call_b".to_string(), serde_json::json!({"q": "x"}))]);
    }

    /// End to end over the wire: two Janus-shaped calls in one response are
    /// two tool-call events.
    #[tokio::test]
    async fn stream_with_two_index_zero_calls_emits_both() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 2048];
            let _ = sock.read(&mut buf).await;
            let body = concat!(
                "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_a\",\"type\":\"function\",\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"path\\\":\\\"/a\\\"}\"}}]},\"finish_reason\":null}]}\n\n",
                "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_b\",\"type\":\"function\",\"function\":{\"name\":\"grep\",\"arguments\":\"{\\\"q\\\":\\\"x\\\"}\"}}]},\"finish_reason\":null}]}\n\n",
                "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
                "data: [DONE]\n\n"
            );
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{body}"
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
            sock.flush().await.unwrap();
        });
        let response = reqwest::get(format!("http://{addr}/")).await.unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        OpenAIProvider::handle_stream(response, tx).await;
        server.await.unwrap();

        let mut got = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let Some(tc) = ev.tool_call {
                got.push((tc.id, tc.name, tc.input));
            }
        }
        assert_eq!(
            got,
            vec![
                ("call_a".to_string(), "read".to_string(), serde_json::json!({"path": "/a"})),
                ("call_b".to_string(), "grep".to_string(), serde_json::json!({"q": "x"})),
            ]
        );
    }

    #[test]
    fn go_duration_strings_become_whole_seconds() {
        assert_eq!(go_duration_secs("5s"), Some(5));
        assert_eq!(go_duration_secs("1.2s"), Some(2), "rounded up");
        assert_eq!(go_duration_secs("5h0m0s"), Some(18_000));
        assert_eq!(go_duration_secs("2m30s"), Some(150));
        assert_eq!(go_duration_secs("250ms"), Some(1), "sub-second waits at least one second");
        assert_eq!(go_duration_secs("0s"), Some(0));
        assert_eq!(go_duration_secs("soon"), None);
        assert_eq!(go_duration_secs(""), None);
        assert_eq!(go_duration_secs("5"), None, "a bare number is not a Go duration");
    }

    // Janus's per-user rate limit arrives INSIDE the stream (it sends 200 +
    // SSE headers before the check runs), as an error envelope carrying
    // `retry_after`. The runner must get that wait on the rate-limit event
    // before the error ends the turn.
    #[tokio::test]
    async fn in_stream_rate_limit_carries_retry_after() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 2048];
            let _ = sock.read(&mut buf).await;
            let envelope = "data: {\"error\":{\"message\":\"Rate limit exceeded for user\",\"type\":\"rate_limit_error\",\"code\":\"rate_limit_exceeded\",\"retryable\":true,\"retry_after\":\"5s\"}}\n\n";
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{envelope}"
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
            sock.flush().await.unwrap();
        });
        let response = reqwest::get(format!("http://{addr}/")).await.unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        OpenAIProvider::handle_stream(response, tx).await;
        server.await.unwrap();

        let mut kinds = Vec::new();
        let mut wait = None;
        while let Ok(ev) = rx.try_recv() {
            if let Some(meta) = &ev.rate_limit {
                wait = meta.retry_after_secs;
            }
            kinds.push(ev.event_type);
        }
        assert_eq!(wait, Some(5), "retry_after reaches the runner as seconds");
        let rl = kinds.iter().position(|k| matches!(k, StreamEventType::RateLimit));
        let er = kinds.iter().position(|k| matches!(k, StreamEventType::Error));
        assert!(rl.is_some() && er.is_some() && rl < er, "the wait precedes the error: {kinds:?}");
    }

    // A well-formed stream that ends with finish_reason + [DONE] must still emit
    // a clean Done and no Error (the truncation guard must not false-positive).
    #[tokio::test]
    async fn complete_stream_emits_done_not_error() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 2048];
            let _ = sock.read(&mut buf).await;
            let body = concat!(
                "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\n",
                "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n"
            );
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{body}"
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
            sock.flush().await.unwrap();
        });

        let response = reqwest::get(format!("http://{addr}/")).await.unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        OpenAIProvider::handle_stream(response, tx).await;
        server.await.unwrap();

        let mut saw_error = false;
        let mut saw_done = false;
        while let Ok(ev) = rx.try_recv() {
            match ev.event_type {
                StreamEventType::Error => saw_error = true,
                StreamEventType::Done => saw_done = true,
                _ => {}
            }
        }
        assert!(saw_done, "complete stream must emit Done");
        assert!(!saw_error, "complete stream must NOT emit an Error");
    }

    // Every request names its purpose on the wire, next to the ids in scope,
    // so Janus can group usage by what the call was for. Empty ids stay off.
    #[tokio::test]
    async fn request_carries_purpose_and_trace_headers() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 16384];
            let n = sock.read(&mut buf).await.unwrap();
            let body = "data: [DONE]\n\n";
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{body}"
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&buf[..n]).to_lowercase()
        });

        let provider =
            OpenAIProvider::with_base_url(String::new(), "m".into(), format!("http://{addr}"));
        let req = ChatRequest {
            messages: vec![Message {
                role: "user".into(),
                content: "hi".into(),
                ..Default::default()
            }],
            ..ChatRequest::new(RequestTrace {
                agent_id: "agent-1".into(),
                ..RequestTrace::new("memory_extract")
            })
        };
        let mut rx = provider.stream(&req).await.unwrap();
        while rx.recv().await.is_some() {}
        let head = server.await.unwrap();

        assert!(head.contains("x-purpose: memory_extract"), "{head}");
        assert!(head.contains("x-agent-id: agent-1"), "{head}");
        assert!(!head.contains("x-run-id"), "empty ids stay off the wire: {head}");
    }

    // The hub rotates the NeboAI token on every comms connect, so a Janus
    // provider built once must present the token current at each request,
    // never the one it was built with.
    #[tokio::test]
    async fn live_key_presents_the_token_rotated_after_construction() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let mut heads = Vec::new();
            for _ in 0..2 {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 16384];
                let n = sock.read(&mut buf).await.unwrap();
                let resp = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: [DONE]\n\n";
                sock.write_all(resp.as_bytes()).await.unwrap();
                heads.push(String::from_utf8_lossy(&buf[..n]).to_lowercase());
            }
            heads
        });

        let token = std::sync::Arc::new(std::sync::Mutex::new("token-at-build".to_string()));
        let live = token.clone();
        let provider = OpenAIProvider::with_base_url(
            ApiKey::live(move || live.lock().unwrap().clone()),
            "m".into(),
            format!("http://{addr}"),
        );
        let req = ChatRequest {
            messages: vec![Message {
                role: "user".into(),
                content: "hi".into(),
                ..Default::default()
            }],
            ..ChatRequest::new(RequestTrace::new("chat"))
        };

        let mut rx = provider.stream(&req).await.unwrap();
        while rx.recv().await.is_some() {}
        *token.lock().unwrap() = "token-rotated".to_string();
        let mut rx = provider.stream(&req).await.unwrap();
        while rx.recv().await.is_some() {}

        let heads = server.await.unwrap();
        assert!(heads[0].contains("authorization: bearer token-at-build"), "{}", heads[0]);
        assert!(heads[1].contains("authorization: bearer token-rotated"), "{}", heads[1]);
        assert!(!heads[1].contains("token-at-build"), "{}", heads[1]);
    }
}
