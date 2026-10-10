use crate::errors;
use crate::origin::ToolContext;
use crate::process::{self, ProcessRegistry};
use crate::registry::ToolResult;
use serde::Deserialize;
use std::sync::Arc;

/// Shell operations: execute commands, manage processes and background sessions.
pub struct ShellTool {
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
    /// What the command does, as the owner read it: named when its end is
    /// reported.
    #[serde(default)]
    description: String,
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
    /// `write`: named keys typed after `data` (a terminal session's only).
    #[serde(default)]
    keys: Vec<String>,
    /// `write`: a new terminal size; a dimension left out keeps its value.
    #[serde(default)]
    cols: Option<u16>,
    #[serde(default)]
    rows: Option<u16>,
    /// `exec`: run the command in a terminal of its own.
    #[serde(default)]
    pty: bool,
    /// Machine-consumer mode: on success return stdout ONLY (no STDERR
    /// section, no "(no output)" placeholder, no truncation footer); on a
    /// non-zero exit return an error carrying stderr. Used by deterministic
    /// workflow nodes whose output is parsed, not read by a model. On
    /// `poll`, a terminal session's output as the program wrote it, escape
    /// codes and all.
    #[serde(default)]
    raw: bool,
}

impl ShellTool {
    pub fn new(registry: Arc<ProcessRegistry>) -> Self {
        Self {
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
                return ToolResult::error(format!("invalid input: {e}"))
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
            "bash" if ctx.reach.offline => {
                let mut result = self.handle_bash(&si, ctx).await;
                result.content.push_str(&offline_note());
                result
            }
            "bash" => self.handle_bash(&si, ctx).await,
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

    async fn handle_bash(&self, input: &ShellInput, ctx: &ToolContext) -> ToolResult {
        if input.command.is_empty() {
            return ToolResult::error(errors::missing_param(
                "exec",
                "command",
                "run_command(command: \"ls -la\", description: \"List files\")",
            ));
        }

        // Document conversion has ONE canonical pathway — the embedded Typst
        // engine behind `convert_file`. Host converter binaries only exist
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
                     as Markdown, then convert_file(path: \"/path/doc.md\", to: \"pdf\"). \
                     It typesets identically on every \
                     platform and the PDF appears in the Work panel automatically.",
                );
            }
        }

        // Installed plugins are on the shell's PATH for workflow command nodes,
        // which also get their auth env. A model invoking one from the shell
        // runs it with no account, no approval gate and no profile, so it
        // 401s and the model rotates through env vars, stdin pipes and direct
        // API calls trying to make it work (CFO, 2026-09-06: ten such calls).
        // Redirect to the plugin tool, which has all three.
        if !ctx.trusted_plugin_env {
            if let Some(ref ps) = self.plugin_store {
                let names: std::collections::HashMap<String, String> = ps
                    .build_env_map()
                    .into_iter()
                    .filter(|(k, _)| k.ends_with("_BIN"))
                    .filter_map(|(k, v)| {
                        let bin = std::path::Path::new(&v).file_name()?.to_str()?.to_string();
                        let slug = k.trim_end_matches("_BIN").to_ascii_lowercase().replace('_', "-");
                        Some((bin, slug))
                    })
                    .collect();
                if let Some((slug, rest)) = plugin_invocation(&input.command, &names) {
                    return ToolResult::error(format!(
                        "`{slug}` is an installed plugin, and the shell runs it with no \
                         account, approval or profile context (that is why it answers 401 \
                         here). Run it through its own tool instead: {tool} with command \
                         \"{rest}\". JSON flag values go in `args` so nothing needs shell \
                         quoting: {tool} with command \"payment create\" and args \
                         {{\"line\": \"{{...}}\"}}.",
                        tool = crate::plugin_tools::plugin_tool_name(&slug)
                    ));
                }
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

        // A cloud bot's package installer, as a command of its own, is the
        // one command that runs through sudo (`system_packages`): it runs
        // outside the confinement (whose no-new-privileges would stop sudo),
        // and what it installs is recorded to be put back after a restart.
        let install = crate::system_packages::allowed(&input.command);
        if install.is_some() && ctx.reach.offline {
            return ToolResult::error(
                "The package installer downloads from the Debian mirrors, and this run's web access is off, \
                 so it can't run here.",
            );
        }

        // Privilege escalation is never a legitimate automation step: Nebo runs
        // unattended, so sudo either hangs on a password prompt or silently
        // escalates. Refuse before anything executes (covers background too).
        if install.is_none() && crate::policy::is_privilege_escalation(&input.command) {
            if crate::cloud_bot() {
                return ToolResult::error(crate::system_packages::CLOUD_SUDO_REFUSAL);
            }
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
                 To be able to undo a change, take a checkpoint first with \
                 checkpoint_files(paths: [...]) and put it back with restore_checkpoint. \
                 For parallel edits give each helper its own copy \
                 (delegate with isolation: \"worktree\"). If the owner truly wants history rewritten, \
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
                 path (no read check, no verification, no ledger). Use edit_file(path, \
                 old_string, new_string) for the same change, or replace_all: true for every \
                 occurrence. Plain `sed` that prints to stdout is fine."
                    .to_string(),
            );
        }

        let (cmd, closed_ports) = match self.command(input, ctx, install.is_some()) {
            Ok(launch) => launch,
            Err(refusal) => return refusal,
        };
        let caller = (!ctx.session_key.is_empty()).then(|| process::Caller {
            session_key: ctx.session_key.clone(),
            description: input.description.clone(),
        });
        if input.background {
            return self.execute_background(cmd, &closed_ports, input, caller, install).await;
        }

        let timeout_secs = if input.timeout > 0 {
            input.timeout as u64
        } else {
            120
        };
        let started_at = std::time::SystemTime::now();
        let started = match self.registry.spawn(cmd, &input.command, process::Spawn::Foreground(ctx.stdin.clone()), input.pty, &closed_ports).await {
            Ok(s) => s,
            Err(e) => return spawn_failure(&input.command, &e),
        };
        let started = record_install(started, install);
        // Until the command ends or moves to the background, its call owns
        // it: a cancelled turn drops this and takes the whole group with it.
        let mut owned = KillOnDrop(Some(started.session.pid));
        let mut exited = started.exited;
        let status = match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), &mut exited).await {
            Ok(status) => status,
            // A workflow's command step is parsed by the next step, and a
            // run nobody started has nobody to tell: their timeout is the
            // end of the command.
            Err(_) if input.raw || caller.is_none() => {
                let mut stopped = ToolResult::error(format!(
                    "Command stopped at its timeout of {timeout_secs}s: `{}`",
                    crate::truncate_str(&input.command, 80)
                ));
                if input.raw {
                    stopped.payload = Some(serde_json::json!({ "kind": COMMAND_EXIT, "timed_out": true }));
                }
                return stopped;
            }
            Err(_) => {
                if self.registry.move_to_background(&started.session, caller) {
                    owned.0 = None;
                    return ToolResult::ok(format!(
                        "Command exceeded its timeout ({timeout_secs}s) and was moved to the background \
                         with ID: {id}. It is still running; you'll be notified when it completes. Read \
                         what it has printed so far with read_output(task_id: \"{id}\"); stop it with \
                         stop_task(task_id: \"{id}\").{typed}",
                        id = started.session.id,
                        typed = if input.pty {
                            " If it is waiting for input, type into it with send_input."
                        } else {
                            ""
                        }
                    ));
                }
                // It ended in the same instant: its status is on the way.
                exited.await
            }
        };
        owned.0 = None;
        let Ok(Some(status)) = status else {
            return ToolResult::error(format!(
                "Command `{}` ended but its exit status could not be read.",
                crate::truncate_str(&input.command, 80)
            ));
        };
        let (stdout, stderr) = started.session.drain_pending(false).await;
        let output = std::process::Output { status, stdout, stderr };
        if input.raw {
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            // A program reading the result (a hook runner) gets the exit
            // status and both streams as they were written.
            let exit = serde_json::json!({
                "kind": COMMAND_EXIT,
                "exit_code": output.status.code(),
                "stdout": stdout,
                "stderr": stderr,
            });
            let mut result = if output.status.success() {
                ToolResult::ok(stdout)
            } else {
                ToolResult::error(format!("{}\n{}", exit_header(&output.status), stderr))
            };
            result.payload = Some(exit);
            return result;
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

        // A pipeline exits with its LAST stage's status: `frobnicate
        // --version | head -1` is exit 0 with "frobnicate: command not
        // found" on stderr, and the hint below never fired. The shell
        // named a missing program; that is the failure, whatever the code.
        let missing_in_pipeline = output.status.success() && missing_command_name(&stderr).is_some();
        // A search that printed nothing over a folder with no files in it
        // searched nothing: say so, whatever the exit code (`grep … | head`
        // launders grep's 1 into 0).
        let run_cwd = if input.cwd.is_empty() { ctx.cwd.as_deref().unwrap_or("") } else { input.cwd.as_str() };
        let nothing_searched = if stdout.trim().is_empty() {
            nothing_searched_note(&input.command, std::path::Path::new(run_cwd))
        } else {
            None
        };
        let mut noted = false;
        if !output.status.success() || missing_in_pipeline {
            let code = output.status.code().unwrap_or(-1);
            let (is_error, semantic_msg) =
                interpret_exit_code(&input.command, code, &result, nothing_searched.as_deref());
            noted = semantic_msg.is_some() && semantic_msg == nothing_searched;
            if let Some(msg) = semantic_msg {
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push_str(&msg);
            }
            if is_error {
                return ToolResult { payload: None, need: None, parked_ask: None, taint: Vec::new(), loads: Vec::new(),
                    content: format!("{}\n{}", exit_header(&output.status), result),
                    is_error: true,
                    image_url: None, more_files: Vec::new(),
                    http_status: None,
                    terminal: false,
                };
            }
            // Non-error exit (e.g. grep exit 1 = no matches) — fall through to success path
        }

        if result.is_empty() {
            result = "(exit 0, no output)".to_string();
        }
        if let (Some(note), false) = (&nothing_searched, noted) {
            result.push('\n');
            result.push_str(note);
        }

        // Long output is persisted by the registry (the one spill
        // path, at this tool's `max_result_chars`), never here.

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
        match crate::plugin_tool::produced_work_document(&tokens, base, started_at) {
            Some(path) => result.with_image_url(path),
            None => result,
        }
    }

    /// The one command every run_command call runs: the shell with the
    /// command, confined as the permission check decided (`confine`: Nebo's
    /// own files and ports closed, but not for Full access; no network for a
    /// run whose web access is off), its folder, and the
    /// environment (sanitized, Nebo's own settings left out, git's prompts
    /// off, installed plugins on the PATH, and plugin auth for a workflow's
    /// command step alone). With it, the ports its spawn must close
    /// (`confine::spawn_with`). A cloud bot's package installer
    /// (`installer`) runs unconfined: the confinement's no-new-privileges
    /// would stop its sudo, and its grammar names nothing but packages.
    fn command(&self, input: &ShellInput, ctx: &ToolContext, installer: bool) -> Result<(tokio::process::Command, Vec<u16>), ToolResult> {
        let trusted_plugin_env = ctx.trusted_plugin_env;
        let default_cwd = ctx.cwd.as_deref();
        // Every command is fenced, a workflow's command step included: it
        // runs the installed plugins, so their programs and data are open to
        // it (`NeboFiles::of`), and nothing else of Nebo's is. The command of
        // an employee with Full access is not (`confine::Reach::unconfined`).
        let (fence, closed_ports) = if ctx.reach.unconfined || installer {
            (None, Vec::new())
        } else {
            (crate::nebo_files::NeboFiles::of(ctx), types::own_ports::list())
        };
        let prefix = crate::confine::Confinement { offline: ctx.reach.offline, fence: fence.as_ref(), closed_ports: &closed_ports }
            .prefix()
            .map_err(|_| ToolResult::error(offline_unavailable()))?;
        let (shell, shell_args) = process::shell_command();
        let mut cmd = match prefix.split_first() {
            Some((program, args)) => {
                let mut cmd = command::new::<tokio::process::Command>(program, command::Console::Hidden);
                cmd.args(args).arg(&shell);
                cmd
            }
            None => command::new::<tokio::process::Command>(&shell, command::Console::Hidden),
        };
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
                return Err(ToolResult::error(errors::path_not_found(cwd)));
            }
            if !cwd_path.is_dir() {
                return Err(ToolResult::error(format!(
                    "Not a directory: {}. The cwd parameter must be a directory path.",
                    cwd
                )));
            }
            cmd.current_dir(cwd);
        }

