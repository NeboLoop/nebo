//! `agent_structured` — drive a provider through a bounded "research, then forced
//! structured output" loop, validating the result against a JSON schema.
//!
//! The deterministic deep-research harness uses this for every sub-agent: the agent
//! does free web work with `tool_choice=Auto`, then the harness FORCES a single
//! `StructuredOutput` call (`tool_choice=Tool`) and validates the captured input
//! against the original schema, retrying on mismatch. It drives `provider.stream()`
//! directly — no `Runner`, no session — so it is deterministic and concurrency-safe.

use std::future::Future;
use std::sync::Arc;

use a2ui_validation::validate as validate_schema;
use ai::{
    ChatRequest, Message, Provider, ProviderError, StreamEventType, ToolCall, ToolChoice,
    ToolDefinition,
};

/// The tool a structured sub-agent must call exactly once to return its final answer.
pub const STRUCTURED_OUTPUT_TOOL: &str = "StructuredOutput";

const DEFAULT_MAX_VALIDATION_RETRIES: u32 = 2;
const DEFAULT_MAX_TOOL_TURNS: u32 = 8;
const DEFAULT_MAX_TOKENS: i32 = 4096;

/// Failure modes of a structured sub-agent run.
#[derive(Debug)]
pub enum StructuredError {
    /// The underlying provider stream errored.
    Provider(String),
    /// The model produced no `StructuredOutput` call, even when forced.
    NoStructuredOutput,
    /// The output failed schema validation after exhausting retries.
    ValidationFailed(Vec<String>),
}

impl std::fmt::Display for StructuredError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Provider(e) => write!(f, "provider error: {e}"),
            Self::NoStructuredOutput => write!(f, "model produced no StructuredOutput call"),
            Self::ValidationFailed(errs) => {
                write!(f, "schema validation failed: {}", errs.join("; "))
            }
        }
    }
}
impl std::error::Error for StructuredError {}

/// One structured sub-agent request.
pub struct StructuredRequest {
    pub system: String,
    pub task: String,
    pub schema: serde_json::Value,
    pub model: String,
    /// Tools the agent may call during phase A (e.g. web search/fetch).
    pub aux_tools: Vec<ToolDefinition>,
    pub max_validation_retries: u32,
    pub max_tool_turns: u32,
    pub max_tokens: i32,
}

impl StructuredRequest {
    /// Construct with sensible defaults (2 validation retries, 8 tool turns).
    pub fn new(
        system: impl Into<String>,
        task: impl Into<String>,
        schema: serde_json::Value,
        model: impl Into<String>,
    ) -> Self {
        Self {
            system: system.into(),
            task: task.into(),
            schema,
            model: model.into(),
            aux_tools: Vec::new(),
            max_validation_retries: DEFAULT_MAX_VALIDATION_RETRIES,
            max_tool_turns: DEFAULT_MAX_TOOL_TURNS,
            max_tokens: DEFAULT_MAX_TOKENS,
        }
    }

    pub fn with_aux_tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.aux_tools = tools;
        self
    }
}

/// Run a structured sub-agent: phase A (free tool use, may call StructuredOutput
/// early) then phase B (forced + schema-validated StructuredOutput). Returns the
/// validated JSON object. `tool_exec` runs the model's aux-tool calls.
pub async fn agent_structured<F, Fut>(
    provider: Arc<dyn Provider>,
    req: StructuredRequest,
    mut tool_exec: F,
) -> Result<serde_json::Value, StructuredError>
where
    F: FnMut(ToolCall) -> Fut + Send,
    Fut: Future<Output = Result<String, String>> + Send,
{
    let structured_tool = ToolDefinition {
        name: STRUCTURED_OUTPUT_TOOL.to_string(),
        description: "Return your final answer. The input schema defines the required shape. \
                      Call this tool exactly once."
            .to_string(),
        input_schema: req.schema.clone(),
    };

    let mut messages = vec![Message {
        role: "user".to_string(),
        content: req.task.clone(),
        ..Default::default()
    }];

    let mut all_tools = req.aux_tools.clone();
    all_tools.push(structured_tool.clone());

    let mut captured: Option<serde_json::Value> = None;

    // --- Phase A: free tool use; the model may call StructuredOutput early. ---
    for _ in 0..req.max_tool_turns {
        let chat = ChatRequest {
            messages: messages.clone(),
            tools: all_tools.clone(),
            tool_choice: ToolChoice::Auto,
            system: req.system.clone(),
            model: req.model.clone(),
            max_tokens: req.max_tokens,
            temperature: 0.0,
            ..Default::default()
        };
        let (text, tool_calls) = drive(&provider, &chat).await?;

        if let Some(tc) = tool_calls.iter().find(|t| t.name == STRUCTURED_OUTPUT_TOOL) {
            captured = Some(tc.input.clone());
            break;
        }
        if tool_calls.is_empty() {
            break; // text-only response → go force the structured call
        }
        // Record the assistant's tool calls, then execute + record their results.
        messages.push(assistant_tool_turn(&text, &tool_calls));
        for tc in &tool_calls {
            let result = tool_exec(tc.clone())
                .await
                .unwrap_or_else(|e| format!("Tool error: {e}"));
            messages.push(tool_result_turn(&tc.id, &result));
        }
    }

    // --- Phase B: force the StructuredOutput call (if needed), validate, retry. ---
    let mut retries = 0;
    loop {
        if captured.is_none() {
            let chat = ChatRequest {
                messages: messages.clone(),
                tools: vec![structured_tool.clone()],
                tool_choice: ToolChoice::Tool(STRUCTURED_OUTPUT_TOOL.to_string()),
                system: req.system.clone(),
                model: req.model.clone(),
                max_tokens: req.max_tokens,
                temperature: 0.0,
                ..Default::default()
            };
            let (_text, tool_calls) = drive(&provider, &chat).await?;
            captured = tool_calls
                .into_iter()
                .find(|t| t.name == STRUCTURED_OUTPUT_TOOL)
                .map(|t| t.input);
        }

        let instance = match captured.take() {
            Some(v) => v,
            None => return Err(StructuredError::NoStructuredOutput),
        };

        match validate_schema(&req.schema, &instance, STRUCTURED_OUTPUT_TOOL) {
            Ok(()) => return Ok(instance),
            Err(errors) => {
                if retries >= req.max_validation_retries {
                    return Err(StructuredError::ValidationFailed(
                        errors
                            .iter()
                            .map(|e| format!("{}: {}", e.path, e.message))
                            .collect(),
                    ));
                }
                retries += 1;
                let detail = errors
                    .iter()
                    .map(|e| format!("- {}: {}", e.path, e.message))
                    .collect::<Vec<_>>()
                    .join("\n");
                // Re-prompt with the JSON-pointer errors (plain user message → cross-provider safe).
                messages.push(Message {
                    role: "user".to_string(),
                    content: format!(
                        "Output does not match required schema:\n{detail}\n\
                         Call {STRUCTURED_OUTPUT_TOOL} again with a corrected shape."
                    ),
                    ..Default::default()
                });
                // `captured` stays None → the loop forces another StructuredOutput call.
            }
        }
    }
}

