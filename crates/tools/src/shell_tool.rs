use serde::Deserialize;
use std::process::Stdio;
use std::sync::Arc;
use crate::origin::ToolContext;
use crate::policy::Policy;
use crate::process::{self, ProcessRegistry};
use crate::registry::ToolResult;

/// Shell operations: execute commands, manage processes and background sessions.
pub struct ShellTool {
    _policy: Policy,
    registry: Arc<ProcessRegistry>,
}

#[derive(Debug, Deserialize)]
struct ShellInput {
    #[serde(default)]
    resource: String,
    action: String,
    #[serde(default)]
    command: String,
    #[serde(default)]
    timeout: i64,
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    background: bool,
    #[serde(default)]
    pid: i64,
    #[serde(default)]
    signal: String,
    #[serde(default)]
    filter: String,
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    data: String,
}

impl ShellTool {
    pub fn new(policy: Policy, registry: Arc<ProcessRegistry>) -> Self {
        Self { _policy: policy, registry }
    }

    pub fn name(&self) -> &str {
        "shell"
    }

    pub async fn execute(&self, _ctx: &ToolContext, input: serde_json::Value) -> ToolResult {
        let mut si: ShellInput = match serde_json::from_value(input) {
            Ok(v) => v,
            Err(e) => return ToolResult::error(format!("invalid input: {}", e)),
        };

        // Default resource based on action or input fields
        if si.resource.is_empty() {
            si.resource = match si.action.as_str() {
                "exec" => "bash".to_string(),
                "poll" | "log" | "write" => "session".to_string(),
                _ => {
                    if si.pid > 0 {
                        "process".to_string()
                    } else if !si.session_id.is_empty() {
                        "session".to_string()
                    } else if !si.command.is_empty() {
                        "bash".to_string()
                    } else {
                        String::new()
                    }
                }
            };
        }

        match si.resource.as_str() {
            "bash" | "shell" => self.handle_bash(&si).await,
            "process" => self.handle_process(&si).await,
            "session" => self.handle_session(&si).await,
            other => ToolResult::error(format!(
                "Unknown resource: {} (valid: bash, process, session)",
                other
            )),
        }
    }

    async fn handle_bash(&self, input: &ShellInput) -> ToolResult {
        if input.command.is_empty() {
            return ToolResult::error("Error: command is required");
        }

        // Handle background execution
        if input.background {
            return self.execute_background(input).await;
        }

        let timeout_secs = if input.timeout > 0 {
            input.timeout as u64
        } else {
            120
        };

        let (shell, shell_args) = process::shell_command();
        let mut cmd = tokio::process::Command::new(&shell);
        for arg in &shell_args {
            cmd.arg(arg);
        }
        cmd.arg(&input.command);

        if !input.cwd.is_empty() {
            cmd.current_dir(&input.cwd);
        }

        process::hide_window(&mut cmd);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.env_clear();
        for (k, v) in process::sanitized_env() {
            cmd.env(k, v);
        }

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            cmd.output(),
        )
        .await;

