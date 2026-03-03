use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::mpsc;
use tracing::{debug, info};

use crate::sse::{self, SseEvent};
use crate::types::*;

/// OpenAI provider using raw HTTP SSE streaming.
/// Reusable for any OpenAI-compatible API (OpenRouter, etc.) via base_url.
pub struct OpenAIProvider {
    client: Client,
    api_key: String,
    model: String,
    base_url: String,
    provider_id: String,
}

impl OpenAIProvider {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            model,
            base_url: "https://api.openai.com/v1".to_string(),
            provider_id: "openai".to_string(),
        }
    }

    /// Create with a custom base URL for OpenAI-compatible APIs.
    pub fn with_base_url(api_key: String, model: String, base_url: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            model,
            base_url,
            provider_id: "openai".to_string(),
        }
    }

    /// Override the provider ID (e.g., "openrouter").
    pub fn set_provider_id(&mut self, id: impl Into<String>) {
        self.provider_id = id.into();
    }

    /// Build OpenAI API messages from our generic format.
    fn build_messages(&self, req: &ChatRequest) -> Vec<OpenAIMessage> {
        // Collect tool call/result IDs for orphan filtering
        let mut responded_tool_ids = std::collections::HashSet::new();
        let mut issued_tool_ids = std::collections::HashSet::new();

        for msg in &req.messages {
            if msg.role == "tool" {
                if let Some(ref tr_val) = msg.tool_results {
                    if let Ok(results) = serde_json::from_value::<Vec<SessionToolResult>>(tr_val.clone()) {
                        for r in &results {
                            responded_tool_ids.insert(r.tool_call_id.clone());
                        }
                    }
                }
            }
            if msg.role == "assistant" {
                if let Some(ref tc_val) = msg.tool_calls {
                    if let Ok(tcs) = serde_json::from_value::<Vec<SessionToolCall>>(tc_val.clone()) {
                        for tc in &tcs {
                            issued_tool_ids.insert(tc.id.clone());
                        }
                    }
                }
            }
        }

        let mut messages = Vec::new();

        // Add system message
        if !req.system.is_empty() {
            messages.push(OpenAIMessage {
                role: "system".to_string(),
                content: Some(req.system.clone()),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            });
        }

        for msg in &req.messages {
            match msg.role.as_str() {
                "user" => {
                    if msg.content.is_empty() {
                        continue;
                    }
                    messages.push(OpenAIMessage {
                        role: "user".to_string(),
                        content: Some(msg.content.clone()),
                        tool_calls: None,
                        tool_call_id: None,
                        name: None,
                    });
                }
                "assistant" => {
                    let mut tool_calls = Vec::new();

                    if let Some(ref tc_val) = msg.tool_calls {
                        if let Ok(tcs) = serde_json::from_value::<Vec<SessionToolCall>>(tc_val.clone()) {
                            for tc in tcs {
                                if !responded_tool_ids.contains(&tc.id) {
                                    continue;
                                }
                                tool_calls.push(OpenAIToolCall {
                                    index: None,
                                    id: Some(tc.id),
                                    call_type: Some("function".to_string()),
                                    function: OpenAIFunction {
                                        name: Some(tc.name),
                                        arguments: Some(tc.input.to_string()),
                                    },
                                });
                            }
                        }
                    }

                    if !msg.content.is_empty() || !tool_calls.is_empty() {
                        // Some gateways reject null content with tool_calls
                        let content = if msg.content.is_empty() && !tool_calls.is_empty() {
                            Some(" ".to_string())
                        } else if !msg.content.is_empty() {
                            Some(msg.content.clone())
                        } else {
                            None
                        };

                        messages.push(OpenAIMessage {
                            role: "assistant".to_string(),
                            content,
                            tool_calls: if tool_calls.is_empty() { None } else { Some(tool_calls) },
                            tool_call_id: None,
                            name: None,
                        });
                    }
                }
                "tool" => {
                    if let Some(ref tr_val) = msg.tool_results {
                        if let Ok(results) = serde_json::from_value::<Vec<SessionToolResult>>(tr_val.clone()) {
                            for r in results {
                                if issued_tool_ids.contains(&r.tool_call_id) && responded_tool_ids.contains(&r.tool_call_id) {
                                    messages.push(OpenAIMessage {
                                        role: "tool".to_string(),
                                        content: Some(r.content),
                                        tool_calls: None,
                                        tool_call_id: Some(r.tool_call_id),
                                        name: None,
                                    });
                                }
                            }
                        }
                    }
                }
                "system" => {
                    if msg.content.is_empty() {
                        continue;
                    }
                    messages.push(OpenAIMessage {
                        role: "system".to_string(),
                        content: Some(msg.content.clone()),
                        tool_calls: None,
                        tool_call_id: None,
                        name: None,
                    });
                }
                _ => {}
            }
        }

        messages
    }

    /// Handle the SSE stream from OpenAI.
    async fn handle_stream(
        response: reqwest::Response,
        tx: mpsc::Sender<StreamEvent>,
    ) {
        // Accumulate tool calls by index
        let mut tool_calls: HashMap<i32, AccumulatedToolCall> = HashMap::new();
        let mut emitted_tool_calls = std::collections::HashSet::new();

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

                match sse::parse_sse_line(&line) {
                    SseEvent::Data(data) => {
                        let chunk: OpenAIStreamChunk = match serde_json::from_str(&data) {
                            Ok(c) => c,
                            Err(e) => {
                                debug!("failed to parse OpenAI chunk: {e}, data: {data}");
                                continue;
                            }
                        };

                        if let Some(choice) = chunk.choices.first() {
                            // Stream text content
                            if let Some(ref content) = choice.delta.content {
                                if !content.is_empty() {
                                    let _ = tx.send(StreamEvent::text(content)).await;
                                }
                            }

                            // Stream reasoning content (MiniMax)
                            if let Some(ref reasoning) = choice.delta.reasoning_content {
                                if !reasoning.is_empty() {
                                    let _ = tx.send(StreamEvent::thinking(reasoning)).await;
                                }
                            }

                            // Accumulate tool calls by index
                            if let Some(ref tcs) = choice.delta.tool_calls {
                                for tc in tcs {
                                    let idx = tc.index.unwrap_or(0);
                                    let entry = tool_calls.entry(idx).or_insert_with(|| AccumulatedToolCall {
                                        id: String::new(),
                                        name: String::new(),
                                        arguments: String::new(),
                                    });

                                    if let Some(ref id) = tc.id {
                                        if !id.is_empty() {
                                            entry.id = id.clone();
                                        }
                                    }
                                    if let Some(ref name) = tc.function.name {
                                        if !name.is_empty() {
                                            entry.name = name.clone();
                                        }
                                    }
                                    if let Some(ref args) = tc.function.arguments {
                                        entry.arguments.push_str(args);
                                    }
                                }
                            }

                            // Check finish reason
                            if choice.finish_reason.is_some() {
                                break;
                            }
                        }
                    }
                    SseEvent::Done => {
                        break;
                    }
                    _ => {}
                }
            }
        }

        // Emit accumulated tool calls
        for (_, tc) in &tool_calls {
            if !tc.id.is_empty() && !emitted_tool_calls.contains(&tc.id) {
                emitted_tool_calls.insert(tc.id.clone());
                let input: serde_json::Value = serde_json::from_str(&tc.arguments)
                    .unwrap_or(serde_json::Value::Object(Default::default()));
                let _ = tx.send(StreamEvent::tool_call(ToolCall {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    input,
                })).await;
            }
        }

        let _ = tx.send(StreamEvent::done()).await;
    }
}

