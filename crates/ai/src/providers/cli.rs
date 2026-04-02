use async_trait::async_trait;
use serde::Deserialize;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tracing::info;

use crate::types::*;

/// Default HTTP server port where agent MCP tools are served.
const DEFAULT_SERVER_PORT: u16 = 27895;

/// CLI provider wrapping official CLI tools (claude, gemini, codex) as AI providers.
///
/// CLI providers execute tools autonomously via MCP, so `handles_tools()` returns true.
/// The runner skips its own tool execution loop when using a CLI provider.
pub struct CLIProvider {
    name: String,
    command: String,
    args: Vec<String>,
}

impl CLIProvider {
    /// Create a provider that wraps the Claude Code CLI.
    ///
    /// Claude Code connects to Nebo's agent MCP server for tool access.
    /// All built-in Claude Code tools are disabled so it only uses Nebo's STRAP tools.
    /// `server_port` is the HTTP server port (0 = default 27895).
    /// `max_turns` caps multi-turn tool use (0 = unlimited).
    pub fn new_claude_code(max_turns: u32, server_port: u16) -> Self {
        let port = if server_port == 0 {
            DEFAULT_SERVER_PORT
        } else {
            server_port
        };

        let mcp_config = format!(
            r#"{{"mcpServers":{{"nebo-agent":{{"type":"http","url":"http://localhost:{}/agent/mcp"}}}}}}"#,
            port
        );

        let mut args = vec![
            "--print".to_string(),
            "--verbose".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--include-partial-messages".to_string(),
            "--dangerously-skip-permissions".to_string(),
            "--tools".to_string(),
            "".to_string(), // Disable ALL built-in tools
            "--mcp-config".to_string(),
            mcp_config,
            "--strict-mcp-config".to_string(),
            "--allowedTools".to_string(),
            "mcp__nebo-agent__*".to_string(),
        ];

        if max_turns > 0 {
            args.push("--max-turns".to_string());
            args.push(max_turns.to_string());
        }

        Self {
            name: "claude-code".to_string(),
            command: "claude".to_string(),
            args,
        }
    }

    /// Create a provider that wraps the Google Gemini CLI.
    /// Gemini CLI reads from stdin.
    pub fn new_gemini_cli() -> Self {
        Self {
            name: "gemini-cli".to_string(),
            command: "gemini".to_string(),
            args: Vec::new(),
        }
    }

    /// Create a provider that wraps the OpenAI Codex CLI.
    /// Uses `--full-auto` for autonomous mode.
    pub fn new_codex_cli() -> Self {
        Self {
            name: "codex-cli".to_string(),
            command: "codex".to_string(),
            args: vec!["--full-auto".to_string()],
        }
    }
}

#[async_trait]
impl Provider for CLIProvider {
    fn id(&self) -> &str {
        &self.name
    }

    fn handles_tools(&self) -> bool {
        true
    }