        match result {
            Err(_) => ToolResult {
                content: format!("Command timed out after {}s", timeout_secs),
                is_error: true,
                image_url: None,
            },
            Ok(Err(e)) => ToolResult::error(format!("Command failed: {}", e)),
            Ok(Ok(output)) => {
                let mut result = String::new();

                let stdout = String::from_utf8_lossy(&output.stdout);
                if !stdout.is_empty() {
                    result.push_str(&stdout);
                }

                let stderr = String::from_utf8_lossy(&output.stderr);
                if !stderr.is_empty() {
                    if !result.is_empty() {
                        result.push('\n');
                    }
                    result.push_str("STDERR:\n");
                    result.push_str(&stderr);
                }

                if !output.status.success() {
                    let code = output.status.code().unwrap_or(-1);
                    return ToolResult {
                        content: format!("Command exited with code {}\n{}", code, result),
                        is_error: true,
                        image_url: None,
                    };
                }

                if result.is_empty() {
                    result = "(no output)".to_string();
                }

                // Truncate very long output
                const MAX_OUTPUT: usize = 50000;
                if result.len() > MAX_OUTPUT {
                    result.truncate(MAX_OUTPUT);
                    result.push_str("\n... (output truncated)");
                }

                ToolResult::ok(result)
            }
        }
    }

    async fn execute_background(&self, input: &ShellInput) -> ToolResult {
        let cwd = if input.cwd.is_empty() {
            None
        } else {
            Some(input.cwd.as_str())
        };

        match self.registry.spawn_background(&input.command, cwd).await {
            Ok(session_id) => {
                // Brief pause to see initial output
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;

                if let Some(sess) = self.registry.get_any_session(&session_id).await {
                    let mut result = format!(
                        "Background session started: **{}** (PID {})\n\nCommand: `{}`\n",
                        sess.id, sess.pid, input.command
                    );

                    if sess.exited {
                        let exit_code = sess
                            .exit_code
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "?".to_string());
                        result.push_str(&format!(
                            "\nProcess completed with exit code {}\n",
                            exit_code
                        ));
                        let output = sess.get_output().await;
                        if !output.is_empty() {
                            result.push_str("Output:\n");
                            result.push_str(&output);
                        }
                    } else {
                        result.push_str("\nProcess running in background. Use shell tool with session resource to manage.\n");
                    }

                    ToolResult::ok(result)
                } else {
                    ToolResult::ok(format!("Background session started: {}", session_id))
                }
            }
            Err(e) => ToolResult::error(format!("Failed to start background process: {}", e)),
        }
    }

    async fn handle_process(&self, input: &ShellInput) -> ToolResult {
        match input.action.as_str() {
            "list" => self.list_processes(&input.filter).await,
            "kill" => {
                if input.pid <= 0 {
                    return ToolResult::error("Error: pid is required for kill action");
                }
                self.kill_process(input.pid as u32, &input.signal).await
            }
            "info" => {
                if input.pid <= 0 {
                    return ToolResult::error("Error: pid is required for info action");
                }
                self.process_info(input.pid as u32).await
            }
            other => ToolResult::error(format!(
                "Unknown action for process: {} (valid: list, kill, info)",
                other
            )),
        }
    }

    async fn list_processes(&self, filter: &str) -> ToolResult {
        #[cfg(unix)]
        let cmd_result = tokio::process::Command::new("ps")
            .args(["aux"])
            .output()
            .await;

        #[cfg(windows)]
        let cmd_result = tokio::process::Command::new("tasklist")
            .args(["/V"])
            .output()
            .await;

        match cmd_result {
            Ok(output) => {
                let text = String::from_utf8_lossy(&output.stdout);
                let lines: Vec<&str> = text.lines().collect();
                let mut result = String::new();

                if let Some(header) = lines.first() {
                    result.push_str(header);
                    result.push('\n');
                }

                let filter_lower = filter.to_lowercase();
                let mut count = 0;

                for line in lines.iter().skip(1) {
                    if line.is_empty() {
                        continue;
                    }
                    if !filter.is_empty() && !line.to_lowercase().contains(&filter_lower) {
                        continue;
                    }
                    result.push_str(line);
                    result.push('\n');
                    count += 1;
                    if count >= 50 {
                        result.push_str("\n... (showing first 50 matching processes)");
                        break;
                    }
                }

                if count == 0 && !filter.is_empty() {
                    return ToolResult::ok(format!("No processes found matching: {}", filter));
                }

                ToolResult::ok(result)
            }
            Err(e) => ToolResult::error(format!("Error listing processes: {}", e)),
        }
    }

    async fn kill_process(&self, pid: u32, signal: &str) -> ToolResult {
        #[cfg(unix)]
        {
            use std::process::Command;
            let sig = if signal.is_empty() { "TERM" } else { signal.trim_start_matches("SIG") };
            let result = Command::new("kill")
                .args([&format!("-{}", sig), &pid.to_string()])
                .output();

            match result {
                Ok(output) if output.status.success() => {
                    ToolResult::ok(format!("Sent SIG{} to process {}", sig, pid))
                }
                Ok(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    ToolResult::error(format!("Error killing process {}: {}", pid, stderr.trim()))
                }
                Err(e) => ToolResult::error(format!("Error: {}", e)),
            }
        }

        #[cfg(windows)]
        {
            let result = std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/F"])
                .output();

            match result {
                Ok(output) if output.status.success() => {
                    ToolResult::ok(format!("Killed process {}", pid))
                }
                Ok(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    ToolResult::error(format!("Error killing process {}: {}", pid, stderr.trim()))
                }
                Err(e) => ToolResult::error(format!("Error: {}", e)),
            }
        }
    }

    async fn process_info(&self, pid: u32) -> ToolResult {
        #[cfg(target_os = "macos")]
        let args = vec![
            "-p".to_string(),
            pid.to_string(),
            "-o".to_string(),
            "pid,ppid,user,%cpu,%mem,state,start,time,command".to_string(),
        ];

        #[cfg(target_os = "linux")]
        let args = vec![
            "-p".to_string(),
            pid.to_string(),
            "-o".to_string(),
            "pid,ppid,user,%cpu,%mem,stat,start,time,cmd".to_string(),
        ];

        #[cfg(unix)]
        {
            let output = tokio::process::Command::new("ps")
                .args(&args)
                .output()
                .await;

            match output {
                Ok(o) if o.status.success() => {
                    let text = String::from_utf8_lossy(&o.stdout);
                    ToolResult::ok(format!("Process Information (PID: {})\n{}", pid, text))
                }
                _ => ToolResult::error(format!("Process {} not found", pid)),
            }
        }

        #[cfg(windows)]
        {
            let output = tokio::process::Command::new("tasklist")
                .args(["/FI", &format!("PID eq {}", pid), "/V"])
                .output()
                .await;

            match output {
                Ok(o) if o.status.success() => {
                    let text = String::from_utf8_lossy(&o.stdout);
                    ToolResult::ok(format!("Process Information (PID: {})\n{}", pid, text))
                }
                _ => ToolResult::error(format!("Process {} not found", pid)),
            }
        }
    }

    async fn handle_session(&self, input: &ShellInput) -> ToolResult {
        match input.action.as_str() {
            "list" => self.list_sessions().await,
            "poll" => {
                if input.session_id.is_empty() {
                    return ToolResult::error("Error: session_id is required");
                }
                self.poll_session(&input.session_id).await
            }
            "log" => {
                if input.session_id.is_empty() {
                    return ToolResult::error("Error: session_id is required");
                }
                self.get_session_log(&input.session_id).await
            }
            "write" => {
                if input.session_id.is_empty() {
                    return ToolResult::error("Error: session_id is required");
                }
                self.write_to_session(&input.session_id, &input.data).await
            }
            "kill" => {
                if input.session_id.is_empty() {
                    return ToolResult::error("Error: session_id is required");
                }
                self.kill_session(&input.session_id).await
            }
            other => ToolResult::error(format!(
                "Unknown action for session: {} (valid: list, poll, log, write, kill)",
                other
            )),
        }
    }

    async fn list_sessions(&self) -> ToolResult {
        let running = self.registry.list_running().await;
        let finished = self.registry.list_finished().await;

        if running.is_empty() && finished.is_empty() {
            return ToolResult::ok("No active or recent sessions");
        }

        let mut result = String::new();

        if !running.is_empty() {
            result.push_str("**Running Sessions:**\n");
            for s in &running {
                let cmd_display = if s.command.len() > 50 {
                    format!("{}...", crate::truncate_str(&s.command, 50))
                } else {
                    s.command.clone()
                };
                result.push_str(&format!("- {} (PID {}): `{}`\n", s.id, s.pid, cmd_display));
            }
        }

        if !finished.is_empty() {
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str("**Recent Finished Sessions:**\n");
            for s in &finished {
                let exit_code = s
                    .exit_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "?".to_string());
                let cmd_display = if s.command.len() > 50 {
                    format!("{}...", crate::truncate_str(&s.command, 50))
                } else {
                    s.command.clone()
                };
                result.push_str(&format!(
                    "- {} (exit {}): `{}`\n",
                    s.id, exit_code, cmd_display
                ));
            }
        }

        ToolResult::ok(result)
    }

    async fn poll_session(&self, session_id: &str) -> ToolResult {
        let sess = match self.registry.get_any_session(session_id).await {
            Some(s) => s,
            None => return ToolResult::error(format!("Session not found: {}", session_id)),
        };

        let mut result = format!("Session: {} (PID {})\n", sess.id, sess.pid);

        if sess.exited {
            let exit_code = sess
                .exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "?".to_string());
            result.push_str(&format!("Status: Exited (code {})\n", exit_code));
        } else {
            result.push_str("Status: Running\n");
        }

        let (stdout, stderr) = sess.drain_pending().await;
        if !stdout.is_empty() || !stderr.is_empty() {
            result.push_str("\nNew output:\n");
            if !stdout.is_empty() {
                result.push_str(&String::from_utf8_lossy(&stdout));
            }
            if !stderr.is_empty() {
                if !stdout.is_empty() {
                    result.push_str("\nSTDERR:\n");
                }
                result.push_str(&String::from_utf8_lossy(&stderr));
            }
        } else {
            result.push_str("\n(no new output)");
        }

        ToolResult::ok(result)
    }

    async fn get_session_log(&self, session_id: &str) -> ToolResult {
        match self.registry.get_any_session(session_id).await {
            Some(sess) => {
                let output = sess.get_output().await;
                if output.is_empty() {
                    ToolResult::ok("(no output)")
                } else {
                    ToolResult::ok(output)
                }
            }
            None => ToolResult::error(format!("Session not found: {}", session_id)),
        }
    }

    async fn write_to_session(&self, session_id: &str, data: &str) -> ToolResult {
        match self.registry.write_stdin(session_id, data.as_bytes()).await {
            Ok(()) => ToolResult::ok(format!(
                "Wrote {} bytes to session {}",
                data.len(),
                session_id
            )),
            Err(e) => ToolResult::error(format!("Error writing to session: {}", e)),
        }
    }

    async fn kill_session(&self, session_id: &str) -> ToolResult {
        match self.registry.kill_session(session_id).await {
            Ok(()) => ToolResult::ok(format!("Killed session {}", session_id)),
            Err(e) => ToolResult::error(format!("Error killing session: {}", e)),
        }
    }
}
