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
    /// Machine-consumer mode: on success return stdout ONLY (no STDERR
    /// section, no "(no output)" placeholder, no truncation footer); on a
    /// non-zero exit return an error carrying stderr. Used by deterministic
    /// workflow nodes whose output is parsed, not read by a model.
    #[serde(default)]
    raw: bool,
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

    pub async fn execute(&self, ctx: &ToolContext, input: serde_json::Value) -> ToolResult {
        let mut si: ShellInput = match serde_json::from_value(input) {
            Ok(v) => v,
            Err(e) => {
                return ToolResult::error(format!(
                    "invalid input: {e}. Shape: os(resource: \"shell\", action: \"exec\", command: \"...\") \
                     with optional timeout (seconds), cwd, background: true"
                ))
            }
        };

        // The os tool stamps resource "shell" on every call, and the model
        // never sees the internal bash/process/session split, so "shell" and
        // an empty resource both route by action and parameters. Until
        // 2026-09-05 "shell" went straight to exec, and poll/log/kill through
        // os answered "exec requires command": every background job was a
        // dead end.
        if si.resource.is_empty() || si.resource == "shell" {
            si.resource = Self::route_for(&si).to_string();
        }

        match si.resource.as_str() {
            "bash" => self.handle_bash(&si, ctx.trusted_plugin_env, ctx.cwd.as_deref()).await,
            "process" => self.handle_process(&si).await,
            "session" => self.handle_session(&si).await,
            other => ToolResult::error(format!(
                "Unknown shell action '{}'{}. Valid: exec, list, poll, log, write, kill, info",
                si.action,
                if other.is_empty() { String::new() } else { format!(" (resource '{other}')") }
            )),
        }
    }

    /// The handler an action belongs to. `exec` runs a command; `poll`, `log`
    /// and `write` manage a background session; `kill`, `info` and `list`
    /// take a `pid` (system process) or a `session_id` (background session),
    /// and a bare `list` is the session list (`filter` asks for processes).
    /// Anything else routes by the parameter that is present, so an unknown
    /// action still gets the error that names the right shape.
    fn route_for(si: &ShellInput) -> &'static str {
        match si.action.as_str() {
            "exec" => "bash",
            "poll" | "log" | "write" => "session",
            "kill" | "info" if si.pid > 0 => "process",
            "kill" | "info" => "session",
            "list" if si.pid > 0 || !si.filter.is_empty() => "process",
            "list" => "session",
            _ if si.pid > 0 => "process",
            _ if !si.session_id.is_empty() => "session",
            _ if !si.command.is_empty() => "bash",
            _ => "",
        }
    }

    async fn handle_bash(&self, input: &ShellInput, trusted_plugin_env: bool, default_cwd: Option<&str>) -> ToolResult {
        if input.command.is_empty() {
            return ToolResult::error(errors::missing_param(
                "exec",
                "command",
                "os(resource: \"shell\", action: \"exec\", command: \"ls -la\")",
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

        // A follow/watch with no bound parks the run for the WHOLE timeout and
        // returns nothing useful. Observed live 2026-08-27: an agent told to
        // poll a log reached for `tail -f … | grep READY` with timeout 300 and
        // sat there for five minutes producing no output. Nebo runs unattended,
        // so there is no one to Ctrl-C it. Refuse and teach the bounded form —
        // same shape as the host-converter redirect above.
        if let Some(flag) = detect_unbounded_follow(&input.command) {
            return ToolResult::error(format!(
                "`{flag}` follows output forever and would block this call for its \
                 entire timeout without returning anything. Take a bounded snapshot \
                 instead — e.g. `tail -n 50 <file>`, `docker compose logs --tail 50`, \
                 or `journalctl -n 50` — and call again if you need a later view. If \
                 you genuinely need to follow, bound it explicitly with `timeout N …` \
                 or pipe through `head -n N`."
            ));
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

        // House git rules: the commands that throw away the owner's work are
        // refused outright, and the refusal names what to use instead.
        if crate::policy::is_destructive_git(&input.command) {
            return ToolResult::error(
                "This git command discards work (stash, reset --hard, checkout/restore of \
                 tracked files, clean -f, force push, branch -D) and is not available. \
                 To be able to undo a change, take a checkpoint first: os(resource: \
                 \"file\", action: \"checkpoint\", paths: [...]) and restore it with \
                 action: \"restore\". For parallel edits use a worktree (agent spawn_parallel \
                 with isolate: \"worktree\"). If the owner truly wants history rewritten, \
                 tell them the exact command and let them run it."
                    .to_string(),
            );
        }

        // `sed -i` rewrites a file behind the read ledger and the edit
        // verification chain. The edit action is the supervised way to change
        // a file; a refusal that names it beats a silent unsupervised write.
        if crate::policy::is_sed_in_place(&input.command) {
            return ToolResult::error(
                "In-place sed is not available: it edits a file outside the supervised edit \
                 path (no read check, no verification, no ledger). Use os(resource: \"file\", \
                 action: \"edit\", path, old_string, new_string) for the same change, or \
                 replace_all: true for every occurrence. Plain `sed` that prints to stdout is fine."
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

        // The call's own cwd wins; otherwise the run's default (an isolated
        // sub-agent's worktree), so a bare `cargo test` never runs in the
        // owner's tree by accident.
        let cwd = if input.cwd.is_empty() { default_cwd.unwrap_or("") } else { input.cwd.as_str() };
        if !cwd.is_empty() {
            let cwd_path = std::path::Path::new(cwd);
            if !cwd_path.exists() {
                return ToolResult::error(errors::path_not_found(cwd));
            }
            if !cwd_path.is_dir() {
                return ToolResult::error(format!(
                    "Not a directory: {}. The cwd parameter must be a directory path.",
                    cwd
                ));
            }
            cmd.current_dir(cwd);
        }

        process::hide_window(&mut cmd);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.env_clear();
        for (k, v) in process::sanitized_env() {
            cmd.env(k, v);
        }
        // An unattended agent can never answer a credential prompt: a `git
        // fetch` on an uncached remote would hang the turn until timeout.
        cmd.env("GIT_TERMINAL_PROMPT", "0");
        cmd.env("GIT_ASKPASS", "");
        cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
        if let Some(ref ps) = self.plugin_store {
            for (k, v) in ps.build_env_map() {
                cmd.env(k, v);
            }
            cmd.env("PATH", ps.path_with_plugins());
            // Workflow command nodes (and ONLY them) also get plugin auth env,
            // so `${plugin.X_BIN}` invocations of env-auth plugins work — the
            // gap that made `odoo doctor` read writes_enabled=false inside
            // commit-state and wrongfully demote real writes.
            if trusted_plugin_env {
                for (k, v) in ps.build_auth_env_map() {
                    cmd.env(k, v);
                }
            }
        }

        let started = std::time::SystemTime::now();
        // Kill the child if the timeout drops the output() future. Without this
        // the process outlives the call FOREVER — it reparents to launchd/init
        // and never exits. Same defect class that accumulated 330 orphaned
        // plugin processes on a customer box (see PluginRuntime::run_capture);
        // a never-exiting command like `dns-sd -B` under the default timeout
        // leaked its process on every single invocation.
        // The command's whole process group ends with the call (completion,
        // timeout or a cancelled turn): a server it started would otherwise
        // reparent to init and run forever. Servers belong in a background session.
        let result = crate::process::output_within(cmd, std::time::Duration::from_secs(timeout_secs)).await;

        match result {
            Ok(None) => ToolResult { payload: None,
                content: format!(
                    "Command timed out after {}s: `{}`\n\
                     The command did not complete within the timeout. \
                     The process was killed and its partial output discarded. \
                     Try a shorter operation, a more specific path, or increase the timeout parameter. \
                     For a server or long job use background: true and poll with action: \"poll\".",
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
            Err(e) => {
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
            Ok(Some(output)) => {
                if input.raw {
                    if !output.status.success() {
                        return ToolResult::error(format!(
                            "{}\n{}",
                            exit_header(&output.status),
                            String::from_utf8_lossy(&output.stderr)
                        ));
                    }
                    return ToolResult::ok(
                        String::from_utf8_lossy(&output.stdout).into_owned(),
                    );
                }
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
                        return ToolResult { payload: None,
                            content: format!("{}\n{}", exit_header(&output.status), result),
                            is_error: true,
                            image_url: None,
                            http_status: None,
                            terminal: false,
                        };
                    }
                    // Non-error exit (e.g. grep exit 1 = no matches) — fall through to success path
                }

                if result.is_empty() {
                    result = "(exit 0, no output)".to_string();
                }

                // Truncate very long output (char-boundary safe)
                if result.len() > crate::MAX_SUBPROCESS_OUTPUT {
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
                    types::strutil::safe_truncate(&mut result, crate::MAX_SUBPROCESS_OUTPUT);

                    if persisted {
                        result.push_str(&format!(
                            "\n\n--- Showing the first 50,000 of {} bytes ({} lines). Full output (stdout, then STDERR section) saved to: {}\n\
                             Read sections with: os(resource: \"file\", action: \"read\", path: \"{}\", offset: N, limit: M)",
                            total_len, total_lines,
                            output_path.display(), output_path.display(),
                        ));
                    } else {
                        let removed = total_len - crate::MAX_SUBPROCESS_OUTPUT;
                        result.push_str(&format!(
                            "\n... [output truncated: showing the first 50,000 of {} bytes; {} bytes not shown. \
                             Use grep to search for specific content, or pipe through head/tail.]",
                            total_len, removed
                        ));
                    }
                }

                // A command that produced a work document (`python gen.py -o report.pdf`,
                // `nebo-office pptx create … -o deck.pptx`) surfaces it exactly like an
                // `os` write — same gate the plugin exec pathway uses.
                // shlex chokes on quote-heavy commands (an HTML heredoc has
                // apostrophes everywhere) and returned ZERO tokens — which made
                // exactly the runs that write big documents the ones whose
                // documents were never detected (observed live 2026-08-28: a
                // dashboard.html heredoc left the Work panel empty). Fall back
                // to whitespace tokens so redirect targets still surface.
                let tokens = shlex::split(&input.command).unwrap_or_else(|| {
                    input
                        .command
                        .split_whitespace()
                        .map(|t| t.trim_matches(|c| c == '"' || c == '\'' || c == '>').to_string())
                        .filter(|t| !t.is_empty())
                        .collect()
                });
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
                        result.push('\n');
                        result.push_str(&session_next_steps(&sess.id));
                        result.push('\n');
                    }

                    ToolResult::ok(result)
                } else {
                    ToolResult::ok(format!(
                        "Background session started: {}\n{}",
                        session_id,
                        session_next_steps(&session_id)
                    ))
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
                "Unknown shell action '{}' for a PID-based call. Valid: list, kill, info",
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
                const SHOWN: usize = 50;
                // Count every match first so the cut can say "50 of N".
                let matching: Vec<&str> = lines
                    .iter()
                    .skip(1)
                    .copied()
                    .filter(|line| !line.is_empty())
                    .filter(|line| filter.is_empty() || line.to_lowercase().contains(&filter_lower))
                    .collect();

                if matching.is_empty() && !filter.is_empty() {
                    return ToolResult::ok(format!("No processes found matching: {}", filter));
                }

                for line in matching.iter().take(SHOWN) {
                    result.push_str(line);
                    result.push('\n');
                }
                if matching.len() > SHOWN {
                    result.push_str(&format!(
                        "\n... showing {} of {} matching processes; pass filter: \"<name>\" to narrow",
                        SHOWN,
                        matching.len()
                    ));
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
                Ok(output) if output.status.success() => ToolResult::ok(format!(
                    "Sent SIG{} to PID {}. Confirm it exited with action: \"info\", pid: {}",
                    sig, pid, pid
                )),
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

        // Android has `ps` too (toybox, or procps on Termux) — use the Linux
        // field list; an unsupported field surfaces as "process not found".
        #[cfg(any(target_os = "linux", target_os = "android"))]
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
                // ps exits non-zero both for "no such PID" and for a field it
                // does not know; its own message says which.
                Ok(o) => ToolResult::error(format!(
                    "ps could not report PID {}: {}",
                    pid,
                    ps_failure_detail(&o)
                )),
                Err(e) => ToolResult::error(format!("ps could not report PID {}: {}", pid, e)),
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
                Ok(o) => ToolResult::error(format!(
                    "tasklist could not report PID {}: {}",
                    pid,
                    ps_failure_detail(&o)
                )),
                Err(e) => ToolResult::error(format!("tasklist could not report PID {}: {}", pid, e)),
            }
        }
    }

    async fn handle_session(&self, input: &ShellInput) -> ToolResult {
        let action = input.action.as_str();
        if matches!(action, "poll" | "log" | "write" | "kill" | "info") && input.session_id.is_empty() {
            return ToolResult::error(format!(
                "session_id is required: os(resource: \"shell\", action: \"{action}\", \
                 session_id: \"<id from the background start>\"){}",
                match action {
                    "write" => ", plus data: \"<text to send to stdin>\"",
                    "kill" | "info" => "; for a system process pass pid: <number> instead",
                    _ => "",
                }
            ));
        }
        match action {
            "list" => self.list_sessions().await,
            "poll" => self.poll_session(&input.session_id).await,
            "log" => self.get_session_log(&input.session_id).await,
            "write" => self.write_to_session(&input.session_id, &input.data).await,
            "kill" => self.kill_session(&input.session_id).await,
            "info" => self.session_info(&input.session_id).await,
            other => ToolResult::error(format!(
                "Unknown shell action '{}' for a session_id-based call. Valid: list, poll, log, write, kill, info",
                other
            )),
        }
    }

    /// Status of a background session without draining its pending output
    /// (`poll` drains; `info` only reports).
    async fn session_info(&self, session_id: &str) -> ToolResult {
        match self.registry.get_any_session(session_id).await {
            Some(sess) => ToolResult::ok(format!(
                "Session: {} (PID {})\n{}\nCommand: `{}`",
                sess.id,
                sess.pid,
                session_status(sess.exited, sess.exit_code),
                sess.command
            )),
            None => ToolResult::error(format!("Session not found: {}", session_id)),
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

        let mut result = format!(
            "Session: {} (PID {})\n{}\n",
            sess.id,
            sess.pid,
            session_status(sess.exited, sess.exit_code)
        );

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
                    // An empty log means different things for a live process
                    // and a finished one; say which.
                    if sess.exited {
                        let code = sess
                            .exit_code
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "unknown".to_string());
                        ToolResult::ok(format!("(no output; exited with code {})", code))
                    } else {
                        ToolResult::ok(format!(
                            "(no output yet; still running, PID {})",
                            sess.pid
                        ))
                    }
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

/// The one status line for a background session, shared by poll and info.
fn session_status(exited: bool, exit_code: Option<i32>) -> String {
    if exited {
        let code = exit_code.map(|c| c.to_string()).unwrap_or_else(|| "?".to_string());
        format!("Status: Exited (code {code})")
    } else {
        "Status: Running".to_string()
    }
}

/// The three calls that manage a background session, spelled out so the
/// model does not have to guess the resource/action pair.
fn session_next_steps(session_id: &str) -> String {
    format!(
        "Running. Poll: os(resource: \"shell\", action: \"poll\", session_id: \"{session_id}\"); \
         full log: action \"log\"; stop: action \"kill\"."
    )
}

/// What a failed `ps`/`tasklist` said, for the PID-lookup error: stderr,
/// else stdout, else the exit code.
fn ps_failure_detail(o: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }
    let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();
    if !stdout.is_empty() {
        return stdout;
    }
    match o.status.code() {
        Some(c) => format!("exit code {c} with no output (no such PID)"),
        None => "ended without an exit code".to_string(),
    }
}

/// Extract the base command name from a (possibly piped) command string.
/// Uses the LAST segment in a pipeline, since that determines the exit code.
/// The program the shell reported missing, read from its own message:
/// `sh: foo: command not found`, `zsh: command not found: foo`,
/// `'foo' is not recognized as an internal or external command`.
fn missing_command_name(output: &str) -> Option<String> {
    for line in output.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_suffix("command not found") {
            // "sh: foo: command not found" -> the token before the last ": "
            let head = rest.trim_end().trim_end_matches(':').trim_end();
            let name = head.rsplit(':').next()?.trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
        if let Some(rest) = l.split("command not found:").nth(1) {
            let name = rest.trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
        if let Some(idx) = l.find(" is not recognized as") {
            let name = l[..idx].trim().trim_matches(|c| c == '\'' || c == '"');
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
        if let Some(rest) = l.strip_suffix(": not found") {
            let name = rest.rsplit(':').next()?.trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

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
/// The follow/watch flag in a command that would never terminate on its own,
/// if any. Returns `None` when the command bounds itself — `timeout N …`,
/// a `head` in the pipeline, or `tail -f -m N` all terminate, and refusing
/// those would block legitimate work.
fn detect_unbounded_follow(command: &str) -> Option<&'static str> {
    let c = command.to_lowercase();
    // Explicit bounds make a follow finite — allow them through.
    if c.contains("timeout ") || c.contains("| head") || c.contains("|head") {
        return None;
    }
    // `watch` re-runs forever and takes no follow flag at all.
    if c.split_whitespace().next() == Some("watch") {
        return Some("watch");
    }
    // A bare `-f`/`--follow` token. Only meaningful for the log readers — for
    // `grep -f patterns.txt` the same flag means "patterns from file", which is
    // finite and must not be refused.
    if !c.split_whitespace().any(|t| t == "-f" || t == "--follow") {
        return None;
    }
    if c.contains("journalctl") {
        return Some("journalctl -f");
    }
    if c.contains("logs") {
        return Some("logs -f");
    }
    if c.contains("tail") {
        return Some("tail -f");
    }
    None
}

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
                // Name the program the shell named, not the last one in the
                // pipeline: for `foo | grep x` with foo missing, the old text told
                // the user grep was not installed.
                Some(match missing_command_name(output) {
                    Some(name) => format!(
                        "The command '{}' is not available on this system. Tell the user it isn't \
                         installed — do not substitute another command or install it without asking.",
                        name
                    ),
                    None => "A command in this pipeline is not installed (the shell's message above names it). Tell the user; do not substitute another command.".to_string(),
                })
            } else if lo.contains("no such file")
                || lo.contains("unable to open")
                || lo.contains("cannot open")
                || lo.contains("does not exist")
            {
                // "does not exist" fires for a missing branch, table, or
                // route as readily as for a file; the stderr line above is
                // the fact, this only points at it. A benchmark run once
                // read the old "verify with the user" wording as "do not
                // look", and burned five commands guessing at a path that
                // one glob found.
                Some(
                    "stderr reports something missing; read the message above before acting. \
                     If it names a file, the working directory may be wrong or the file may \
                     live elsewhere: one glob for its name settles that. Do not substitute a \
                     different file or name; if none matches, say so."
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
    #[test]
    fn the_missing_command_is_the_one_the_shell_named_not_the_last_in_the_pipe() {
        use super::missing_command_name;
        assert_eq!(missing_command_name("sh: foo: command not found\n").as_deref(), Some("foo"));
        assert_eq!(missing_command_name("zsh: command not found: foo").as_deref(), Some("foo"));
        assert_eq!(missing_command_name("/bin/sh: 1: foo: not found").as_deref(), Some("foo"));
        assert_eq!(missing_command_name("'foo' is not recognized as an internal or external command").as_deref(), Some("foo"));
        assert_eq!(missing_command_name("grep: x: No such file"), None);
    }

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
    #[test]
    fn unbounded_follows_are_detected() {
        // The exact shape that parked a live run for 300s.
        assert!(detect_unbounded_follow("tail -f /tmp/app.log 2>&1 | grep \"READY\"").is_some());
        assert!(detect_unbounded_follow("tail -f /var/log/x").is_some());
        assert!(detect_unbounded_follow("docker compose logs -f web").is_some());
        assert!(detect_unbounded_follow("journalctl -f -u nebo").is_some());
        assert!(detect_unbounded_follow("watch docker ps").is_some());
    }

    #[test]
    fn bounded_and_ordinary_commands_pass() {
        // Self-bounding forms must NOT be refused.
        assert!(detect_unbounded_follow("timeout 5 tail -f /tmp/app.log").is_none());
        assert!(detect_unbounded_follow("tail -f /tmp/app.log | head -n 20").is_none());
        // Ordinary snapshots — the form the refusal teaches.
        assert!(detect_unbounded_follow("tail -n 50 /tmp/app.log").is_none());
        assert!(detect_unbounded_follow("docker compose logs --tail 50 web").is_none());
        assert!(detect_unbounded_follow("ls -la /tmp").is_none());
        assert!(detect_unbounded_follow("grep -f patterns.txt input.txt").is_none());
    }

    // An unattended agent can never answer a credential prompt, so every
    // shell command runs with git's prompts disabled.
    #[tokio::test]
    async fn git_commands_in_the_shell_get_the_no_prompt_env() {
        let t = tool();
        let r = t.execute(&ctx(), json!({"resource": "shell", "action": "exec", "command": "env"})).await;
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("GIT_TERMINAL_PROMPT=0"), "{}", r.content);
        assert!(r.content.lines().any(|l| l == "GIT_ASKPASS="), "{}", r.content);
        assert!(r.content.contains("GIT_SSH_COMMAND=ssh -o BatchMode=yes"), "{}", r.content);
    }

    // The run's default cwd (an isolated sub-agent's worktree) is used when
    // the call names none; the call's own cwd still wins.
    #[tokio::test]
    async fn default_cwd_from_run_request_is_used_when_the_call_has_none() {
        let t = tool();
        let dir = tempfile::tempdir().unwrap();
        let want = std::fs::canonicalize(dir.path()).unwrap();
        let mut c = ctx();
        c.cwd = Some(dir.path().to_string_lossy().into_owned());
        let r = t.execute(&c, json!({"resource": "shell", "action": "exec", "command": "pwd -P"})).await;
        assert_eq!(r.content.trim(), want.to_string_lossy(), "{}", r.content);
        let other = tempfile::tempdir().unwrap();
        let r = t
            .execute(&c, json!({"resource": "shell", "action": "exec", "command": "pwd -P", "cwd": other.path()}))
            .await;
        assert_eq!(r.content.trim(), std::fs::canonicalize(other.path()).unwrap().to_string_lossy(), "{}", r.content);
    }

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

    // An empty result names the exit code instead of a bare "(no output)",
    // and an unroutable call names the valid actions and the call shape.
    #[tokio::test]
    async fn empty_output_and_unknown_actions_are_stated_in_full() {
        let t = tool();
        let r = t.execute(&ctx(), json!({"action": "exec", "command": "true"})).await;
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(r.content, "(exit 0, no output)");

        let r = t.execute(&ctx(), json!({"action": "frobnicate"})).await;
        assert!(r.is_error);
        assert!(r.content.contains("Unknown shell action 'frobnicate'"), "{}", r.content);
        assert!(r.content.contains("Valid: exec, list, poll, log, write, kill, info"), "{}", r.content);

        let r = t.execute(&ctx(), json!({"action": "poll"})).await;
        assert!(r.is_error);
        assert!(r.content.contains("session_id: \"<id from the background start>\""), "{}", r.content);

        let r = t.execute(&ctx(), json!({"action": "frobnicate", "pid": 1})).await;
        assert!(r.content.contains("for a PID-based call. Valid: list, kill, info"), "{}", r.content);
    }

    // The background start names the poll call; an empty session log says
    // whether the process is still running or how it exited.
    #[tokio::test]
    async fn background_start_names_the_poll_call_and_the_log_states_liveness() {
        let t = tool();
        let r = t
            .execute(&ctx(), json!({"action": "exec", "command": "sleep 2", "background": true}))
            .await;
        assert!(!r.is_error, "{}", r.content);
        assert!(
            r.content.contains("Poll: os(resource: \"shell\", action: \"poll\", session_id: \""),
            "{}",
            r.content
        );
        let id = r
            .content
            .split("**")
            .nth(1)
            .expect("session id between ** markers")
            .to_string();
        let log = t.execute(&ctx(), json!({"action": "log", "session_id": id})).await;
        assert!(log.content.starts_with("(no output yet; still running, PID "), "{}", log.content);
        let _ = t.execute(&ctx(), json!({"action": "kill", "session_id": id})).await;
    }
}

/// "Command exited with code N", or the signal that killed it. "code -1"
/// hid that a dev-server restart had taken the child with it (2026-09-03).
fn exit_header(status: &std::process::ExitStatus) -> String {
    #[cfg(unix)]
    if let Some(sig) = std::os::unix::process::ExitStatusExt::signal(status) {
        return format!("Command was killed by signal {sig} ({})", signal_name(sig));
    }
    match status.code() {
        Some(code) => format!("Command exited with code {code}"),
        None => "Command ended without an exit code".to_string(),
    }
}

#[cfg(unix)]
fn signal_name(sig: i32) -> &'static str {
    match sig {
        1 => "SIGHUP",
        2 => "SIGINT",
        6 => "SIGABRT",
        7 => "SIGBUS",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        15 => "SIGTERM",
        _ => "unknown signal",
    }
}

#[cfg(all(test, unix))]
mod exit_header_tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    #[test]
    fn a_signal_is_named_and_a_code_is_kept() {
        let killed = std::process::ExitStatus::from_raw(9);
        assert_eq!(exit_header(&killed), "Command was killed by signal 9 (SIGKILL)");
        let failed = std::process::ExitStatus::from_raw(3 << 8);
        assert_eq!(exit_header(&failed), "Command exited with code 3");
    }
}