/// Drive one provider request to completion, collecting assistant text + tool calls.
async fn drive(
    provider: &Arc<dyn Provider>,
    chat: &ChatRequest,
) -> Result<(String, Vec<ToolCall>), StructuredError> {
    let mut rx = provider
        .stream(chat)
        .await
        .map_err(|e: ProviderError| StructuredError::Provider(e.to_string()))?;
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    while let Some(ev) = rx.recv().await {
        match ev.event_type {
            StreamEventType::Text => text.push_str(&ev.text),
            StreamEventType::ToolCall => {
                if let Some(tc) = ev.tool_call {
                    tool_calls.push(tc);
                }
            }
            StreamEventType::Error => {
                return Err(StructuredError::Provider(
                    ev.error.unwrap_or_else(|| "stream error".to_string()),
                ));
            }
            _ => {}
        }
    }
    Ok((text, tool_calls))
}

fn assistant_tool_turn(text: &str, tool_calls: &[ToolCall]) -> Message {
    Message {
        role: "assistant".to_string(),
        content: text.to_string(),
        tool_calls: serde_json::to_value(tool_calls).ok(),
        ..Default::default()
    }
}

fn tool_result_turn(tool_call_id: &str, content: &str) -> Message {
    Message {
        role: "tool".to_string(),
        tool_results: Some(serde_json::json!([{
            "tool_call_id": tool_call_id,
            "content": content,
            "is_error": false,
        }])),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// Mock provider: each `stream()` call returns one scripted StructuredOutput input.
    struct MockProvider {
        scripted: Mutex<VecDeque<serde_json::Value>>,
    }

    #[async_trait]
    impl Provider for MockProvider {
        fn id(&self) -> &str {
            "mock"
        }
        async fn stream(&self, _req: &ChatRequest) -> Result<ai::EventReceiver, ProviderError> {
            let input = self
                .scripted
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| serde_json::json!({}));
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            tokio::spawn(async move {
                let _ = tx
                    .send(ai::StreamEvent::tool_call(ToolCall {
                        id: "1".to_string(),
                        name: STRUCTURED_OUTPUT_TOOL.to_string(),
                        input,
                    }))
                    .await;
                let _ = tx.send(ai::StreamEvent::done()).await;
            });
            Ok(rx)
        }
    }

    fn schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["answer"],
            "properties": { "answer": { "type": "string" } },
            "additionalProperties": false
        })
    }

    fn mock(scripted: Vec<serde_json::Value>) -> Arc<dyn Provider> {
        Arc::new(MockProvider {
            scripted: Mutex::new(scripted.into()),
        })
    }

    async fn noop_tool(_tc: ToolCall) -> Result<String, String> {
        Ok(String::new())
    }

    #[tokio::test]
    async fn returns_validated_object() {
        let provider = mock(vec![serde_json::json!({"answer": "42"})]);
        let req = StructuredRequest::new("sys", "task", schema(), "mock");
        let out = agent_structured(provider, req, noop_tool).await.unwrap();
        assert_eq!(out["answer"], "42");
    }

    #[tokio::test]
    async fn retries_on_invalid_then_validates() {
        // First output is missing the required field → validation fails → retry → valid.
        let provider = mock(vec![
            serde_json::json!({ "wrong": "shape" }),
            serde_json::json!({ "answer": "ok" }),
        ]);
        let req = StructuredRequest::new("sys", "task", schema(), "mock");
        let out = agent_structured(provider, req, noop_tool).await.unwrap();
        assert_eq!(out["answer"], "ok");
    }

    #[tokio::test]
    async fn gives_up_after_retries_exhausted() {
        // Always invalid → exhausts retries → ValidationFailed.
        let provider = mock(vec![
            serde_json::json!({ "wrong": "a" }),
            serde_json::json!({ "wrong": "b" }),
            serde_json::json!({ "wrong": "c" }),
            serde_json::json!({ "wrong": "d" }),
        ]);
        let req = StructuredRequest::new("sys", "task", schema(), "mock");
        let err = agent_structured(provider, req, noop_tool).await.unwrap_err();
        assert!(matches!(err, StructuredError::ValidationFailed(_)));
    }
}
