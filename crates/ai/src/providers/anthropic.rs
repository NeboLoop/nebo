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
            if msg.role == "assistant" {
                if let Some(ref tc_val) = msg.tool_calls {
                    if let Ok(tcs) = serde_json::from_value::<Vec<SessionToolCall>>(tc_val.clone()) {
                        for tc in &tcs {
                            all_tool_call_ids.insert(tc.id.clone());
                        }
                    }
                }
            }
            if msg.role == "tool" {
                if let Some(ref tr_val) = msg.tool_results {
                    if let Ok(results) = serde_json::from_value::<Vec<SessionToolResult>>(tr_val.clone()) {
                        for r in &results {
                            responded_tool_ids.insert(r.tool_call_id.clone());
                        }
                    }
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
                    if msg.content.is_empty() {
                        continue;
                    }
                    messages.push(AnthropicMessage {
                        role: "user".to_string(),
                        content: AnthropicContent::Text(msg.content.clone()),
                    });
                }
                "assistant" => {
                    let mut blocks = Vec::new();

                    if !msg.content.is_empty() {
                        blocks.push(ContentBlock::Text { text: msg.content.clone() });
                    }

                    if let Some(ref tc_val) = msg.tool_calls {
                        if let Ok(tcs) = serde_json::from_value::<Vec<SessionToolCall>>(tc_val.clone()) {
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
                                });
                            }
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
                    if let Some(ref tr_val) = msg.tool_results {
                        if let Ok(results) = serde_json::from_value::<Vec<SessionToolResult>>(tr_val.clone()) {
                            let mut blocks = Vec::new();
                            for r in results {
                                if !all_tool_call_ids.contains(&r.tool_call_id) || !responded_tool_ids.contains(&r.tool_call_id) {
                                    continue;
                                }
                                blocks.push(ContentBlock::ToolResult {
                                    tool_use_id: r.tool_call_id,
                                    content: r.content,
                                    is_error: r.is_error,
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
                }
                _ => {}
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
                                if let Some(msg) = event.message {
                                    if let Some(usage) = msg.usage {
                                        let _ = tx.send(StreamEvent::usage(UsageInfo {
                                            input_tokens: usage.input_tokens,
                                            output_tokens: usage.output_tokens,
                                            cache_creation_input_tokens: usage.cache_creation_input_tokens.unwrap_or(0),
                                            cache_read_input_tokens: usage.cache_read_input_tokens.unwrap_or(0),
                                        })).await;
                                    }
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
                                if let Some(block) = event.content_block {
                                    if block.block_type == "tool_use" {
                                        current_tool_id = block.id.unwrap_or_default();
                                        current_tool_name = block.name.unwrap_or_default();
                                        input_buffer.clear();
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

        // Build system blocks with caching
        let system_blocks = if !system_prompt.is_empty() {
            if !req.static_system.is_empty() && system_prompt.starts_with(&req.static_system) {
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

        // Build tools
        let tools: Option<Vec<AnthropicTool>> = if req.tools.is_empty() {
            None
        } else {
            Some(req.tools.iter().map(|t| {
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
                }
            }).collect())
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
#[serde(tag = "type")]
enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String, input: serde_json::Value },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },
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
