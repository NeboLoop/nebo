use crate::errors;
use crate::origin::ToolContext;
use crate::policy::Policy;
use crate::process::{self, ProcessRegistry};
use crate::registry::ToolResult;
use serde::Deserialize;
use std::process::Stdio;
use std::sync::Arc;

/// Shell operations: execute commands, manage processes and background sessions.
pub struct ShellTool {
    _policy: Policy,
    registry: Arc<ProcessRegistry>,
    plugin_store: Option<Arc<napp::plugin::PluginStore>>,
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
        Self {
            _policy: policy,
            registry,
            plugin_store: None,
        }
    }

    pub fn with_plugin_store(mut self, ps: Arc<napp::plugin::PluginStore>) -> Self {
        self.plugin_store = Some(ps);
        self
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
            return ToolResult::error(errors::missing_param(
                "exec",
                "command",
                "shell(action: \"exec\", command: \"ls -la\")",
            ));
        }

        // Document conversion has ONE canonical pathway — the embedded Typst
        // engine behind `os(file convert)`. Host converter binaries only exist
        // on some machines (wkhtmltopdf is abandoned upstream), so shelling out
        // to them produces runs that work on the developer's laptop and fail on
        // every customer install. Redirect instead of executing.
        {
            let cmd_head = input.command.trim_start();
            const HOST_CONVERTERS: &[&str] = &["wkhtmltopdf", "weasyprint", "pandoc", "wkhtmltoimage"];
            if HOST_CONVERTERS
                .iter()
                .any(|c| cmd_head.starts_with(c) && cmd_head[c.len()..].starts_with([' ', '\t']))
            {
                return ToolResult::error(
                    "Host document converters are not available on user machines. \
                     Convert documents with the built-in engine instead: write the document \
                     as Markdown, then os(resource: \"file\", action: \"convert\", \
                     path: \"/path/doc.md\", to: \"pdf\"). It typesets identically on every \
                     platform and the PDF appears in the Work panel automatically.",
                );
            }
        }

        // Privilege escalation is never a legitimate automation step: Nebo runs
        // unattended, so sudo either hangs on a password prompt or silently
        // escalates. Refuse before anything executes (covers background too).
        if crate::policy::is_privilege_escalation(&input.command) {
            return ToolResult::error(
                "Privilege escalation (sudo/doas/su) is not available — Nebo runs \
                 unattended and cannot enter passwords or hold admin rights. Do not \
                 retry with sudo. Instead: use a user-writable location, or tell the \
                 user this operation requires administrator privileges and they need \
                 to perform it themselves."
                    .to_string(),
            );
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
            let cwd_path = std::path::Path::new(&input.cwd);
            if !cwd_path.exists() {
                return ToolResult::error(errors::path_not_found(&input.cwd));
            }
            if !cwd_path.is_dir() {
                return ToolResult::error(format!(
                    "Not a directory: {}. The cwd parameter must be a directory path.",
                    input.cwd
                ));
            }
            cmd.current_dir(&input.cwd);
        }

        process::hide_window(&mut cmd);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.env_clear();
        for (k, v) in process::sanitized_env() {
            cmd.env(k, v);
        }
        if let Some(ref ps) = self.plugin_store {
            for (k, v) in ps.build_env_map() {
                cmd.env(k, v);
            }
            cmd.env("PATH", ps.path_with_plugins());
        }

        let started = std::time::SystemTime::now();
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), cmd.output()).await;

        match result {
            Err(_) => ToolResult {
                content: format!(
                    "Command timed out after {}s: `{}`\n\
                     The command did not complete within the timeout. \
                     Try a shorter operation, a more specific path, or increase the timeout parameter.",
                    timeout_secs,
                    if input.command.len() > 80 {
                        format!("{}...", crate::truncate_str(&input.command, 80))
                    } else {
                        input.command.clone()
                    }
                ),
                is_error: true,
                image_url: None,
                http_status: None,
                terminal: false,
            },
            Ok(Err(e)) => {
                let err_str = e.to_string();
                if err_str.contains("No such file or directory") || err_str.contains("not found") {
                    let base_cmd = extract_base_command(&input.command);
                    ToolResult::error(errors::command_not_found(&base_cmd))
                } else if err_str.contains("Permission denied") {
                    ToolResult::error(errors::permission_denied(&input.command, "execute"))
                } else {
                    ToolResult::error(format!("Command failed to start: {}", e))
                }
            }
            Ok(Ok(output)) => {
                let mut result = String::new();

                let stdout = String::from_utf8_lossy(&output.stdout);
                if !stdout.is_empty() {
                    result.push_str(&stdout);
                }

                let stderr = String::from_utf8_lossy(&output.stderr);
                #[cfg(target_os = "windows")]
                let stderr: std::borrow::Cow<'_, str> =
                    std::borrow::Cow::Owned(process::clean_powershell_stderr(&stderr));
                if !stderr.is_empty() {
                    if !result.is_empty() {
                        result.push('\n');
                    }
                    result.push_str("STDERR:\n");
                    result.push_str(&stderr);
                }

                if !output.status.success() {
                    let code = output.status.code().unwrap_or(-1);
                    let (is_error, semantic_msg) =
                        interpret_exit_code(&input.command, code, &result);
                    if let Some(msg) = semantic_msg {
                        if !result.is_empty() {
                            result.push('\n');
                        }
                        result.push_str(&msg);
                    }
                    if is_error {
                        return ToolResult {
                            content: format!("Command exited with code {}\n{}", code, result),
                            is_error: true,
                            image_url: None,
                            http_status: None,
                            terminal: false,
                        };
                    }
                    // Non-error exit (e.g. grep exit 1 = no matches) — fall through to success path
                }

                if result.is_empty() {
                    result = "(no output)".to_string();
                }

                // Truncate very long output (char-boundary safe)
                const MAX_OUTPUT: usize = 50000;
                if result.len() > MAX_OUTPUT {
                    let total_len = result.len();
                    let total_lines = result.lines().count();

                    // Persist full output to disk
                    let output_dir = dirs::data_dir()
                        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
                        .join("nebo/shell_output");
                    let _ = std::fs::create_dir_all(&output_dir);
                    let filename = format!("cmd_{}.txt", uuid::Uuid::new_v4().as_simple());
                    let output_path = output_dir.join(&filename);
                    let persisted = std::fs::write(&output_path, &result).is_ok();

                    // Truncate for inline result
                    types::strutil::safe_truncate(&mut result, MAX_OUTPUT);

                    if persisted {
                        result.push_str(&format!(
                            "\n\n--- Full output ({} chars, {} lines) saved to: {}\n\
                             Read sections with: os(resource: \"file\", action: \"read\", path: \"{}\", offset: N, limit: M)",
                            total_len, total_lines,
                            output_path.display(), output_path.display(),
                        ));
                    } else {
                        let removed_kb = (total_len - MAX_OUTPUT) / 1024;
                        result.push_str(&format!(
                            "\n... [output truncated — showing first 50000 of {} chars, {}KB removed. \
                             Use grep to search for specific content, or pipe through head/tail.]",
                            total_len, removed_kb
                        ));
                    }
                }

                // A command that produced a work document (`python gen.py -o report.pdf`,
                // `nebo-office pptx create … -o deck.pptx`) surfaces it exactly like an
                // `os` write — same gate the plugin exec pathway uses.
                let tokens = shlex::split(&input.command).unwrap_or_default();
                let base = (!input.cwd.is_empty()).then(|| std::path::Path::new(&input.cwd));
                let result = ToolResult::ok(result);
                match crate::plugin_tool::produced_work_document(&tokens, base, started) {
                    Some(path) => result.with_image_url(path),
                    None => result,
                }
            }
        }
    }

    async fn execute_background(&self, input: &ShellInput) -> ToolResult {
        let cwd = if input.cwd.is_empty() {
            None
        } else {
            Some(input.cwd.as_str())
        };

        let plugin_envs = self
            .plugin_store
            .as_ref()
            .map(|ps| ps.build_env_map())
            .unwrap_or_default();

        match self
            .registry
            .spawn_background(&input.command, cwd, &plugin_envs)
            .await
        {
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
            let sig = if signal.is_empty() {
                "TERM"
            } else {
                signal.trim_start_matches("SIG")
            };
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
            let _ = signal;
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

/// Extract the base command name from a (possibly piped) command string.
/// Uses the LAST segment in a pipeline, since that determines the exit code.
fn extract_base_command(command: &str) -> String {
    let last_segment = command.rsplit('|').next().unwrap_or(command);
    last_segment
        .trim()
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string()
}

/// Interpret a command's exit code using command-specific semantics.
/// Returns (is_error, optional_message).
fn interpret_exit_code(command: &str, exit_code: i32, output: &str) -> (bool, Option<String>) {
    let base = extract_base_command(command);
    match base.as_str() {
        // grep/rg: 0=matches found, 1=no matches, 2+=error
        "grep" | "rg" | "egrep" | "fgrep" => {
            if exit_code == 1 {
                (false, Some("No matches found. This is not an error — the pattern does not appear in the searched files. Do not retry the same search.".to_string()))
            } else {
                (true, None)
            }
        }
        // diff: 0=identical, 1=differences found, 2+=error
        "diff" | "colordiff" => {
            if exit_code == 1 {
                (false, Some("Files differ.".to_string()))
            } else {
                (true, None)
            }
        }
        // find: 0=success, 1=some dirs inaccessible (partial), 2+=error
        "find" | "fd" => {
            if exit_code == 1 {
                (false, Some("Some directories were inaccessible.".to_string()))
            } else {
                (true, None)
            }
        }
        // test/[: 0=true, 1=false, 2+=error
        "test" | "[" => {
            if exit_code == 1 {
                (false, Some("Condition is false.".to_string()))
            } else {
                (true, None)
            }
        }
        // Generic command (no exit-code convention): surface the *cause* from stderr so
        // the model diagnoses instead of spiraling. A misleading error is what starts a
        // search/retry loop — e.g. `convert image.png …` fails with an IMv7 deprecation
        // banner that buries "unable to open image", and the model goes hunting for a png
        // across the disk. The original output is kept; we only append a one-line hint.
        _ => {
            let lo = output.to_lowercase();
            let hint = if lo.contains("command not found")
                || lo.contains("not recognized as")
                || lo.contains(&format!("{}: not found", base))
            {
                Some(format!(
                    "The command '{}' is not available on this system. Tell the user it isn't \
                     installed — do not substitute another command or install it without asking.",
                    base
                ))
            } else if lo.contains("no such file")
                || lo.contains("unable to open")
                || lo.contains("cannot open")
                || lo.contains("does not exist")
            {
                Some(
                    "An input the command needs was not found. Verify the exact path or ask the \
                     user for it — do not search the filesystem for a substitute."
                        .to_string(),
                )
            } else {
                None
            };
            (true, hint)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::origin::Origin;
    use serde_json::json;

    fn tool() -> ShellTool {
        ShellTool::new(Policy::default(), Arc::new(ProcessRegistry::new()))
    }

    fn ctx() -> ToolContext {
        ToolContext::new(Origin::User)
    }

    // Privilege escalation never executes — foreground or background — and the
    // refusal steers toward reporting to the user, not retrying.
    #[tokio::test]
    async fn privilege_escalation_is_refused() {
        let t = tool();
        for input in [
            json!({"action": "exec", "command": "sudo whoami"}),
            json!({"action": "exec", "command": "echo hi | sudo tee /var/root/f"}),
            json!({"action": "exec", "command": "doas id", "background": true}),
        ] {
            let res = t.execute(&ctx(), input.clone()).await;
            assert!(res.is_error, "must refuse: {}", input);
            assert!(
                res.content.contains("not available"),
                "refusal must explain: {}",
                res.content
            );
        }
    }

    #[tokio::test]
    async fn plain_commands_still_execute() {
        let t = tool();
        let res = t
            .execute(&ctx(), json!({"action": "exec", "command": "echo nebo-ok"}))
            .await;
        assert!(!res.is_error, "plain echo failed: {}", res.content);
        assert!(res.content.contains("nebo-ok"));
    }
}
