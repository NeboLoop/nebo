//! Headless Bridge — wraps `agent-browser` CLI as a browser automation backend.
//!
//! When the Chrome extension is not connected, this bridge provides the same
//! tool interface by spawning `agent-browser` subprocesses. Uses persistent
//! sessions so the headless Chromium stays running across tool calls.
//!
//! Ref translation: Nebo uses `ref_N`, agent-browser uses `@eN`.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::Mutex;
use tracing::{debug, info};

use crate::extension_bridge::{BatchAction, BatchOptions};

/// Tools that agent-browser does not support — return a clear error.
const UNSUPPORTED_TOOLS: &[&str] = &[
    "triple_click",
    "right_click",
    "zoom",
    "list_tabs",
    "new_tab",
    "read_console_messages",
    "console_messages",
    "read_network_requests",
    "network_requests",
    "resize_window",
    "resize",
];

/// Tools that mutate page state — cache is invalidated before these.
const MUTATION_TOOLS: &[&str] = &[
    "click",
    "double_click",
    "hover",
    "form_input",
    "fill",
    "type",
    "select",
    "press",
    "navigate",
    "go_back",
    "go_forward",
    "evaluate",
    "drag",
    "close_tab",
    "close",
    "file_upload",
    "upload_file",
];

/// Cached read_page result with timestamp.
struct PageCacheEntry {
    result: Value,
    timestamp: Instant,
}

/// Default agent-browser session used when a call carries no `session_id`
/// (e.g. top-level web use that isn't scoped to a sub-agent).
const DEFAULT_SESSION: &str = "nebo_default";

/// Headless browser backend wrapping `agent-browser` CLI.
pub struct HeadlessBridge {
    binary: String,
    /// Per-session read_page cache, keyed by the agent-browser session name.
    page_cache: Mutex<HashMap<String, PageCacheEntry>>,
    /// Open session names, so `cleanup_all` can close them on shutdown.
    sessions: Mutex<HashSet<String>>,
}

