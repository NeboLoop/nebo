use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::sse::{SseEvent, parse_sse_line};
use crate::types::*;

/// Native Google Gemini provider using the REST API directly.
///
/// Handles Gemini-specific requirements: alternating user/model turns,
/// function declarations from JSON Schema, and sequential tool call IDs.
pub struct GeminiProvider {
    api_key: String,
    model: String,
}

impl GeminiProvider {
    pub fn new(api_key: String, model: String) -> Self {
        Self { api_key, model }
    }

    /// Build Gemini request contents from our generic message format.
    /// Returns (history contents, system instruction text).
    fn build_contents(&self, req: &ChatRequest) -> (Vec<GeminiContent>, String) {
        // Collect responded tool call IDs
        let mut responded_tool_ids = HashSet::new();
        for msg in &req.messages {
            if msg.role == "tool" {
                if let Some(ref tr_val) = msg.tool_results {
                    if let Ok(results) =
                        serde_json::from_value::<Vec<SessionToolResult>>(tr_val.clone())
                    {
                        for r in &results {
                            responded_tool_ids.insert(r.tool_call_id.clone());
                        }
                    }
                }
            }
        }

        let mut contents: Vec<GeminiContent> = Vec::new();

        for msg in &req.messages {
            match msg.role.as_str() {
                "user" => {
                    if !msg.content.is_empty() {
                        contents.push(GeminiContent {
                            role: "user".to_string(),
                            parts: vec![GeminiPart::Text {
                                text: msg.content.clone(),
                            }],
                        });
                    }
                }
                "assistant" => {
                    let mut parts = Vec::new();

                    if !msg.content.is_empty() {
                        parts.push(GeminiPart::Text {
                            text: msg.content.clone(),
                        });
                    }

                    // Add function calls (only those with responses)
                    if let Some(ref tc_val) = msg.tool_calls {
                        if let Ok(tcs) =
                            serde_json::from_value::<Vec<SessionToolCall>>(tc_val.clone())
                        {
                            for tc in tcs {
                                if !responded_tool_ids.contains(&tc.id) {
                                    continue;
                                }
                                let args: HashMap<String, serde_json::Value> =
                                    serde_json::from_value(tc.input.clone())
                                        .unwrap_or_default();
                                parts.push(GeminiPart::FunctionCall {
                                    function_call: GeminiFunctionCall {
                                        name: tc.name,
                                        args,
                                    },
                                });
                            }
                        }
                    }

                    if !parts.is_empty() {
                        contents.push(GeminiContent {
                            role: "model".to_string(),
                            parts,
                        });
                    }
                }
                "tool" => {
                    if let Some(ref tr_val) = msg.tool_results {
                        if let Ok(results) =
                            serde_json::from_value::<Vec<SessionToolResult>>(tr_val.clone())
                        {
                            let mut parts = Vec::new();
                            for r in &results {
                                let tool_name =
                                    extract_tool_name(&r.tool_call_id, &req.messages);
                                let mut response = HashMap::new();
                                response.insert(
                                    "result".to_string(),
                                    serde_json::Value::String(r.content.clone()),
                                );
                                parts.push(GeminiPart::FunctionResponse {
                                    function_response: GeminiFunctionResponse {
                                        name: tool_name,
                                        response,
                                    },
                                });
                            }
                            if !parts.is_empty() {
                                // Function responses go in a "user" role content
                                contents.push(GeminiContent {
                                    role: "user".to_string(),
                                    parts,
                                });
                            }
                        }
                    }
                }
                "system" => {
                    // Handled via system_instruction
                }
                _ => {}
            }
        }

        // Normalize: ensure alternating turns and starts with user
        contents = normalize_contents(contents);

        (contents, req.system.clone())
    }

    /// Convert tool definitions to Gemini function declarations.
    fn build_tools(&self, tools: &[ToolDefinition]) -> Option<Vec<GeminiToolDecl>> {
        if tools.is_empty() {
            return None;
        }

        let funcs: Vec<GeminiFunctionDeclaration> = tools
            .iter()
            .map(|t| GeminiFunctionDeclaration {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: convert_json_schema(&t.input_schema),
            })
            .collect();

        Some(vec![GeminiToolDecl {
            function_declarations: funcs,
        }])
    }
}

#[async_trait]
impl Provider for GeminiProvider {
    fn id(&self) -> &str {
        "google"
    }