    async fn stream(&self, req: &ChatRequest) -> Result<EventReceiver, ProviderError> {
        let prompt = build_prompt_from_messages(&req.messages);

        let mut args = self.args.clone();

        // Add model flag if specified
        if !req.model.is_empty() && (self.name == "claude-code" || self.name == "codex-cli") {
            args.push("--model".to_string());
            args.push(req.model.clone());
        }

        // Control thinking effort for Claude
        if self.command == "claude" {
            if req.enable_thinking {
                args.push("--effort".to_string());
                args.push("high".to_string());
            } else {
                args.push("--effort".to_string());
                args.push("low".to_string());
            }
        }

        info!(
            command = %self.command,
            prompt_len = prompt.len(),
            system_len = req.system.len(),
            thinking = req.enable_thinking,
            "running CLI command"
        );

        // Build stdin content: system prompt + user prompt
        let stdin_content = if !req.system.is_empty() && self.command == "claude" {
            format!("[System]\n{}\n\n{}", req.system, prompt)
        } else {
            prompt
        };

        let mut cmd = tokio::process::Command::new(&self.command);
        cmd.args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Windows: suppress console window flash for GUI app
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        // Unix: set process group for clean shutdown
        #[cfg(unix)]
        {
            unsafe {
                cmd.pre_exec(|| {
                    libc::setpgid(0, 0);
                    Ok(())
                });
            }
        }

        let mut child = cmd.spawn().map_err(|e| {
            ProviderError::Request(format!("failed to start {}: {}", self.command, e))
        })?;

        info!(command = %self.command, "CLI process started");

        // Write stdin
        if let Some(mut stdin) = child.stdin.take() {
            let content = stdin_content.clone();
            tokio::spawn(async move {
                let _ = stdin.write_all(content.as_bytes()).await;
                let _ = stdin.shutdown().await;
            });
        }

        let (tx, rx) = mpsc::channel(100);

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let command_name = self.command.clone();
        let enable_thinking = req.enable_thinking;
        let cancel_token = req.cancel_token.clone();

        tokio::spawn(async move {
            // Read stderr in background
            let stderr_handle = if let Some(stderr) = stderr {
                let handle = tokio::spawn(async move {
                    let mut reader = BufReader::new(stderr);
                    let mut output = String::new();
                    let _ = tokio::io::AsyncReadExt::read_to_string(&mut reader, &mut output).await;
                    output.trim().to_string()
                });
                Some(handle)
            } else {
                None
            };

            // Stream stdout line by line
            if let Some(stdout) = stdout {
                let reader = BufReader::new(stdout);
                let mut lines = reader.lines();

                // Tool state tracking for accumulated input
                let mut pending_tool: Option<PendingToolCall> = None;

                loop {
                    let line = if let Some(ref token) = cancel_token {
                        tokio::select! {
                            _ = token.cancelled() => {
                                info!(command = %command_name, "CLI process cancelled — killing child");
                                kill_child_process(&mut child);
                                let _ = tx.send(StreamEvent::error("Cancelled".to_string())).await;
                                break;
                            }
                            result = lines.next_line() => match result {
                                Ok(Some(line)) => line,
                                _ => break,
                            }
                        }
                    } else {
                        match lines.next_line().await {
                            Ok(Some(line)) => line,
                            _ => break,
                        }
                    };
                    if line.is_empty() {
                        continue;
                    }

                    // Pre-parse to intercept tool-related streaming events
                    if let Ok(mut raw) = serde_json::from_str::<serde_json::Value>(&line) {
                        let mut event_type = raw
                            .get("type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();

                        // Unwrap stream_event envelope
                        if event_type == "stream_event" || raw.get("event").is_some() {
                            if let Some(inner) = raw.get("event").cloned() {
                                event_type = inner
                                    .get("type")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                raw = inner;
                            }
                        }

                        match event_type.as_str() {
                            "content_block_start" => {
                                if let Some(block) = raw.get("content_block") {
                                    let block_type =
                                        block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                                    if block_type == "tool_use" {
                                        let id = block
                                            .get("id")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        let name = block
                                            .get("name")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        pending_tool = Some(PendingToolCall {
                                            id,
                                            name,
                                            input: String::new(),
                                        });
                                        continue;
                                    }
                                    if block_type == "thinking" && !enable_thinking {
                                        continue;
                                    }
                                }
                            }
                            "content_block_delta" => {
                                if let Some(delta) = raw.get("delta") {
                                    let delta_type = delta
                                        .get("type")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("");
                                    if delta_type == "input_json_delta" {
                                        if let Some(ref mut tool) = pending_tool {
                                            if let Some(partial) =
                                                delta.get("partial_json").and_then(|v| v.as_str())
                                            {
                                                tool.input.push_str(partial);
                                            }
                                        }
                                        continue;
                                    }
                                    if delta_type == "text_delta" {
                                        if let Some(text) =
                                            delta.get("text").and_then(|v| v.as_str())
                                        {
                                            if !text.is_empty() {
                                                let _ = tx.send(StreamEvent::text(text)).await;
                                            }
                                        }
                                        continue;
                                    }
                                    if delta_type == "thinking_delta" {
                                        if enable_thinking {
                                            if let Some(text) =
                                                delta.get("thinking").and_then(|v| v.as_str())
                                            {
                                                let _ =
                                                    tx.send(StreamEvent::thinking(text)).await;
                                            }
                                        }
                                        continue;
                                    }
                                }
                            }
                            "content_block_stop" => {
                                if let Some(tool) = pending_tool.take() {
                                    let input_json = if tool.input.is_empty() {
                                        "{}".to_string()
                                    } else {
                                        tool.input
                                    };
                                    let input: serde_json::Value =
                                        serde_json::from_str(&input_json)
                                            .unwrap_or(serde_json::json!({}));
                                    let _ = tx
                                        .send(StreamEvent::tool_call(ToolCall {
                                            id: tool.id,
                                            name: tool.name,
                                            input,
                                        }))
                                        .await;
                                }
                                continue;
                            }
                            "result" => {
                                let subtype = raw
                                    .get("subtype")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                if subtype == "success" || subtype == "error_max_turns" {
                                    let _ = tx.send(StreamEvent::done()).await;
                                    break;
                                }
                                if let Some(result) =
                                    raw.get("result").and_then(|v| v.as_str())
                                {
                                    let _ = tx.send(StreamEvent::text(result)).await;
                                }
                                continue;
                            }
                            "error" => {
                                let msg = raw
                                    .get("error")
                                    .map(|v| format!("{}", v))
                                    .unwrap_or_else(|| "CLI error".to_string());
                                let _ = tx.send(StreamEvent::error(msg)).await;
                                continue;
                            }
                            "message_delta" | "message_stop" | "message_start"
                            | "ping" | "system" => {
                                continue;
                            }
                            _ => {}
                        }
                    }
                }
            }

            // Wait for stderr
            let stderr_output = if let Some(handle) = stderr_handle {
                handle.await.unwrap_or_default()
            } else {
                String::new()
            };

            // Wait for process to finish
            let exit_status = child.wait().await;
            info!(command = %command_name, status = ?exit_status, "CLI command finished");

            if let Ok(status) = exit_status {
                if !status.success() {
                    let mut err_msg = format!("{} exited with error", command_name);
                    if !stderr_output.is_empty() {
                        err_msg = format!("{}: {}", err_msg, stderr_output);
                    }
                    let _ = tx.send(StreamEvent::error(err_msg)).await;
                }
            }

            let _ = tx.send(StreamEvent::done()).await;
        });

        Ok(rx)
    }
}

/// Kill a CLI child process and its entire process group.
fn kill_child_process(child: &mut tokio::process::Child) {
    // Try SIGTERM on the process group first (graceful)
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            unsafe {
                // Negative PID sends to the process group
                libc::kill(-(pid as i32), libc::SIGTERM);
            }
            // Give it 100ms then force kill
            let _ = child.start_kill();
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.start_kill();
    }
}