#[async_trait]
impl Provider for OpenAIProvider {
    fn id(&self) -> &str {
        &self.provider_id
    }

    async fn stream(
        &self,
        req: &ChatRequest,
    ) -> Result<EventReceiver, ProviderError> {
        let messages = self.build_messages(req);

        let model = if req.model.is_empty() { &self.model } else { &req.model };

        // Build tools
        let tools: Option<Vec<OpenAITool>> = if req.tools.is_empty() {
            None
        } else {
            Some(req.tools.iter().map(|t| OpenAITool {
                tool_type: "function".to_string(),
                function: OpenAIFunctionDef {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.input_schema.clone(),
                },
            }).collect())
        };

        let api_req = OpenAIApiRequest {
            model: model.to_string(),
            messages,
            max_completion_tokens: if req.max_tokens > 0 { Some(req.max_tokens) } else { None },
            temperature: if req.temperature > 0.0 { Some(req.temperature) } else { None },
            stream: true,
            stream_options: Some(StreamOptions { include_usage: true }),
            tools,
            tool_choice: if !req.tools.is_empty() { Some("auto".to_string()) } else { None },
        };

        info!(model = model, messages = api_req.messages.len(), tools = req.tools.len(), "sending OpenAI request");

        let response = self.client
            .post(format!("{}/chat/completions", self.base_url))
            .header("authorization", format!("Bearer {}", self.api_key))
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
            if body.contains("context_length_exceeded") || (body.contains("context") && body.contains("exceeded")) {
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

// --- Helper types ---

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
}

// --- OpenAI API types ---

#[derive(Debug, Serialize)]
struct OpenAIApiRequest {
    model: String,
    messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_completion_tokens: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAITool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
}

#[derive(Debug, Serialize)]
struct StreamOptions {
    include_usage: bool,
}

#[derive(Debug, Serialize)]
struct OpenAIMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAIToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIToolCall {
    #[serde(skip_serializing_if = "Option::is_none")]
    index: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    call_type: Option<String>,
    function: OpenAIFunction,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIFunction {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    arguments: Option<String>,
}

#[derive(Debug, Serialize)]
struct OpenAITool {
    #[serde(rename = "type")]
    tool_type: String,
    function: OpenAIFunctionDef,
}

#[derive(Debug, Serialize)]
struct OpenAIFunctionDef {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

// --- Streaming response types ---

#[derive(Debug, Deserialize)]
struct OpenAIStreamChunk {
    #[serde(default)]
    choices: Vec<OpenAIStreamChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamChoice {
    delta: OpenAIStreamDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OpenAIToolCall>>,
}
