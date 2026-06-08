//! Stdio transport for local MCP servers.
//!
//! The HTTP transport (`client.rs`) is stateless — each request is a fresh POST.
//! A stdio server is the opposite: one long-lived child process per integration
//! that speaks JSON-RPC 2.0 over its stdin/stdout (newline-delimited). This module
//! owns that process and multiplexes requests over the single pipe, correlating
//! responses to callers by JSON-RPC `id`. It is the `command`/`args`/`env` half of
//! the standard MCP server config block (Claude Desktop / VS Code), the complement
//! to the existing remote `type: http` half.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::{Mutex, oneshot};
use tracing::debug;

use crate::{McpError, McpToolDef, McpToolResult};

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>>;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// A live stdio MCP server: a child process speaking JSON-RPC 2.0 over
/// stdin/stdout. One session per integration; held by the `McpClient`.
pub struct StdioSession {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    pending: Pending,
    next_id: AtomicI64,
}

impl StdioSession {
    /// Spawn the server process and perform the MCP `initialize` handshake.
    pub async fn spawn(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<Arc<Self>, McpError> {
        let mut child = tokio::process::Command::new(command)
            .args(args)
            .envs(env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| McpError::Other(format!("spawn MCP stdio server '{command}': {e}")))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::Other("stdio server has no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::Other("stdio server has no stdout".into()))?;

        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));

        // Background reader: dispatch each response line to its waiting caller by
        // `id`. Notifications (no `id`) are ignored. Ends when stdout closes (i.e.
        // the child exits / is killed).
        {
            let pending = pending.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if line.trim().is_empty() {
                        continue;
                    }
                    let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                        debug!(line = %line, "MCP stdio: non-JSON line from server");
                        continue;
                    };
                    if let Some(id) = msg.get("id").and_then(|v| v.as_i64()) {
                        if let Some(tx) = pending.lock().await.remove(&id) {
                            let _ = tx.send(msg);
                        }
                    }
                }
            });
        }

        let session = Arc::new(Self {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            pending,
            next_id: AtomicI64::new(1),
        });

        session.initialize().await?;
        Ok(session)
    }

    async fn initialize(&self) -> Result<(), McpError> {
        self.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "nebo", "version": env!("CARGO_PKG_VERSION") }
            }),
        )
        .await?;
        // `initialized` is a notification — no id, no response expected.
        self.notify("notifications/initialized", json!({})).await
    }

    /// List the server's tools (`tools/list`).
    pub async fn list_tools(&self) -> Result<Vec<McpToolDef>, McpError> {
        let result = self.request("tools/list", json!({})).await?;
        Ok(result
            .get("tools")
            .and_then(|t| t.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| serde_json::from_value::<McpToolDef>(t.clone()).ok())
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Invoke a tool (`tools/call`), flattening the content blocks to text.
    pub async fn call_tool(&self, name: &str, input: Value) -> Result<McpToolResult, McpError> {
        let result = self
            .request("tools/call", json!({ "name": name, "arguments": input }))
            .await?;
        let is_error = result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let content = result
            .get("content")
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|blk| blk.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        Ok(McpToolResult { content, is_error })
    }

    /// Terminate the child process. The reader task ends when stdout closes.
    pub async fn shutdown(&self) {
        let _ = self.child.lock().await.start_kill();
    }

    /// Send a JSON-RPC request and await its correlated response.
    async fn request(&self, method: &str, params: Value) -> Result<Value, McpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let line = format!(
            "{}\n",
            serde_json::to_string(
                &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
            )?
        );
        {
            let mut stdin = self.stdin.lock().await;
            stdin.write_all(line.as_bytes()).await?;
            stdin.flush().await?;
        }

        let msg = match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
            Ok(Ok(msg)) => msg,
            Ok(Err(_)) => {
                return Err(McpError::Other(format!(
                    "MCP stdio '{method}': server closed the connection"
                )));
            }
            Err(_) => {
                self.pending.lock().await.remove(&id);
                return Err(McpError::Other(format!("MCP stdio '{method}': timed out")));
            }
        };

        if let Some(err) = msg.get("error") {
            let m = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            return Err(McpError::Other(format!("MCP stdio '{method}' error: {m}")));
        }
        Ok(msg.get("result").cloned().unwrap_or(Value::Null))
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
        let line = format!(
            "{}\n",
            serde_json::to_string(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))?
        );
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await?;
        stdin.flush().await?;
        Ok(())
    }
}
