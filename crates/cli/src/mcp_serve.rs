use std::time::Duration;

use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, error, info};

/// Stdio ↔ HTTP bridge for MCP protocol.
/// Reads JSON-RPC from stdin, POSTs to the running Nebo server, writes responses to stdout.
pub struct McpStdioBridge {
    server_url: String,
    http: reqwest::Client,
    tool_allow: Option<Vec<String>>,
    tool_deny: Option<Vec<String>>,
}

impl McpStdioBridge {
    pub fn new(server_url: String, tools: Option<String>, exclude_tools: Option<String>) -> Self {
        Self {
            server_url,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(660)) // nebo chat can take up to 600s
                .build()
                .unwrap_or_default(),
            tool_allow: tools.map(|t| {
                t.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            }),
            tool_deny: exclude_tools.map(|t| {
                t.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            }),
        }
    }

    /// Run the stdio bridge loop. Blocks until stdin closes.
    pub async fn run(&self) -> anyhow::Result<()> {
        // Health check — verify Nebo server is reachable
        if !self.health_check().await {
            anyhow::bail!(
                "Cannot connect to Nebo at {}. Is the server running? Start it with `nebo serve` or `make dev`.",
                self.server_url
            );
        }
        info!(url = %self.server_url, "MCP stdio bridge connected");

        let stdin = tokio::io::stdin();
        let mut stdout = tokio::io::stdout();
        let reader = BufReader::new(stdin);
        let mut lines = reader.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }

            debug!(request = %line, "MCP stdin");

            // Forward to Nebo server
            let response = self.forward_request(&line).await;

            // Apply tool filtering if this is a tools/list response
            let response = self.maybe_filter_tools(&line, response);

            debug!(response = %response, "MCP stdout");

            // Write response to stdout (newline-delimited JSON-RPC)
            if let Err(e) = stdout.write_all(response.as_bytes()).await {
                error!(error = %e, "stdout write failed");
                break;
            }
            if let Err(e) = stdout.write_all(b"\n").await {
                error!(error = %e, "stdout newline write failed");
                break;
            }
            if let Err(e) = stdout.flush().await {
                error!(error = %e, "stdout flush failed");
                break;
            }
        }

        info!("MCP stdin closed, shutting down");
        Ok(())
    }

    /// Check that the Nebo server is reachable.
    async fn health_check(&self) -> bool {
        let url = format!("{}/health", self.server_url);
        for attempt in 1..=3 {
            match self
                .http
                .get(&url)
                .timeout(Duration::from_secs(5))
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => return true,
                Ok(resp) => {
                    eprintln!(
                        "[nebo-mcp] health check attempt {}/3: status {}",
                        attempt,
                        resp.status()
                    );
                }
                Err(e) => {
                    eprintln!("[nebo-mcp] health check attempt {}/3: {}", attempt, e);
                }
            }
            if attempt < 3 {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
        false
    }

    /// Forward a JSON-RPC request to the Nebo HTTP server.
    async fn forward_request(&self, json_line: &str) -> String {
        let url = format!("{}/agent/mcp", self.server_url);
        match self
            .http
            .post(&url)
            .header("Content-Type", "application/json")
            .body(json_line.to_string())
            .send()
            .await
        {
            Ok(resp) => resp.text().await.unwrap_or_else(|e| {
                make_error_response(
                    extract_request_id(json_line),
                    -32603,
                    &format!("read response: {e}"),
                )
            }),
            Err(e) => make_error_response(
                extract_request_id(json_line),
                -32000,
                &format!("Cannot connect to Nebo at {}: {e}", self.server_url),
            ),
        }
    }

    /// If the request was tools/list, filter the response's tools array.
    fn maybe_filter_tools(&self, request: &str, response: String) -> String {
        if self.tool_allow.is_none() && self.tool_deny.is_none() {
            return response;
        }

        // Check if request is tools/list
        let req: serde_json::Value = match serde_json::from_str(request) {
            Ok(v) => v,
            Err(_) => return response,
        };
        if req.get("method").and_then(|m| m.as_str()) != Some("tools/list") {
            return response;
        }

        let mut resp: serde_json::Value = match serde_json::from_str(&response) {
            Ok(v) => v,
            Err(_) => return response,
        };

        if let Some(tools) = resp.pointer_mut("/result/tools") {
            if let Some(arr) = tools.as_array_mut() {
                arr.retain(|tool| {
                    let name = tool.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    if let Some(ref allow) = self.tool_allow {
                        if !allow.iter().any(|a| a == name) {
                            return false;
                        }
                    }
                    if let Some(ref deny) = self.tool_deny {
                        if deny.iter().any(|d| d == name) {
                            return false;
                        }
                    }
                    true
                });
            }
        }

        serde_json::to_string(&resp).unwrap_or(response)
    }
}