    async fn stream(&self, req: &ChatRequest) -> Result<EventReceiver, ProviderError> {
        let model = if req.model.is_empty() {
            &self.model
        } else {
            &req.model
        };

        let (contents, system_text) = self.build_contents(req);
        let tools = self.build_tools(&req.tools);

        // Build the request body
        let mut body = serde_json::json!({
            "contents": contents,
        });

        if !system_text.is_empty() {
            body["system_instruction"] = serde_json::json!({
                "parts": [{"text": system_text}]
            });
        }

        if let Some(tools) = tools {
            body["tools"] = serde_json::to_value(&tools).unwrap_or_default();
        }

        // Generation config
        let mut gen_config = serde_json::Map::new();
        if req.temperature > 0.0 {
            gen_config.insert(
                "temperature".into(),
                serde_json::Value::Number(
                    serde_json::Number::from_f64(req.temperature)
                        .unwrap_or_else(|| serde_json::Number::from(0)),
                ),
            );
        }
        if req.max_tokens > 0 {
            gen_config.insert(
                "maxOutputTokens".into(),
                serde_json::json!(req.max_tokens),
            );
        }
        if !gen_config.is_empty() {
            body["generationConfig"] = serde_json::Value::Object(gen_config);
        }

        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:streamGenerateContent?key={}&alt=sse",
            model, self.api_key
        );

        info!(
            model = model,
            contents = contents.len(),
            tools = req.tools.len(),
            "sending Gemini request"
        );

        let client = reqwest::Client::new();
        let response = client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Request(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            warn!(
                status = status.as_u16(),
                body = %body,
                "Gemini HTTP error"
            );
            return Err(map_gemini_error(status.as_u16(), &body));
        }

        let (tx, rx) = mpsc::channel(100);
        tokio::spawn(handle_gemini_stream(response, tx));
        Ok(rx)
    }
}

