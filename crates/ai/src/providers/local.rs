use async_trait::async_trait;
use std::sync::Mutex;
use tracing::warn;

use crate::types::*;

/// Local GGUF model inference provider using llama.cpp.
/// No external runtime (Ollama, etc.) required.
///
/// When the `local-inference` feature is disabled, `stream()` returns an error.
/// Fields are used when the `local-inference` feature is enabled for llama.cpp FFI.
#[cfg_attr(not(feature = "local-inference"), allow(dead_code))]
pub struct LocalProvider {
    model_path: String,
    model_name: String,
    mu: Mutex<()>,
}

#[cfg_attr(not(feature = "local-inference"), allow(dead_code))]
impl LocalProvider {
    pub fn new(model_path: &str, model_name: &str) -> Self {
        Self {
            model_path: model_path.to_string(),
            model_name: model_name.to_string(),
            mu: Mutex::new(()),
        }
    }

    /// Inject tool definitions into the system prompt so local models can call tools
    /// via `<tool_call>` tags in their text output.
    pub(crate) fn build_system_with_tools(&self, system: &str, tools: &[ToolDefinition]) -> String {
        let mut sb = String::from(system);
        sb.push_str("\n\n# Available Tools\n\n");
        sb.push_str("You have access to the following tools. To call a tool, respond with a JSON block wrapped in <tool_call> tags:\n\n");
        sb.push_str("<tool_call>\n{\"name\": \"tool_name\", \"arguments\": {\"arg1\": \"value1\"}}\n</tool_call>\n\n");
        sb.push_str("You may call multiple tools by using multiple <tool_call> blocks.\n\n");

        for tool in tools {
            sb.push_str(&format!("## {}\n{}\n", tool.name, tool.description));
            sb.push_str(&format!(
                "Parameters: {}\n\n",
                serde_json::to_string(&tool.input_schema).unwrap_or_default()
            ));
        }

        sb
    }

    /// Parse `<tool_call>...</tool_call>` blocks from model text output.
    pub(crate) fn extract_tool_calls(&self, response: &str, tools: &[ToolDefinition]) -> Vec<ToolCall> {
        if tools.is_empty() {
            return Vec::new();
        }

        let mut calls = Vec::new();
        let mut remaining = response;
        let mut counter = 0;

        loop {
            let start = match remaining.find("<tool_call>") {
                Some(i) => i,
                None => break,
            };
            let after_tag = &remaining[start + "<tool_call>".len()..];
            let end = match after_tag.find("</tool_call>") {
                Some(i) => i,
                None => break,
            };

            let json_str = after_tag[..end].trim();
            remaining = &after_tag[end + "</tool_call>".len()..];

            #[derive(serde::Deserialize)]
            struct RawCall {
                name: String,
                arguments: serde_json::Value,
            }

            match serde_json::from_str::<RawCall>(json_str) {
                Ok(raw) => {
                    // Verify the tool exists
                    if tools.iter().any(|t| t.name == raw.name) {
                        counter += 1;
                        calls.push(ToolCall {
                            id: format!("local-call-{}", counter),
                            name: raw.name,
                            input: raw.arguments,
                        });
                    } else {
                        warn!(name = %raw.name, "model called unknown tool");
                    }
                }
                Err(e) => {
                    warn!(json = %json_str, error = %e, "failed to parse tool call");
                }
            }
        }

        calls
    }
}

#[async_trait]
impl Provider for LocalProvider {
    fn id(&self) -> &str {
        "local"
    }

    fn handles_tools(&self) -> bool {
        false
    }

    async fn stream(&self, req: &ChatRequest) -> Result<EventReceiver, ProviderError> {
        #[cfg(feature = "local-inference")]
        {
            return self.stream_local(req).await;
        }

        #[cfg(not(feature = "local-inference"))]
        {
            let _ = req;
            Err(ProviderError::Request(
                "Local inference requires the 'local-inference' feature".into(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_injection() {
        let provider = LocalProvider::new("/tmp/test.gguf", "test");
        let tools = vec![ToolDefinition {
            name: "file".into(),
            description: "Read and write files".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }];

        let result = provider.build_system_with_tools("You are a helpful assistant.", &tools);
        assert!(result.contains("# Available Tools"));
        assert!(result.contains("## file"));
        assert!(result.contains("<tool_call>"));
    }

    #[test]
    fn test_extract_tool_calls() {
        let provider = LocalProvider::new("/tmp/test.gguf", "test");
        let tools = vec![
            ToolDefinition {
                name: "file".into(),
                description: "File ops".into(),
                input_schema: serde_json::json!({}),
            },
            ToolDefinition {
                name: "shell".into(),
                description: "Shell ops".into(),
                input_schema: serde_json::json!({}),
            },
        ];

        let response = r#"I'll read the file for you.
<tool_call>
{"name": "file", "arguments": {"action": "read", "path": "/tmp/test.txt"}}
</tool_call>
And then run a command:
<tool_call>
{"name": "shell", "arguments": {"command": "ls -la"}}
</tool_call>"#;

        let calls = provider.extract_tool_calls(response, &tools);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "local-call-1");
        assert_eq!(calls[0].name, "file");
        assert_eq!(calls[1].id, "local-call-2");
        assert_eq!(calls[1].name, "shell");
    }

    #[test]
    fn test_extract_unknown_tool() {
        let provider = LocalProvider::new("/tmp/test.gguf", "test");
        let tools = vec![ToolDefinition {
            name: "file".into(),
            description: "File ops".into(),
            input_schema: serde_json::json!({}),
        }];

        let response = r#"<tool_call>
{"name": "unknown_tool", "arguments": {}}
</tool_call>"#;

        let calls = provider.extract_tool_calls(response, &tools);
        assert_eq!(calls.len(), 0); // Unknown tool filtered out
    }

    #[test]
    fn test_extract_malformed_json() {
        let provider = LocalProvider::new("/tmp/test.gguf", "test");
        let tools = vec![ToolDefinition {
            name: "file".into(),
            description: "File ops".into(),
            input_schema: serde_json::json!({}),
        }];

        let response = r#"<tool_call>
not valid json
</tool_call>"#;

        let calls = provider.extract_tool_calls(response, &tools);
        assert_eq!(calls.len(), 0);
    }
}