/// Extract the `id` field from a JSON-RPC request for error responses.
fn extract_request_id(json_line: &str) -> Option<serde_json::Value> {
    serde_json::from_str::<serde_json::Value>(json_line)
        .ok()
        .and_then(|v| v.get("id").cloned())
}

/// Build a JSON-RPC error response string.
fn make_error_response(id: Option<serde_json::Value>, code: i32, message: &str) -> String {
    serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message,
        }
    }))
    .unwrap_or_else(|_| {
        r#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"internal error"}}"#.to_string()
    })
}

/// Target applications for MCP config generation.
#[derive(Clone, clap::ValueEnum)]
pub enum ConfigTarget {
    /// Claude Desktop
    ClaudeDesktop,
    /// Cursor
    Cursor,
}

/// Print MCP configuration snippet for the target application.
pub fn print_config(target: &ConfigTarget) {
    let nebo_path = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "nebo".to_string());

    let config = json!({
        "mcpServers": {
            "nebo": {
                "command": nebo_path,
                "args": ["mcp", "serve"]
            }
        }
    });

    println!("{}", serde_json::to_string_pretty(&config).unwrap());

    match target {
        ConfigTarget::ClaudeDesktop => {
            eprintln!();
            eprintln!(
                "Add the above to ~/Library/Application Support/Claude/claude_desktop_config.json"
            );
        }
        ConfigTarget::Cursor => {
            eprintln!();
            eprintln!("Add the above to your Cursor MCP settings (.cursor/mcp.json)");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// INVARIANT: tool lists parse as comma-separated with whitespace trimmed
    /// and empty segments dropped; an absent flag stays None (no filtering).
    #[test]
    fn tool_list_parsing() {
        let bridge = McpStdioBridge::new(
            "http://localhost:1".to_string(),
            Some("system, web ,,bot".to_string()),
            None,
        );
        assert_eq!(
            bridge.tool_allow,
            Some(vec!["system".to_string(), "web".to_string(), "bot".to_string()])
        );
        assert!(bridge.tool_deny.is_none());
    }

    /// INVARIANT: tools/list responses are filtered — the allowlist keeps only
    /// named tools and the denylist removes named tools — while every other
    /// method's response passes through byte-identical.
    #[test]
    fn tools_list_filtering() {
        let bridge = McpStdioBridge::new(
            "http://localhost:1".to_string(),
            Some("system,web".to_string()),
            Some("web".to_string()),
        );
        let request = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let response = r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"system"},{"name":"web"},{"name":"bot"}]}}"#;

        let filtered = bridge.maybe_filter_tools(request, response.to_string());
        let v: serde_json::Value = serde_json::from_str(&filtered).unwrap();
        let tools = v["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1, "allow keeps system+web, deny removes web");
        assert_eq!(tools[0]["name"], "system");

        // Non-tools/list traffic must not be rewritten
        let call_req = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call"}"#;
        let untouched = bridge.maybe_filter_tools(call_req, response.to_string());
        assert_eq!(untouched, response);
    }

    /// INVARIANT: with no allow/deny configured the response string is returned
    /// unmodified — filtering never runs.
    #[test]
    fn no_filters_passthrough() {
        let bridge = McpStdioBridge::new("http://localhost:1".to_string(), None, None);
        let request = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let response = r#"{"result":{"tools":[{"name":"system"}]}}"#;
        assert_eq!(
            bridge.maybe_filter_tools(request, response.to_string()),
            response
        );
    }

    /// INVARIANT: extract_request_id pulls the JSON-RPC id (any JSON type) and
    /// tolerates malformed input by returning None.
    #[test]
    fn request_id_extraction() {
        assert_eq!(
            extract_request_id(r#"{"id":42,"method":"x"}"#),
            Some(serde_json::json!(42))
        );
        assert_eq!(
            extract_request_id(r#"{"id":"abc"}"#),
            Some(serde_json::json!("abc"))
        );
        assert_eq!(extract_request_id("not json"), None);
        assert_eq!(extract_request_id("{}"), None);
    }

    /// INVARIANT: make_error_response emits valid JSON-RPC 2.0 with the given
    /// code and message, and a null id when the request id is unknown.
    #[test]
    fn error_response_shape() {
        let s = make_error_response(Some(serde_json::json!(7)), -32000, "nope");
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 7);
        assert_eq!(v["error"]["code"], -32000);
        assert_eq!(v["error"]["message"], "nope");

        let s = make_error_response(None, -32603, "boom");
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert!(v["id"].is_null());
    }
}