/// Process Gemini SSE stream.
async fn handle_gemini_stream(response: reqwest::Response, tx: mpsc::Sender<StreamEvent>) {
    let mut byte_stream = response.bytes_stream();
    let mut line_buf = String::new();
    let mut tool_call_counter = 0u32;

    while let Some(result) = byte_stream.next().await {
        let bytes = match result {
            Ok(b) => b,
            Err(e) => {
                warn!(error = %e, "Gemini stream read error");
                let _ = tx.send(StreamEvent::error(format!("stream error: {e}"))).await;
                break;
            }
        };

        let text = String::from_utf8_lossy(&bytes);
        line_buf.push_str(&text);

        while let Some(newline_pos) = line_buf.find('\n') {
            let line = line_buf[..newline_pos].to_string();
            line_buf = line_buf[newline_pos + 1..].to_string();

            match parse_sse_line(&line) {
                SseEvent::Done => {
                    let _ = tx.send(StreamEvent::done()).await;
                    return;
                }
                SseEvent::Data(data) => {
                    let chunk: GeminiStreamChunk = match serde_json::from_str(&data) {
                        Ok(c) => c,
                        Err(e) => {
                            // Check for error response
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&data) {
                                if let Some(err) = val.get("error") {
                                    let msg = err.get("message")
                                        .and_then(|m| m.as_str())
                                        .unwrap_or("Gemini API error");
                                    let _ = tx.send(StreamEvent::error(msg.to_string())).await;
                                    return;
                                }
                            }
                            debug!(error = %e, "failed to parse Gemini chunk");
                            continue;
                        }
                    };

                    if let Some(candidates) = chunk.candidates {
                        for candidate in &candidates {
                            if let Some(ref content) = candidate.content {
                                for part in &content.parts {
                                    match part {
                                        GeminiResponsePart::Text { text } => {
                                            if !text.is_empty() {
                                                let _ = tx.send(StreamEvent::text(text)).await;
                                            }
                                        }
                                        GeminiResponsePart::FunctionCall { function_call } => {
                                            tool_call_counter += 1;
                                            let input = serde_json::to_value(&function_call.args)
                                                .unwrap_or(serde_json::json!({}));
                                            let _ = tx
                                                .send(StreamEvent::tool_call(ToolCall {
                                                    id: format!("gemini-call-{}", tool_call_counter),
                                                    name: function_call.name.clone(),
                                                    input,
                                                }))
                                                .await;
                                        }
                                    }
                                }
                            }

                            // Check finish reason
                            if let Some(ref reason) = candidate.finish_reason {
                                match reason.as_str() {
                                    "STOP" | "MAX_TOKENS" => {
                                        let _ = tx.send(StreamEvent::done()).await;
                                        return;
                                    }
                                    "SAFETY" => {
                                        let _ = tx.send(StreamEvent::error(
                                            "Response blocked by safety filters".to_string(),
                                        )).await;
                                        return;
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }

                    // Usage metadata
                    if let Some(ref usage) = chunk.usage_metadata {
                        let _ = tx
                            .send(StreamEvent::usage(UsageInfo {
                                input_tokens: usage.prompt_token_count.unwrap_or(0),
                                output_tokens: usage.candidates_token_count.unwrap_or(0),
                                cache_creation_input_tokens: 0,
                                cache_read_input_tokens: 0,
                            }))
                            .await;
                    }
                }
                _ => {}
            }
        }
    }

    let _ = tx.send(StreamEvent::done()).await;
}

/// Normalize contents to ensure alternating user/model turns.
fn normalize_contents(contents: Vec<GeminiContent>) -> Vec<GeminiContent> {
    if contents.is_empty() {
        return contents;
    }

    let mut normalized: Vec<GeminiContent> = Vec::with_capacity(contents.len());
    let mut last_role = String::new();

    for c in contents {
        // Gemini requires starting with user
        if normalized.is_empty() && c.role != "user" {
            normalized.push(GeminiContent {
                role: "user".to_string(),
                parts: vec![GeminiPart::Text {
                    text: "Continue.".to_string(),
                }],
            });
        }

        // Merge consecutive same-role messages
        if c.role == last_role && !normalized.is_empty() {
            let last = normalized.last_mut().unwrap();
            last.parts.extend(c.parts);
        } else {
            last_role = c.role.clone();
            normalized.push(c);
        }
    }

    normalized
}

/// Extract tool name from a tool call ID by searching messages.
fn extract_tool_name(tool_call_id: &str, messages: &[Message]) -> String {
    for msg in messages {
        if msg.role == "assistant" {
            if let Some(ref tc_val) = msg.tool_calls {
                if let Ok(calls) =
                    serde_json::from_value::<Vec<SessionToolCall>>(tc_val.clone())
                {
                    for c in calls {
                        if c.id == tool_call_id {
                            return c.name;
                        }
                    }
                }
            }
        }
    }
    "unknown".to_string()
}

/// Convert JSON Schema to Gemini schema format.
fn convert_json_schema(schema: &serde_json::Value) -> GeminiSchema {
    let obj = match schema.as_object() {
        Some(o) => o,
        None => return GeminiSchema::default_object(),
    };

    let type_str = obj
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("OBJECT");

    let schema_type = match type_str.to_uppercase().as_str() {
        "STRING" => "STRING",
        "NUMBER" => "NUMBER",
        "INTEGER" => "INTEGER",
        "BOOLEAN" => "BOOLEAN",
        "ARRAY" => "ARRAY",
        "OBJECT" => "OBJECT",
        _ => "STRING",
    };

    let description = obj.get("description").and_then(|v| v.as_str()).map(String::from);

    let enum_values = obj.get("enum").and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect()
    });

    let properties = obj.get("properties").and_then(|v| v.as_object()).map(|props| {
        props
            .iter()
            .map(|(name, prop_val)| (name.clone(), convert_json_schema(prop_val)))
            .collect()
    });

    let required = obj.get("required").and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect()
    });

    let items = obj
        .get("items")
        .map(|v| Box::new(convert_json_schema(v)));

    GeminiSchema {
        schema_type: schema_type.to_string(),
        description,
        enum_values,
        properties,
        required,
        items,
    }
}

/// Map Gemini HTTP errors to ProviderError.
fn map_gemini_error(status: u16, body: &str) -> ProviderError {
    let msg = if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        v["error"]["message"]
            .as_str()
            .unwrap_or(body)
            .to_string()
    } else {
        body.to_string()
    };

    match status {
        429 => ProviderError::RateLimit,
        401 | 403 => ProviderError::Auth(msg),
        _ => {
            if msg.contains("context") && msg.contains("exceeded") {
                return ProviderError::ContextOverflow;
            }
            ProviderError::Api {
                code: status.to_string(),
                message: msg,
                retryable: status >= 500,
            }
        }
    }
}

// --- Gemini API types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GeminiContent {
    role: String,
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
enum GeminiPart {
    Text {
        text: String,
    },
    FunctionCall {
        #[serde(rename = "functionCall")]
        function_call: GeminiFunctionCall,
    },
    FunctionResponse {
        #[serde(rename = "functionResponse")]
        function_response: GeminiFunctionResponse,
    },
}