impl HeadlessBridge {
    /// Detect the `agent-browser` binary. Returns None if not installed.
    pub fn detect_binary() -> Option<String> {
        // Try `which agent-browser` synchronously (called once at startup)
        let output = std::process::Command::new("which")
            .arg("agent-browser")
            .output()
            .ok()?;
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(path);
            }
        }
        None
    }

    /// Create a new HeadlessBridge with the given binary path.
    pub fn new(binary: String) -> Self {
        info!(binary = %binary, "headless browser backend available (agent-browser)");
        Self {
            binary,
            page_cache: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashSet::new()),
        }
    }

    /// Derive a stable agent-browser `--session` name from a Nebo `session_id`.
    /// `None`/empty → the shared default; otherwise sanitize to `[A-Za-z0-9_-]`
    /// so keys like `subagent:research-…:sa-…` are valid and map deterministically.
    fn session_name(session_id: Option<&str>) -> String {
        match session_id {
            Some(s) if !s.is_empty() => s
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect(),
            _ => DEFAULT_SESSION.to_string(),
        }
    }

    /// Execute a browser tool via agent-browser subprocess, scoped to the
    /// sub-agent's own session (`session_id` → agent-browser `--session`).
    pub async fn execute(
        &self,
        tool: &str,
        args: &Value,
        session_id: Option<&str>,
    ) -> Result<Value, String> {
        // Check unsupported
        if UNSUPPORTED_TOOLS.contains(&tool) {
            return Err(format!(
                "Action '{}' is not supported in headless mode. \
                 Connect the Nebo Chrome extension for full browser control.",
                tool
            ));
        }

        let session = Self::session_name(session_id);

        // Track this session so shutdown can close it.
        self.sessions.lock().await.insert(session.clone());

        // Check read_page cache (per session)
        if tool == "read_page" {
            let cached = {
                let guard = self.page_cache.lock().await;
                guard
                    .get(&session)
                    .filter(|e| e.timestamp.elapsed() < Duration::from_millis(2500))
                    .map(|e| e.result.clone())
            };
            if let Some(result) = cached {
                debug!("headless read_page cache hit");
                return Ok(result);
            }
        }

        // Invalidate cache before mutation tools (per session)
        if MUTATION_TOOLS.contains(&tool) {
            self.page_cache.lock().await.remove(&session);
        }

        let cmd_args = self.map_tool_to_command(tool, args)?;
        let result = self.run_command(&session, &cmd_args).await?;
        let normalized = self.normalize_response(tool, result)?;

        // Populate read_page cache on success (per session)
        if tool == "read_page" {
            self.page_cache.lock().await.insert(
                session.clone(),
                PageCacheEntry {
                    result: normalized.clone(),
                    timestamp: Instant::now(),
                },
            );
        }

        Ok(normalized)
    }

    /// Execute multiple tools sequentially (no WS batching in headless).
    pub async fn batch_execute(
        &self,
        actions: Vec<BatchAction>,
        opts: BatchOptions,
        session_id: Option<&str>,
    ) -> Result<Vec<Result<Value, String>>, String> {
        let mut results = Vec::with_capacity(actions.len());
        for action in &actions {
            match self.execute(&action.tool, &action.args, session_id).await {
                Ok(val) => results.push(Ok(val)),
                Err(e) => {
                    results.push(Err(e.clone()));
                    if opts.stop_on_error {
                        break;
                    }
                }
            }
        }
        Ok(results)
    }

    /// Close a single headless session (best-effort) and drop its cache entry.
    /// No-op cost if the session was never opened — agent-browser ignores it.
    pub async fn close_session(&self, session_id: Option<&str>) {
        let session = Self::session_name(session_id);
        self.page_cache.lock().await.remove(&session);
        self.sessions.lock().await.remove(&session);
        let _ = tokio::process::Command::new(&self.binary)
            .args(["--session", &session, "close"])
            .output()
            .await;
        info!(session = %session, "headless browser session closed");
    }

    /// Close every tracked session on shutdown.
    pub async fn cleanup_all(&self) {
        let sessions: Vec<String> = {
            let guard = self.sessions.lock().await;
            guard.iter().cloned().collect()
        };
        for session in sessions {
            self.close_session(Some(&session)).await;
        }
        self.page_cache.lock().await.clear();
    }

    /// Map a web_tool action + args to agent-browser CLI arguments.
    fn map_tool_to_command(&self, tool: &str, args: &Value) -> Result<Vec<String>, String> {
        match tool {
            "navigate" => {
                let url = args
                    .get("url")
                    .and_then(|v| v.as_str())
                    .ok_or("navigate requires 'url' parameter")?;
                Ok(vec!["open".into(), url.into()])
            }
            "read_page" | "snapshot" => {
                let filter = args.get("filter").and_then(|v| v.as_str()).unwrap_or("all");
                let mut cmd = vec!["snapshot".into()];
                if filter == "interactive" {
                    cmd.push("-i".into());
                }
                cmd.push("-c".into()); // compact format
                Ok(cmd)
            }
            "click" => {
                let r = self.extract_ref(args)?;
                Ok(vec!["click".into(), r])
            }
            "double_click" => {
                let r = self.extract_ref(args)?;
                Ok(vec!["dblclick".into(), r])
            }
            "hover" => {
                let r = self.extract_ref(args)?;
                Ok(vec!["hover".into(), r])
            }
            "fill" | "form_input" => {
                let r = self.extract_ref(args)?;
                let value = args
                    .get("value")
                    .and_then(|v| v.as_str())
                    .ok_or("fill requires 'value' parameter")?;
                Ok(vec!["fill".into(), r, value.into()])
            }
            "type" => {
                let text = args
                    .get("text")
                    .and_then(|v| v.as_str())
                    .ok_or("type requires 'text' parameter")?;
                Ok(vec!["type".into(), text.into()])
            }
            "select" => {
                let r = self.extract_ref(args)?;
                let value = args
                    .get("value")
                    .and_then(|v| v.as_str())
                    .ok_or("select requires 'value' parameter")?;
                Ok(vec!["select".into(), r, value.into()])
            }
            "press" | "key" => {
                let key = args
                    .get("key")
                    .and_then(|v| v.as_str())
                    .ok_or("press requires 'key' parameter")?;
                Ok(vec!["press".into(), key.into()])
            }
            "scroll" => {
                let direction = args
                    .get("direction")
                    .and_then(|v| v.as_str())
                    .unwrap_or("down");
                let amount = args.get("amount").and_then(|v| v.as_u64()).unwrap_or(3);
                Ok(vec!["scroll".into(), direction.into(), amount.to_string()])
            }
            "scroll_to" => {
                let r = self.extract_ref(args)?;
                Ok(vec!["scrollintoview".into(), r])
            }
            "screenshot" => Ok(vec!["screenshot".into(), "--json".into()]),
            "evaluate" => {
                let expr = args
                    .get("expression")
                    .and_then(|v| v.as_str())
                    .ok_or("evaluate requires 'expression' parameter")?;
                Ok(vec!["eval".into(), expr.into()])
            }
            "wait" => {
                let ms = args
                    .get("ms")
                    .or_else(|| args.get("time"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(1000);
                Ok(vec!["wait".into(), ms.to_string()])
            }
            "go_back" | "back" => Ok(vec!["back".into()]),
            "go_forward" | "forward" => Ok(vec!["forward".into()]),
            "get_page_text" => Ok(vec!["text".into()]),
            "find" | "find_elements" => {
                let query = args
                    .get("query")
                    .or_else(|| args.get("text"))
                    .and_then(|v| v.as_str())
                    .ok_or("find requires 'query' parameter")?;
                Ok(vec!["find".into(), "text".into(), query.into()])
            }
            "file_upload" | "upload_file" => {
                let r = self.extract_ref(args)?;
                let path = args
                    .get("path")
                    .or_else(|| args.get("file"))
                    .and_then(|v| v.as_str())
                    .ok_or("file_upload requires 'path' parameter")?;
                Ok(vec!["upload".into(), r, path.into()])
            }
            "drag" => {
                let from = args
                    .get("from")
                    .or_else(|| args.get("source"))
                    .and_then(|v| v.as_str())
                    .ok_or("drag requires 'from' parameter")?;
                let to = args
                    .get("to")
                    .or_else(|| args.get("target"))
                    .and_then(|v| v.as_str())
                    .ok_or("drag requires 'to' parameter")?;
                Ok(vec![
                    "drag".into(),
                    ref_to_agent_browser(from),
                    ref_to_agent_browser(to),
                ])
            }
            "close_tab" | "close" => Ok(vec!["close".into()]),
            _ => Err(format!("Unknown browser action '{}'", tool)),
        }
    }

    /// Extract and translate a ref from args.
    fn extract_ref(&self, args: &Value) -> Result<String, String> {
        let raw = args.get("ref").or_else(|| args.get("ref_id"))
            .and_then(|v| v.as_str())
            .ok_or("This action requires a 'ref' parameter. Use read_page first to get element references.")?;
        Ok(ref_to_agent_browser(raw))
    }

    /// Run an agent-browser subprocess and return stdout parsed as JSON or text.
    async fn run_command(&self, session: &str, cmd_args: &[String]) -> Result<Value, String> {
        debug!(session = %session, args = ?cmd_args, "running agent-browser command");

        let output = match tokio::time::timeout(
            Duration::from_secs(30),
            tokio::process::Command::new(&self.binary)
                .args(["--session", session, "--json"])
                .args(cmd_args)
                .output(),
        )
        .await
        {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => return Err(format!("Failed to run agent-browser: {}", e)),
            Err(_) => return Err("agent-browser command timed out after 30s".into()),
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            let msg = if !stderr.is_empty() {
                stderr.trim().to_string()
            } else if !stdout.is_empty() {
                stdout.trim().to_string()
            } else {
                format!("agent-browser exited with status {}", output.status)
            };
            return Err(msg);
        }

        // Try JSON first, fall back to plain text
        let trimmed = stdout.trim();
        if trimmed.starts_with('{') || trimmed.starts_with('[') {
            serde_json::from_str(trimmed)
                .map_err(|e| format!("Failed to parse agent-browser JSON output: {}", e))
        } else {
            Ok(Value::String(trimmed.to_string()))
        }
    }

    /// Normalize agent-browser output to match extension response format.
    fn normalize_response(&self, tool: &str, raw: Value) -> Result<Value, String> {
        match tool {
            "read_page" | "snapshot" => {
                // agent-browser snapshot returns text with @eN refs
                let text = match &raw {
                    Value::String(s) => s.clone(),
                    Value::Object(obj) => {
                        // May return { "content": "..." } in JSON mode
                        obj.get("content")
                            .or_else(|| obj.get("snapshot"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| {
                                serde_json::to_string_pretty(&raw).unwrap_or_default()
                            })
                    }
                    _ => raw.to_string(),
                };
                let translated = refs_agent_browser_to_nebo(&text);
                Ok(serde_json::json!({ "pageContent": translated }))
            }
            "screenshot" => {
                // agent-browser --json screenshot returns { "data": "base64...", "format": "png" }
                if let Value::Object(obj) = &raw {
                    let data = obj
                        .get("data")
                        .or_else(|| obj.get("base64"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let format = obj.get("format").and_then(|v| v.as_str()).unwrap_or("png");
                    Ok(serde_json::json!({
                        "screenshot": {
                            "data": data,
                            "format": format,
                        }
                    }))
                } else {
                    Ok(serde_json::json!({ "screenshot": raw }))
                }
            }
            "evaluate" => Ok(serde_json::json!({ "result": raw })),
            _ => {
                // General: translate refs in text output
                match &raw {
                    Value::String(s) => {
                        let translated = refs_agent_browser_to_nebo(s);
                        Ok(serde_json::json!({ "text": translated }))
                    }
                    _ => Ok(serde_json::json!({ "text": raw })),
                }
            }
        }
    }
}

/// Translate a Nebo ref (`ref_5` or `5`) to agent-browser format (`@e5`).
fn ref_to_agent_browser(r: &str) -> String {
    let num = r.strip_prefix("ref_").unwrap_or(r);
    format!("@e{}", num)
}

/// Translate all agent-browser refs (`@eN`) in text to Nebo format (`ref_N`).
fn refs_agent_browser_to_nebo(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if i + 2 < bytes.len() && bytes[i] == b'@' && bytes[i + 1] == b'e' {
            // Check if followed by digits
            let start = i + 2;
            let mut end = start;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            if end > start {
                result.push_str("ref_");
                result.push_str(&text[start..end]);
                i = end;
                continue;
            }
        }
        result.push(bytes[i] as char);
        i += 1;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ref_to_agent_browser() {
        assert_eq!(ref_to_agent_browser("ref_5"), "@e5");
        assert_eq!(ref_to_agent_browser("5"), "@e5");
        assert_eq!(ref_to_agent_browser("ref_123"), "@e123");
    }

    #[test]
    fn test_refs_agent_browser_to_nebo() {
        assert_eq!(
            refs_agent_browser_to_nebo("button \"Submit\" @e5"),
            "button \"Submit\" ref_5"
        );
        assert_eq!(
            refs_agent_browser_to_nebo("link \"Home\" @e1 href=\"/\"\nbutton @e2"),
            "link \"Home\" ref_1 href=\"/\"\nbutton ref_2"
        );
        // No match
        assert_eq!(refs_agent_browser_to_nebo("no refs here"), "no refs here");
        // @e without digits stays
        assert_eq!(
            refs_agent_browser_to_nebo("email@example.com"),
            "email@example.com"
        );
    }
}