/// Accumulated tool call state during streaming.
struct PendingToolCall {
    id: String,
    name: String,
    input: String,
}

/// Build a single prompt string from messages.
/// Merges consecutive same-role messages to avoid fragmentation.
fn build_prompt_from_messages(messages: &[Message]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut last_role = String::new();
    let mut pending_content = String::new();

    let flush = |parts: &mut Vec<String>, role: &str, content: &str| {
        if content.is_empty() {
            return;
        }
        let prefix = match role {
            "system" => "[System]",
            "user" | "tool" => "[User]",
            "assistant" => "[Assistant]",
            _ => "[User]",
        };
        parts.push(format!("{}\n{}", prefix, content));
    };

    for msg in messages {
        let role = if msg.role == "tool" {
            "user"
        } else {
            &msg.role
        };
        let mut content = msg.content.trim().to_string();

        // Handle tool results inline
        if let Some(ref tr_val) = msg.tool_results {
            if let Ok(results) = serde_json::from_value::<Vec<ToolResultEntry>>(tr_val.clone()) {
                for r in &results {
                    if !r.content.is_empty() {
                        if !content.is_empty() {
                            content.push('\n');
                        }
                        content.push_str(&format!(
                            "[Tool Result: {}]\n{}",
                            r.tool_call_id, r.content
                        ));
                    }
                }
            }
        }

        if content.is_empty() {
            continue;
        }

        if role == last_role {
            if !pending_content.contains(&content) {
                pending_content.push_str("\n\n");
                pending_content.push_str(&content);
            }
        } else {
            flush(&mut parts, &last_role, &pending_content);
            last_role = role.to_string();
            pending_content = content;
        }
    }

    flush(&mut parts, &last_role, &pending_content);
    parts.join("\n\n")
}

#[derive(Debug, Deserialize)]
struct ToolResultEntry {
    tool_call_id: String,
    content: String,
    /// Present in JSON payload but not read directly — needed for deserialization.
    #[serde(default)]
    _is_error: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_build_prompt() {
        let messages = vec![
            Message {
                role: "user".to_string(),
                content: "Hello".to_string(),
                ..Default::default()
            },
            Message {
                role: "assistant".to_string(),
                content: "Hi there!".to_string(),
                ..Default::default()
            },
            Message {
                role: "user".to_string(),
                content: "How are you?".to_string(),
                ..Default::default()
            },
        ];

        let prompt = build_prompt_from_messages(&messages);
        assert!(prompt.contains("[User]\nHello"));
        assert!(prompt.contains("[Assistant]\nHi there!"));
        assert!(prompt.contains("[User]\nHow are you?"));
    }

    #[test]
    fn test_cli_merge_consecutive() {
        let messages = vec![
            Message {
                role: "user".to_string(),
                content: "Part 1".to_string(),
                ..Default::default()
            },
            Message {
                role: "user".to_string(),
                content: "Part 2".to_string(),
                ..Default::default()
            },
        ];

        let prompt = build_prompt_from_messages(&messages);
        // Should merge into a single [User] block
        let user_count = prompt.matches("[User]").count();
        assert_eq!(user_count, 1, "Expected 1 [User] block, got {}: {}", user_count, prompt);
    }

    #[test]
    fn test_cli_tool_accumulation() {
        let messages = vec![
            Message {
                role: "user".to_string(),
                content: "Read the file".to_string(),
                ..Default::default()
            },
            Message {
                role: "tool".to_string(),
                content: String::new(),
                tool_results: Some(serde_json::json!([{
                    "tool_call_id": "call_1",
                    "content": "file contents here",
                    "is_error": false
                }])),
                ..Default::default()
            },
        ];

        let prompt = build_prompt_from_messages(&messages);
        assert!(prompt.contains("[Tool Result: call_1]"));
        assert!(prompt.contains("file contents here"));
    }
}