impl<'de> Deserialize<'de> for GeminiPart {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let val = serde_json::Value::deserialize(deserializer)?;
        if val.get("text").is_some() {
            Ok(GeminiPart::Text {
                text: val["text"].as_str().unwrap_or("").to_string(),
            })
        } else if val.get("functionCall").is_some() {
            let fc: GeminiFunctionCall =
                serde_json::from_value(val["functionCall"].clone())
                    .map_err(serde::de::Error::custom)?;
            Ok(GeminiPart::FunctionCall { function_call: fc })
        } else if val.get("functionResponse").is_some() {
            let fr: GeminiFunctionResponse =
                serde_json::from_value(val["functionResponse"].clone())
                    .map_err(serde::de::Error::custom)?;
            Ok(GeminiPart::FunctionResponse {
                function_response: fr,
            })
        } else {
            // Default to empty text
            Ok(GeminiPart::Text {
                text: String::new(),
            })
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GeminiFunctionCall {
    name: String,
    #[serde(default)]
    args: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GeminiFunctionResponse {
    name: String,
    response: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GeminiToolDecl {
    #[serde(rename = "functionDeclarations")]
    function_declarations: Vec<GeminiFunctionDeclaration>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GeminiFunctionDeclaration {
    name: String,
    description: String,
    parameters: GeminiSchema,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GeminiSchema {
    #[serde(rename = "type")]
    schema_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    enum_values: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    properties: Option<HashMap<String, GeminiSchema>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    required: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    items: Option<Box<GeminiSchema>>,
}

impl GeminiSchema {
    fn default_object() -> Self {
        Self {
            schema_type: "OBJECT".to_string(),
            description: None,
            enum_values: None,
            properties: None,
            required: None,
            items: None,
        }
    }
}

// Stream response types
#[derive(Debug, Deserialize)]
struct GeminiStreamChunk {
    #[serde(default)]
    candidates: Option<Vec<GeminiCandidate>>,
    #[serde(rename = "usageMetadata")]
    usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Debug, Deserialize)]
struct GeminiCandidate {
    content: Option<GeminiResponseContent>,
    #[serde(rename = "finishReason")]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GeminiResponseContent {
    parts: Vec<GeminiResponsePart>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum GeminiResponsePart {
    FunctionCall {
        #[serde(rename = "functionCall")]
        function_call: GeminiFunctionCall,
    },
    Text {
        text: String,
    },
}

#[derive(Debug, Deserialize)]
struct GeminiUsageMetadata {
    #[serde(rename = "promptTokenCount")]
    prompt_token_count: Option<i32>,
    #[serde(rename = "candidatesTokenCount")]
    candidates_token_count: Option<i32>,
}

// Re-used from openai module (same session format)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gemini_history_normalization() {
        let contents = vec![
            GeminiContent {
                role: "model".to_string(),
                parts: vec![GeminiPart::Text {
                    text: "Hello".to_string(),
                }],
            },
            GeminiContent {
                role: "user".to_string(),
                parts: vec![GeminiPart::Text {
                    text: "Hi".to_string(),
                }],
            },
        ];

        let normalized = normalize_contents(contents);
        // Should prepend a user message before the model message
        assert_eq!(normalized.len(), 3);
        assert_eq!(normalized[0].role, "user");
        assert_eq!(normalized[1].role, "model");
        assert_eq!(normalized[2].role, "user");
    }

    #[test]
    fn test_gemini_merge_consecutive() {
        let contents = vec![
            GeminiContent {
                role: "user".to_string(),
                parts: vec![GeminiPart::Text {
                    text: "Part 1".to_string(),
                }],
            },
            GeminiContent {
                role: "user".to_string(),
                parts: vec![GeminiPart::Text {
                    text: "Part 2".to_string(),
                }],
            },
        ];

        let normalized = normalize_contents(contents);
        // Should merge consecutive user messages
        assert_eq!(normalized.len(), 1);
        assert_eq!(normalized[0].parts.len(), 2);
    }

    #[test]
    fn test_gemini_schema_conversion() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The name"
                },
                "count": {
                    "type": "integer"
                },
                "tags": {
                    "type": "array",
                    "items": {
                        "type": "string"
                    }
                }
            },
            "required": ["name"]
        });

        let result = convert_json_schema(&schema);
        assert_eq!(result.schema_type, "OBJECT");
        let props = result.properties.unwrap();
        assert_eq!(props["name"].schema_type, "STRING");
        assert_eq!(props["count"].schema_type, "INTEGER");
        assert_eq!(props["tags"].schema_type, "ARRAY");
        assert_eq!(props["tags"].items.as_ref().unwrap().schema_type, "STRING");
        assert_eq!(result.required.unwrap(), vec!["name"]);
    }
}
