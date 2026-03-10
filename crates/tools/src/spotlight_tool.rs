use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// Spotlight tool: search files using platform-native search (mdfind on macOS, plocate/find on Linux).
pub struct SpotlightTool;

impl SpotlightTool {
    pub fn new() -> Self {
        Self
    }
}

impl DynTool for SpotlightTool {
    fn name(&self) -> &str {
        "spotlight"
    }

    fn description(&self) -> String {
        "Search for files using the OS search index (Spotlight on macOS, plocate on Linux, PowerShell on Windows).\n\n\
         Actions:\n\
         - search: Find files matching a query\n\n\
         Examples:\n  \
         spotlight(action: \"search\", query: \"budget 2024\")\n  \
         spotlight(action: \"search\", query: \"*.pdf\", dir: \"~/Documents\")"
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "Action to perform",
                    "enum": ["search"]
                },
                "query": {
                    "type": "string",
                    "description": "Search query or filename pattern"
                },
                "dir": {
                    "type": "string",
                    "description": "Directory to search within (optional)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum results (default 50)"
                }
            },
            "required": ["action", "query"]
        })
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let action = input["action"].as_str().unwrap_or("");
            match action {
                "search" => handle_search(&input).await,
                _ => ToolResult::error(format!("Unknown action '{}'. Use: search", action)),
            }
        })
    }
}

async fn handle_search(input: &serde_json::Value) -> ToolResult {
    let query = input["query"].as_str().unwrap_or("");
    if query.is_empty() {
        return ToolResult::error("'query' parameter is required");
    }
    let limit = input["limit"].as_i64().unwrap_or(50) as usize;
    let dir = input["dir"].as_str().unwrap_or("");

    #[cfg(target_os = "macos")]
    {
        let mut cmd = tokio::process::Command::new("mdfind");
        if !dir.is_empty() {
            cmd.arg("-onlyin").arg(dir);
        }
        cmd.arg(query);

        match cmd.output().await {
            Ok(output) if output.status.success() => {
                let text = String::from_utf8_lossy(&output.stdout);
                let results: Vec<&str> = text.lines().take(limit).collect();
                if results.is_empty() {
                    ToolResult::ok("No files found")
                } else {
                    ToolResult::ok(format!(
                        "Found {} results:\n{}",
                        results.len(),
                        results.join("\n")
                    ))
                }
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                ToolResult::error(format!("mdfind error: {}", stderr))
            }
            Err(e) => ToolResult::error(format!("Failed to run mdfind: {}", e)),
        }
    }

    #[cfg(target_os = "linux")]
    {
        // Try plocate first, fall back to find
        let output = tokio::process::Command::new("plocate")
            .arg("-l")
            .arg(limit.to_string())
            .arg(query)
            .output()
            .await;

        match output {
            Ok(out) if out.status.success() => {
                let text = String::from_utf8_lossy(&out.stdout);
                let results: Vec<&str> = text.lines().collect();
                if results.is_empty() {
                    ToolResult::ok("No files found")
                } else {
                    ToolResult::ok(format!("Found {} results:\n{}", results.len(), results.join("\n")))
                }
            }
            _ => {
                // Fallback to find
                let search_dir = if dir.is_empty() { "/" } else { dir };
                let find_output = tokio::process::Command::new("find")
                    .args([search_dir, "-name", query, "-maxdepth", "5"])
                    .output()
                    .await;
                match find_output {
                    Ok(out) => {
                        let text = String::from_utf8_lossy(&out.stdout);
                        let results: Vec<&str> = text.lines().take(limit).collect();
                        if results.is_empty() {
                            ToolResult::ok("No files found")
                        } else {
                            ToolResult::ok(format!("Found {} results:\n{}", results.len(), results.join("\n")))
                        }
                    }
                    Err(e) => ToolResult::error(format!("Search failed: {}", e)),
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        // Use PowerShell to search via Windows Search API
        let escaped_query = query.replace("'", "''");
        let search_dir = if dir.is_empty() { "$env:USERPROFILE".to_string() } else { format!("'{}'", dir.replace("'", "''")) };
        let script = format!(
            "Get-ChildItem -Path {} -Recurse -Filter '*{}*' -ErrorAction SilentlyContinue | Select-Object -First {} -ExpandProperty FullName",
            search_dir, escaped_query, limit
        );
        match tokio::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .output()
            .await
        {
            Ok(output) if output.status.success() => {
                let text = String::from_utf8_lossy(&output.stdout);
                let results: Vec<&str> = text.lines().take(limit).collect();
                if results.is_empty() {
                    ToolResult::ok("No files found")
                } else {
                    ToolResult::ok(format!("Found {} results:\n{}", results.len(), results.join("\n")))
                }
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                ToolResult::error(format!("Search error: {}", stderr))
            }
            Err(e) => ToolResult::error(format!("Failed to run search: {}", e)),
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        ToolResult::error("File search is not supported on this platform")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_metadata() {
        let tool = SpotlightTool::new();
        assert_eq!(tool.name(), "spotlight");
        assert!(tool.description().contains("search"));
        let schema = tool.schema();
        assert!(schema["properties"]["query"].is_object());
    }

    #[tokio::test]
    async fn test_missing_query() {
        let tool = SpotlightTool::new();
        let ctx = ToolContext::default();
        let input = serde_json::json!({"action": "search"});
        let result = tool.execute_dyn(&ctx, input).await;
        assert!(result.is_error);
        assert!(result.content.contains("query"));
    }

    #[tokio::test]
    async fn test_unknown_action() {
        let tool = SpotlightTool::new();
        let ctx = ToolContext::default();
        let input = serde_json::json!({"action": "delete", "query": "test"});
        let result = tool.execute_dyn(&ctx, input).await;
        assert!(result.is_error);
        assert!(result.content.contains("Unknown action"));
    }
}