        cmd.env_clear();
        for (k, v) in process::command_env() {
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
        Ok((cmd, closed_ports))
    }

    async fn execute_background(
        &self,
        cmd: tokio::process::Command,
        closed_ports: &[u16],
        input: &ShellInput,
        caller: Option<process::Caller>,
        install: Option<Vec<String>>,
    ) -> ToolResult {
        let told = if caller.is_some() { " You'll be notified when it ends." } else { "" };
        match self.registry.spawn(cmd, &input.command, process::Spawn::Background(caller), input.pty, closed_ports).await {
            Ok(started) => {
                let started = record_install(started, install);
                ToolResult::ok(format!(
                    "Background session started: **{}** (PID {})\n\nCommand: `{}`\n\n{}{told}\n",
                    started.session.id,
                    started.session.pid,
                    input.command,
                    session_next_steps(&started.session.id, input.pty)
                ))
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
        let cmd_result = command::new::<tokio::process::Command>("ps", command::Console::Hidden)
            .args(["aux"])
            .output()
            .await;

        #[cfg(windows)]
        let cmd_result = command::new::<tokio::process::Command>("tasklist", command::Console::Hidden)
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
            let result = command::new::<Command>("kill", command::Console::Hidden)
                .args([&format!("-{}", sig), &pid.to_string()])
                .output();

            match result {
                Ok(output) if output.status.success() => ToolResult::ok(format!(
                    "Sent SIG{} to PID {}. Confirm it exited with list_processes(pid: {})",
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
            let result = command::new::<std::process::Command>("taskkill", command::Console::Hidden)
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
            let output = command::new::<tokio::process::Command>("ps", command::Console::Hidden)
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
            let output = command::new::<tokio::process::Command>("tasklist", command::Console::Hidden)
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
                "task_id is required: the id run_command gave the background command (bg-…){}",
                match action {
                    "kill" | "info" => "; for another process on this computer, use list_processes with pid",
                    _ => "",
                }
            ));
        }
        match action {
            "list" => self.list_sessions().await,
            "poll" => self.poll_session(&input.session_id, input.raw).await,
            "log" => self.get_session_log(&input.session_id).await,
            "write" => self.write_to_session(input).await,
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

    async fn poll_session(&self, session_id: &str, raw: bool) -> ToolResult {
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

        let (stdout, stderr) = sess.drain_pending(raw).await;
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

    /// Type into a session: text, then named keys, after resizing its
    /// terminal when a size is given. Keys and a size need a terminal.
    async fn write_to_session(&self, input: &ShellInput) -> ToolResult {
        let session_id = input.session_id.as_str();
        let resize = input.cols.is_some() || input.rows.is_some();
        if input.data.is_empty() && input.keys.is_empty() && !resize {
            return ToolResult::error("Nothing to send: give text, keys, or a size (cols, rows).");
        }
        let mut bytes = input.data.clone().into_bytes();
        let mut resized = String::new();
        if !input.keys.is_empty() || resize {
            let terminal = match self.registry.terminal(session_id).await {
                Ok(t) => t,
                Err(e) => return ToolResult::error(format!("Error writing to session: {e}")),
            };
            if resize {
                match terminal.resize(input.cols, input.rows) {
                    Ok((cols, rows)) => resized = format!("; terminal is now {cols}x{rows}"),
                    Err(e) => return ToolResult::error(format!("Error resizing session {session_id}: {e}")),
                }
            }
            match crate::terminal::encode_keys(&input.keys, terminal.app_cursor()) {
                Ok(keys) => bytes.extend(keys),
                Err(e) => return ToolResult::error(e),
            }
        }
        if bytes.is_empty() {
            return ToolResult::ok(format!("Resized session {session_id}{resized}"));
        }
        match self.registry.write_stdin(session_id, &bytes).await {
            Ok(()) => ToolResult::ok(format!("Wrote {} bytes to session {session_id}{resized}", bytes.len())),
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

/// A foreground command's process group, killed when its call is dropped
/// (a cancelled turn) before the command ended or moved to the background.
struct KillOnDrop(Option<u32>);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            process::kill_group(pid);
        }
    }
}

/// A cloud bot's install (`packages`, from `system_packages::allowed`) is
/// recorded once the command succeeds, however the call ends: in the
/// foreground, moved to the background past its timeout, or started there.
/// The exit status reaches the caller after the record is written.
fn record_install(started: process::Started, packages: Option<Vec<String>>) -> process::Started {
    let Some(packages) = packages.filter(|p| !p.is_empty()) else {
        return started;
    };
    let (tx, rx) = tokio::sync::oneshot::channel();
    let exited = started.exited;
    tokio::spawn(async move {
        if let Ok(status) = exited.await {
            if status.is_some_and(|s| s.success()) {
                crate::system_packages::record(&packages);
            }
            let _ = tx.send(status);
        }
    });
    process::Started { session: started.session, exited: rx }
}

/// The `payload` kind of a raw command's result: `exit_code` (null when a
/// signal ended it), `stdout` and `stderr`, or `timed_out`.
pub const COMMAND_EXIT: &str = "command_exit";

/// Told after every command of a run whose web access is off.
fn offline_note() -> String {
    format!("\n\n{}", crate::capabilities::web_off(
        "This ran with no network access: if it reached for the network, that is why it failed.",
        "this work",
    ))
}

/// A run whose web access is off, on a computer that can't keep a command
/// off the network.
fn offline_unavailable() -> String {
    crate::capabilities::web_off(
        "Commands can't run for this work here: this computer has no way to run a command without the network.",
        "this work",
    )
}

fn spawn_failure(command: &str, e: &std::io::Error) -> ToolResult {
    let err_str = e.to_string();
    if err_str.contains("No such file or directory") || err_str.contains("not found") {
        ToolResult::error(errors::command_not_found(&extract_base_command(command)))
    } else if err_str.contains("Permission denied") {
        ToolResult::error(errors::permission_denied(command, "execute"))
    } else {
        ToolResult::error(format!("Command failed to start: {}", e))
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

/// The calls that manage a background command, spelled out with its id; a
/// terminal session is also typed into.
fn session_next_steps(session_id: &str, terminal: bool) -> String {
    let typed = if terminal {
        format!(
            " Type into it with send_input(task_id: \"{session_id}\", text: \"…\", keys: [\"Enter\"]); \
             read_output shows what the terminal printed."
        )
    } else {
        String::new()
    };
    format!(
        "Running. Read its output with read_output(task_id: \"{session_id}\"); stop it with \
         stop_task(task_id: \"{session_id}\").{typed}"
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
        // PowerShell: "foo : The term 'foo' is not recognized as the name of a cmdlet, …"
        if let Some(rest) = l.split("The term '").nth(1)
            && let Some((name, _)) = rest.split_once("' is not recognized as")
            && !name.is_empty()
        {
            return Some(name.to_string());
        }
        // cmd.exe: "'foo' is not recognized as an internal or external command, …"
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

/// The program a shell call's exit code comes from, and the exit-code
/// convention to read it by. The shell reports the status of the last
/// command it ran (`policy::exit_command`): the last stage of a pipeline,
/// the right side of the last `&&`/`||`, so `cat f | grep x` exiting 1 is
/// grep's "no matches". After `&&` the code may be the step before it
/// failing (`cd d && grep x` exits 1 when `d` is missing), so there the
/// default applies: nonzero is an error. The convention is the program's
/// name, `git diff` and `git grep` by their subcommand, or empty for the
/// default. A script the reader can't parse falls back to the last stage
/// split on `|`.
fn exit_program(command: &str) -> (String, String) {
    let Some((sub, operator)) = crate::policy::exit_command(command) else {
        let base = extract_base_command(command);
        return (base.clone(), base);
    };
    let words: Vec<&str> = sub.words().iter().map(|w| w.as_deref().unwrap_or("")).collect();
    let program = words
        .first()
        .and_then(|w| std::path::Path::new(w).file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    let convention = match (program.as_str(), words.get(1).copied()) {
        _ if operator.as_deref() == Some("&&") => String::new(),
        ("git", Some(sub @ ("diff" | "grep"))) => format!("git {sub}"),
        _ => program.clone(),
    };
    (program, convention)
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

/// Programs whose empty output means "nothing matched" only when they had
/// files to look at.
const SEARCH_PROGRAMS: &[&str] = &["grep", "egrep", "fgrep", "rg", "ag", "ack", "find", "fd"];

/// Most entries looked at to decide a folder holds no files; past it the
/// folder is taken to have some.
const EMPTY_FOLDER_SCAN: usize = 2_000;

/// When `command` runs a search that searched nothing, the note that says
/// so: over folders that hold no files at all (the search programs'
/// arguments that name existing directories, relative ones against `cwd`),
/// or from a path that does not exist: a `find` starting point or an `ls`
/// operand.
///
/// Gate 2026-09-27 (`file-discovery-spiral`, 0/3): `find ~/Desktop … 2>/dev/null
/// | head` on a computer with no Desktop folder came back "(exit 0, no
/// output)", the same words as a search that found nothing, and every run
/// spent 3–5 more commands finding out the folder wasn't there, widening to
/// `/home` and then to `/`. A missing path is read only where every such
/// word is a path: `find`'s starting points (the words before its
/// expression) and `ls`'s operands. The re-run after that fix
/// (36371219698) still had `ls -la ~/Desktop/ 2>/dev/null` answer a bare
/// "Command exited with code 2". The search programs take a pattern first,
/// and which of their words is a path is theirs to know. The commands are
/// read by `policy::subcommands`, the one shell reader: in 36374049578 a
/// split on spaces kept `2>/dev/null;` as one word, read the next command's
/// words as `ls` operands, and told the model "ls and 3 and d … does not
/// exist".
fn nothing_searched_note(command: &str, cwd: &std::path::Path) -> Option<String> {
    let mut empty: Vec<String> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    // A `cd` earlier in the command moves where relative paths point.
    let mut moved = false;
    // Each command as the shell runs it (`policy::subcommands`): its words
    // with quoting removed, redirections left out, and a word the shell
    // still rewrites (a variable, a glob) unknown.
    for sub in crate::policy::subcommands(command) {
        let words = sub.words();
        let Some(Some(first)) = words.first() else { continue };
        let program = std::path::Path::new(first).file_name().and_then(|n| n.to_str()).unwrap_or("");
        let args = &words[1..];
        match program {
            "cd" => moved = true,
            "ls" => {
                let mut value_next = false;
                for word in args {
                    if std::mem::take(&mut value_next) {
                        continue;
                    }
                    match word.as_deref() {
                        Some(w) if LS_VALUE_OPTIONS.contains(&w) => value_next = true,
                        Some(w) if !w.starts_with('-') => {
                            note_missing(w, cwd, moved, &mut missing);
                        }
                        _ => {}
                    }
                }
            }
            p if SEARCH_PROGRAMS.contains(&p) => {
                if p == "find" {
                    for root in find_roots(args) {
                        note_missing(root, cwd, moved, &mut missing);
                    }
                }
                for word in args.iter().flatten().filter(|w| !w.starts_with('-')) {
                    let path = cwd.join(crate::file_tool::expand_path(word));
                    if path.is_dir() && !holds_a_file(&path) && !empty.contains(word) {
                        empty.push(word.clone());
                    }
                }
            }
            _ => {}
        }
    }
    let mut notes: Vec<String> = Vec::new();
    if !missing.is_empty() {
        notes.push(format!(
            "{} {}, so nothing was searched or listed there. If the owner named that folder, tell them \
             it isn't there and ask where the files are, rather than searching other folders for it.",
            missing.join(" and "),
            if missing.len() == 1 { "does not exist" } else { "do not exist" }
        ));
    }
    if !empty.is_empty() {
        notes.push(format!(
            "{} holds no files, so this searched nothing: the empty result says nothing about whether what you \
             looked for exists. Tell the owner the folder is empty and ask where the files are.",
            empty.join(" and ")
        ));
    }
    (!notes.is_empty()).then(|| notes.join(" "))
}

/// A `find`'s starting points: the known words before its expression,
/// past its leading -H/-L/-P/-D/-O options.
fn find_roots(args: &[Option<String>]) -> Vec<&str> {
    let mut roots = Vec::new();
    let mut words = args.iter().peekable();
    while let Some(word) = words.peek() {
        match word.as_deref() {
            Some("-H" | "-L" | "-P") => {}
            Some("-D") => {
                words.next();
            }
            Some(w) if w.starts_with("-O") => {}
            _ => break,
        }
        words.next();
    }
    for word in words {
        match word.as_deref() {
            Some(w) if w.starts_with('-') || matches!(w, "(" | ")" | "!" | ",") => break,
            Some(w) => roots.push(w),
            None => {}
        }
    }
    roots
}

/// `ls` options whose value is the next word, not an operand.
const LS_VALUE_OPTIONS: &[&str] = &[
    "-I", "-w", "-T", "--ignore", "--hide", "--width", "--tabsize", "--block-size", "--format", "--sort",
    "--time-style", "--quoting-style", "--indicator-style",
];

/// Add `token` to `missing` when it names a path that isn't there, shown
/// with the path it resolves to. Whether it did.
fn note_missing(token: &str, cwd: &std::path::Path, moved: bool, missing: &mut Vec<String>) -> bool {
    let Some(path) = missing_path(token, cwd, moved) else { return false };
    let shown = match path.to_string_lossy() {
        p if p == token => token.to_string(),
        p => format!("{token} ({p})"),
    };
    if !missing.contains(&shown) {
        missing.push(shown);
    }
    true
}

/// The path a search starting point names when nothing is there, read as
/// the shell would pass it: `~` expanded, relative to `cwd` unless a `cd`
/// came before it (`moved`). `~user` is not read.
fn missing_path(token: &str, cwd: &std::path::Path, moved: bool) -> Option<std::path::PathBuf> {
    if token.starts_with('~') && token != "~" && !token.starts_with("~/") {
        return None;
    }
    let expanded = std::path::PathBuf::from(crate::file_tool::expand_path(token));
    if moved && expanded.is_relative() {
        return None;
    }
    let path = cwd.join(expanded);
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(path),
        _ => None,
    }
}

/// Whether `dir` holds at least one file anywhere below it (a folder too
/// big to scan counts as holding one).
fn holds_a_file(dir: &std::path::Path) -> bool {
    let mut pending = vec![dir.to_path_buf()];
    let mut seen = 0;
    while let Some(d) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for entry in entries.flatten() {
            seen += 1;
            if seen > EMPTY_FOLDER_SCAN {
                return true;
            }
            match entry.file_type() {
                Ok(t) if t.is_dir() => pending.push(entry.path()),
                _ => return true,
            }
        }
    }
    false
}

/// `nothing_searched`: the note for a search that searched nothing (folders
/// with no files, or a `find` starting point that isn't there), which
/// replaces grep's "no matches" and find's "inaccessible" (there was nothing
/// to match against).
fn interpret_exit_code(command: &str, exit_code: i32, output: &str, nothing_searched: Option<&str>) -> (bool, Option<String>) {
    let (base, convention) = exit_program(command);
    match convention.as_str() {
        // grep/rg: 0=matches found, 1=no matches, 2+=error
        "grep" | "rg" | "egrep" | "fgrep" | "git grep" => {
            if let (1, Some(note)) = (exit_code, nothing_searched) {
                (false, Some(note.to_string()))
            } else if exit_code == 1 {
                (false, Some("No matches found. This is not an error — the pattern does not appear in the searched files. Do not retry the same search.".to_string()))
            } else {
                (true, None)
            }
        }
        // diff: 0=identical, 1=differences found, 2+=error
        "diff" | "colordiff" | "git diff" => {
            if exit_code == 1 {
                (false, Some("Files differ.".to_string()))
            } else {
                (true, None)
            }
        }
        // find: 0=success, 1=some dirs inaccessible (partial), 2+=error
        // A starting point that isn't there also exits 1: the note says so.
        "find" | "fd" => {
            if let (1, Some(note)) = (exit_code, nothing_searched) {
                (false, Some(note.to_string()))
            } else if exit_code == 1 {
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
                        "The command '{name}' is not installed on this system. {} Don't search the disk \
                         for it or substitute another command.",
                        crate::errors::install_guidance()
                    ),
                    None => format!(
                        "A command in this pipeline is not installed (the shell's message above names it). {} \
                         Don't substitute another command.",
                        crate::errors::install_guidance()
                    ),
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
                     live elsewhere: one find for its name settles that. Do not substitute a \
                     different file or name; if none matches, say so."
                        .to_string(),
                )
            } else {
                None
            };
            // A failure with its error discarded (`2>/dev/null`) that
            // listed a path that isn't there says so.
            (true, hint.or_else(|| nothing_searched.map(str::to_string)))
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn shell_spots_a_plugin_binary_in_any_pipeline_stage() {
        let names: std::collections::HashMap<String, String> =
            [("quickbooks".to_string(), "quickbooks".to_string())].into_iter().collect();
        let hit = plugin_invocation(
            "cat /tmp/p.json | QUICKBOOKS_REALM_ID=1 quickbooks payment create --json 2>&1 || true",
            &names,
        );
        assert_eq!(hit, Some(("quickbooks".to_string(), "payment create --json".to_string())));
        assert_eq!(plugin_invocation("quickbooks doctor", &names).map(|h| h.1), Some("doctor".to_string()));
        // A plain command, or the word inside an argument, is not an invocation.
        assert_eq!(plugin_invocation("grep quickbooks notes.txt", &names), None);
        assert_eq!(plugin_invocation("ls -la", &names), None);
    }

    #[test]
    fn the_missing_command_is_the_one_the_shell_named_not_the_last_in_the_pipe() {
        use super::missing_command_name;
        assert_eq!(missing_command_name("sh: foo: command not found\n").as_deref(), Some("foo"));
        assert_eq!(missing_command_name("zsh: command not found: foo").as_deref(), Some("foo"));
        assert_eq!(missing_command_name("/bin/sh: 1: foo: not found").as_deref(), Some("foo"));
        assert_eq!(missing_command_name("'foo' is not recognized as an internal or external command").as_deref(), Some("foo"));
        assert_eq!(
            missing_command_name("foo-x : The term 'foo-x' is not recognized as the name of a cmdlet,\nfunction, script file, or operable program.").as_deref(),
            Some("foo-x")
        );
        assert_eq!(missing_command_name("grep: x: No such file"), None);
    }

    /// The exit code is read by the convention of the command it came from:
    /// the last one run. A search's 1 is "no matches", not an error,
    /// wherever the search stands in a pipeline or after `;`/`||`; after
    /// `&&` it may be the step before failing, so it is an error.
    #[test]
    fn the_exit_code_is_read_by_the_last_command_run() {
        use super::interpret_exit_code;
        let error = |cmd: &str, code: i32| interpret_exit_code(cmd, code, "", None).0;
        assert!(!error("cat f | grep x", 1), "a pipeline's code is its last stage's");
        assert!(error("cd d && grep x", 1), "after && the step before may have failed");
        assert!(!error("cd d; grep x f", 1));
        assert!(!error("grep x f 2>/dev/null", 1));
        assert!(!error("timeout 5 rg x", 1), "a wrapper runs the search");
        assert!(!error("/usr/bin/grep x f", 1));
        assert!(error("grep x f", 2), "2 is grep's error");
        assert!(error("grep x f | wc -l", 1), "wc's 1 is an error");
        assert!(!error("git diff --exit-code", 1), "files differ");
        assert!(!error("ls && test -f x || [ -d y ]", 1), "condition is false");
        assert!(!error("cat a | diff - b", 1));
        assert!(error("cat f | sort", 1));
        let (_, said) = interpret_exit_code("cat f | grep x", 1, "", None);
        assert!(said.is_some_and(|s| s.starts_with("No matches found")));
        let (failed, said) = interpret_exit_code("cd d && find x", 1, "", Some("Nothing was searched."));
        assert!(failed, "after && the default applies");
        assert_eq!(said.as_deref(), Some("Nothing was searched."), "the nothing-searched note stays");
    }

    use super::*;
    use crate::origin::Origin;
    use serde_json::json;

    fn tool() -> ShellTool {
        ShellTool::new(Arc::new(ProcessRegistry::new()))
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

    fn session_ctx() -> ToolContext {
        let mut c = ctx();
        c.session_key = "agent:a1:web".into();
        c
    }

    /// Past its timeout a command moves to the background instead of being
    /// killed, and its end
    /// reaches the session that ran it. Before: "Command killed after 1s".
    #[tokio::test]
    async fn a_command_past_its_timeout_moves_to_the_background_and_reports() {
        let registry = Arc::new(ProcessRegistry::new());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        registry.set_exit_sink(Arc::new(move |exit| {
            let _ = tx.send(exit);
        }));
        let t = ShellTool::new(registry.clone());
        let started = std::time::Instant::now();
        let r = t
            .execute(
                &session_ctx(),
                json!({"action": "exec", "command": "sleep 2; echo migrated", "timeout": 1, "description": "run the migration"}),
            )
            .await;
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("was moved to the background with ID: bg-"), "{}", r.content);
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "the call answered at its timeout");
        let exit = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv()).await.expect("told in time").expect("told");
        assert_eq!(exit.caller.session_key, "agent:a1:web");
        assert_eq!(exit.caller.description, "run the migration");
        assert_eq!((exit.exit_code, exit.output.trim()), (Some(0), "migrated"));
        assert!(r.content.contains(&exit.task_id), "the id it was moved under is the one reported");
    }

    /// The call answers at its timeout even when the command left behind a
    /// process the group kill cannot reach (the gate's
    /// `run-command-retry-spiral`, run 3: a `find` grandchild in
    /// uninterruptible sleep): the command moves to the background, so
    /// nothing waits on it.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_timeout_answer_arrives_even_with_a_child_the_kill_cannot_reach() {
        let file = std::env::temp_dir().join(format!("nebo-shell-escape-{}", std::process::id()));
        let _ = std::fs::remove_file(&file);
        let t = tool();
        let started = std::time::Instant::now();
        let r = t
            .execute(
                &session_ctx(),
                json!({"action": "exec", "command": crate::process::escaped_child_command(&file), "timeout": 1}),
            )
            .await;
        assert!(r.content.contains("moved to the background"), "{}", r.content);
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        let id = r.content.split("ID: ").nth(1).and_then(|s| s.split('.').next()).unwrap().to_string();
        let _ = t.execute(&ctx(), json!({"action": "kill", "session_id": id})).await;
        let escaped = crate::process::grandchild_pid(&file).await;
        // SAFETY: a pid this test created; the group kill misses it by design.
        unsafe { libc::kill(escaped, libc::SIGKILL) };
        let _ = std::fs::remove_file(&file);
    }

    /// A cancelled turn drops its call, and a foreground command that has
    /// not moved to the background dies with it, children included.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_dropped_call_takes_its_foreground_command_with_it() {
        let file = std::env::temp_dir().join(format!("nebo-shell-drop-{}", uuid::Uuid::new_v4()));
        let t = tool();
        let command = format!("sleep 30 & echo $! > {}; wait", file.display());
        let c = session_ctx();
        let call = t.execute(&c, json!({"action": "exec", "command": command, "timeout": 60}));
        let _ = tokio::time::timeout(std::time::Duration::from_millis(500), call).await;
        let pid = crate::process::grandchild_pid(&file).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(!crate::process::alive(pid), "the command outlived its cancelled call");
        let _ = std::fs::remove_file(&file);
    }

    // `frobnicate --version | head -1` exits 0 (the pipeline takes head's
    // status) with "frobnicate: command not found" on stderr; the gate saw
    // the model search the disk for it because no hint was appended.
    #[tokio::test]
    async fn a_missing_first_stage_is_the_failure_even_when_the_pipeline_exits_zero() {
        let t = tool();
        let res = t
            .execute(&ctx(), json!({"action": "exec", "command": "frobnicate_zz --version | head -1"}))
            .await;
        assert!(res.is_error, "{}", res.content);
        assert!(res.content.contains("'frobnicate_zz' is not installed"), "{}", res.content);
        assert!(res.content.contains("Don't search the disk"), "{}", res.content);
    }

    // Owner, 2026-09-28: installing never asks ("We must be able to actually
    // install things so we can code"). A missing program's hint says to
    // install it with this platform's installers and carry on; it never says
    // to ask first, and sudo stays refused.
    #[tokio::test]
    async fn a_missing_program_is_installed_and_the_work_carries_on() {
        let t = tool();
        let res = t.execute(&ctx(), json!({"action": "exec", "command": "frobnicate_zz --version"})).await;
        assert!(res.is_error, "{}", res.content);
        let hint = crate::errors::install_guidance();
        assert!(res.content.contains(&format!("'frobnicate_zz' is not installed on this system. {hint}")), "{}", res.content);
        let piped = t.execute(&ctx(), json!({"action": "exec", "command": "frobnicate_zz | head -1"})).await;
        assert!(piped.content.contains(hint), "{}", piped.content);
        let not_started = crate::errors::command_not_found("frobnicate_zz");
        assert!(not_started.contains(hint), "{not_started}");
        for text in [res.content.as_str(), piped.content.as_str(), not_started.as_str()] {
            let lower = text.to_lowercase();
            for ask in ["without asking", "ask the owner first", "ask the user first", "ask before"] {
                assert!(!lower.contains(ask), "{ask:?} in: {text}");
            }
        }
        assert!(hint.starts_with("Install it and carry on"), "{hint}");
        assert!(hint.contains("then run the command again"), "{hint}");
        assert!(hint.contains("cargo install") && hint.contains("go install"), "{hint}");
        #[cfg(target_os = "macos")]
        assert!(hint.contains("brew install") && hint.contains("Never sudo"), "{hint}");
        #[cfg(target_os = "linux")]
        assert!(hint.contains("pipx install") && hint.contains("apt and dnf need root"), "{hint}");
        #[cfg(target_os = "windows")]
        assert!(hint.contains("winget install --scope user") && hint.contains("scoop install"), "{hint}");
        let sudo = t.execute(&ctx(), json!({"action": "exec", "command": "sudo apt-get install -y frobnicate"})).await;
        assert!(sudo.is_error, "sudo is still refused: {}", sudo.content);
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
        assert!(r.content.contains("task_id is required"), "{}", r.content);

        let r = t.execute(&ctx(), json!({"action": "frobnicate", "pid": 1})).await;
        assert!(r.content.contains("for a PID-based call. Valid: list, kill, info"), "{}", r.content);
    }

    // Gate 2026-09-26 (correction-grep-no-files): `grep -rn x empty-src
    // 2>/dev/null | head` came back "(exit 0, no output)", and runs spent
    // four more commands finding out the folder was empty, one then telling
    // the owner the function "is not defined anywhere". A search over a
    // folder with no files says it searched nothing, on either exit code.
    #[tokio::test]
    async fn a_search_over_an_empty_folder_says_nothing_was_searched() {
        let t = tool();
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty-src");
        std::fs::create_dir_all(empty.join("nested")).unwrap();
        let empty = empty.to_string_lossy().into_owned();

        let piped = format!("grep -rn handle_login {empty} 2>/dev/null | head -20");
        let r = t.execute(&ctx(), json!({"action": "exec", "command": piped})).await;
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.starts_with("(exit 0, no output)\n"), "{}", r.content);
        assert!(r.content.contains(&format!("{empty} holds no files, so this searched nothing")), "{}", r.content);

        let bare = format!("grep -rn handle_login {empty}");
        let r = t.execute(&ctx(), json!({"action": "exec", "command": bare})).await;
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("holds no files"), "{}", r.content);
        assert!(!r.content.contains("No matches found"), "nothing was there to match: {}", r.content);

        // Relative to the call's folder, and `find` counts as a search.
        let r = t
            .execute(&ctx(), json!({"action": "exec", "command": "find empty-src -name '*.py'", "cwd": dir.path()}))
            .await;
        assert!(r.content.contains("empty-src holds no files"), "{}", r.content);

        // A folder with files and no match is a real "no matches".
        std::fs::write(dir.path().join("empty-src/nested/app.py"), "def other(): pass\n").unwrap();
        let r = t.execute(&ctx(), json!({"action": "exec", "command": format!("grep -rn handle_login {empty}")})).await;
        assert!(r.content.contains("No matches found"), "{}", r.content);
        assert!(!r.content.contains("holds no files"), "{}", r.content);
    }

    // Gate 2026-09-27 (file-discovery-spiral, 0/3): `find ~/Desktop … 2>/dev/null
    // | head` on a computer with no Desktop came back "(exit 0, no output)",
    // the words of a search that found nothing, and every run spent 3–5 more
    // commands finding out the folder wasn't there. A `find` from a starting
    // point that doesn't exist says so, on either exit code.
    #[tokio::test]
    async fn a_find_from_a_folder_that_isnt_there_says_nothing_was_searched() {
        let t = tool();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "x").unwrap();
        let missing = dir.path().join("Desktop").to_string_lossy().into_owned();
        let said = format!("{missing} does not exist, so nothing was searched or listed there");

        let piped = format!("find {missing} -type f \\( -iname '*screenshot*' -o -iname '*.png' \\) 2>/dev/null | head -30");
        let r = t.execute(&ctx(), json!({"action": "exec", "command": piped})).await;
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.starts_with("(exit 0, no output)\n"), "{}", r.content);
        assert!(r.content.contains(&said), "{}", r.content);
        assert!(r.content.contains("If the owner named that folder, tell them it isn't there"), "{}", r.content);

        // find's own exit 1 is the missing folder, not "inaccessible" ones.
        let r = t.execute(&ctx(), json!({"action": "exec", "command": format!("find -L {missing} -name '*.png'")})).await;
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains(&said), "{}", r.content);
        assert!(!r.content.contains("Some directories were inaccessible"), "{}", r.content);

        // Relative to the call's folder, and `~` as the shell expands it,
        // each shown with the path it names.
        let r = t
            .execute(&ctx(), json!({"action": "exec", "command": "find Desktop -name '*.png'", "cwd": dir.path()}))
            .await;
        assert!(r.content.contains(&format!("Desktop ({missing}) does not exist")), "{}", r.content);
        let home = dirs::home_dir().unwrap().join("nebo-no-such-folder-5c1e");
        let r = t.execute(&ctx(), json!({"action": "exec", "command": "find ~/nebo-no-such-folder-5c1e -name x 2>/dev/null"})).await;
        assert!(r.content.contains(&format!("~/nebo-no-such-folder-5c1e ({}) does not exist", home.display())), "{}", r.content);

        // A folder that is there and holds files: a real "nothing matched".
        // A pattern after the expression begins is never a starting point.
        let here = dir.path().to_string_lossy().into_owned();
        let r = t.execute(&ctx(), json!({"action": "exec", "command": format!("find {here} -name Desktop")})).await;
        assert_eq!(r.content, "(exit 0, no output)", "{}", r.content);

        // An `ls` of a folder that isn't there, its error discarded, fails
        // saying so instead of a bare exit code (36371219698: `ls -la
        // ~/Desktop/ 2>/dev/null` answered "Command exited with code 2").
        let r = t.execute(&ctx(), json!({"action": "exec", "command": format!("ls -la {missing}/ 2>/dev/null")})).await;
        assert!(r.is_error, "{}", r.content);
        assert!(r.content.contains(&format!("{missing}/ does not exist, so nothing was searched or listed there")), "{}", r.content);
        // Several at once are named together.
        let two = format!("find {missing} {missing}-2 -name x 2>/dev/null");
        let r = t.execute(&ctx(), json!({"action": "exec", "command": two})).await;
        assert!(r.content.contains(&format!("{missing} and {missing}-2 do not exist, so nothing")), "{}", r.content);
        // An option's value is not an operand.
        let note = nothing_searched_note(&format!("ls -la -I Desktop --hide Pictures {here}"), dir.path());
        assert_eq!(note, None, "{note:?}");

        // Each command of a compound is read on its own (36374049578: `;`
        // joined to a redirect ran the next command's words into `ls`).
        let compound = format!(
            "ls -la {missing}/ 2>/dev/null; ls -la {here}/ >/dev/null; find {here} -maxdepth 3 -type d -iname \"desktop\" 2>/dev/null"
        );
        let r = t.execute(&ctx(), json!({"action": "exec", "command": compound})).await;
        assert!(r.content.contains(&format!("\n{missing}/ does not exist, so nothing was searched or listed there")), "{}", r.content);
        for word in [" and ls", " and 3", " and d ", "desktop does not exist"] {
            assert!(!r.content.contains(word), "{word:?}: {}", r.content);
        }

        // Words the shell still rewrites, and relative paths after a `cd`,
        // aren't read; `ls` is read only where it is the command.
        for command in [
            "find $HOME/nebo-no-such-folder-5c1e -name x 2>/dev/null",
            "cd / && find Desktop -name x 2>/dev/null",
            "ls /home/*/nebo-no-such-folder-5c1e 2>/dev/null",
            "echo ls nebo-no-such-folder-5c1e >/dev/null",
        ] {
            let r = t.execute(&ctx(), json!({"action": "exec", "command": command, "cwd": dir.path()})).await;
            assert!(!r.content.contains("does not exist"), "{command}: {}", r.content);
        }
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
            r.content.contains("read_output(task_id: \""),
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

/// The installed plugin a shell command invokes, if any: the first word of any
/// pipeline stage (after `VAR=value` prefixes) that names a plugin binary.
/// Returns the plugin slug and the rest of that stage's command line.
fn plugin_invocation(command: &str, names: &std::collections::HashMap<String, String>) -> Option<(String, String)> {
    for stage in command.split(|c| c == '|' || c == ';' || c == '&' || c == '\n') {
        let mut words = stage.split_whitespace().skip_while(|w| {
            w.contains('=') && !w.starts_with('-') && !w.starts_with('"') && !w.starts_with('\'')
        });
        let Some(head) = words.next() else { continue };
        let bin = std::path::Path::new(head).file_name().and_then(|f| f.to_str()).unwrap_or(head);
        if let Some(slug) = names.get(bin) {
            // Drop redirections and the `|| true` tail: they are shell, not
            // plugin arguments.
            let rest = words
                .filter(|w| !w.contains('>') && !w.contains('<') && !matches!(*w, "||" | "&&" | "true"))
                .collect::<Vec<_>>()
                .join(" ");
            return Some((slug.clone(), rest));
        }
    }
    None
}
