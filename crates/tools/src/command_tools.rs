//! The command tools: `run_command` (always loaded) and the deferred
//! `read_output`, `stop_task`, `list_processes` and `send_input`. They run
//! on the shell handlers in `shell_tool.rs`; `read_output` and `stop_task`
//! also reach helpers and runs, by the kind of id they are given.

use std::sync::Arc;

use serde_json::{Value, json};
use types::permissions::RuleField;

use crate::file_tools::Machine;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

type Fut<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>>;

/// How long a call waits on its command when it names no timeout, and the
/// most it may name. Past it the command moves to the background.
const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;

fn str_arg<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// The id a call names, trimmed.
fn task_id(input: &Value) -> &str {
    str_arg(input, "task_id").map(str::trim).unwrap_or("")
}

/// A background command's id, as opposed to a helper's or a run's.
fn is_command_id(id: &str) -> bool {
    id.starts_with(crate::process::SESSION_ID_PREFIX)
}

/// `timeout` in milliseconds, capped, as whole seconds for the shell handler.
fn timeout_secs(input: &Value) -> u64 {
    let ms = input
        .get("timeout")
        .and_then(Value::as_u64)
        .filter(|ms| *ms > 0)
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .min(MAX_TIMEOUT_MS);
    ms.div_ceil(1000)
}

/// Where finding and searching happen, in the shell commands run in.
#[cfg(not(windows))]
const SEARCH_NOTE: &str = "- Find files with `find` and search contents with `grep` or `rg` here; keep searches bounded (a path, `-maxdepth`, `| head`).";
#[cfg(windows)]
const SEARCH_NOTE: &str = "- Commands run in PowerShell: find files with Get-ChildItem -Recurse and search contents with Select-String here; keep searches bounded (a path, `-Depth`, `| Select-Object -First 50`). `/tmp` and `~` paths work.";

/// A command cut for a label.
fn short(command: &str, n: usize) -> String {
    let t: String = command.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > n {
        format!("{}…", t.chars().take(n).collect::<String>())
    } else {
        t
    }
}

// ── run_command ────────────────────────────────────────────────────

pub struct RunCommandTool(pub Arc<Machine>);

impl DynTool for RunCommandTool {
    fn name(&self) -> &str {
        "run_command"
    }

    fn description(&self) -> String {
        format!(
            "Runs a shell command and returns its output.\n\
         - Prefer absolute paths; shell state doesn't carry between calls.\n\
         - Use read_file, edit_file and write_file instead of cat, head, tail, sed, awk or echo.\n\
         {SEARCH_NOTE}\n\
         - The owner sees `description`, not the command.\n\
         - Long jobs: set `background: true` and continue; you're notified when they end."
        )
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "The command to run." },
                "description": { "type": "string", "description": "What this command does, in plain words the owner will read (5–10 words). Don't repeat the command." },
                "timeout": { "type": "integer", "description": "Milliseconds to wait before it moves to the background (default 120000, max 600000)." },
                "background": { "type": "boolean", "description": "Run detached; you're told when it ends. Read output with read_output." },
                "cwd": { "type": "string", "description": "Folder to run in. Default: the conversation's working folder." }
            },
            "required": ["command", "description"]
        })
    }

    fn search_hint(&self) -> &str {
        "run shell command find grep"
    }

    fn should_defer(&self) -> bool {
        false
    }

    fn rule_field(&self, input: &Value) -> Option<RuleField> {
        str_arg(input, "command").map(|c| RuleField::CommandPrefix(c.to_string()))
    }

    fn capability(&self, _input: &Value) -> Option<&'static str> {
        Some("shell")
    }

    fn max_result_chars(&self, _input: &Value) -> Option<usize> {
        Some(crate::MAX_SUBPROCESS_OUTPUT)
    }

    fn activity(&self, input: &Value) -> String {
        match str_arg(input, "description") {
            Some(d) => d.to_string(),
            None => format!("running `{}`", short(str_arg(input, "command").unwrap_or(""), 72)),
        }
    }

    fn outcome(&self, input: &Value) -> String {
        match str_arg(input, "description") {
            Some(d) => d.to_string(),
            None => format!("Ran `{}`", short(str_arg(input, "command").unwrap_or(""), 72)),
        }
    }

    fn clearable(&self, _input: &Value) -> bool {
        true
    }

    /// A document the command produced surfaces as a card.
    fn emits_image(&self, _input: &Value) -> bool {
        true
    }

    fn execute_dyn<'a>(&'a self, ctx: &'a ToolContext, input: Value) -> Fut<'a> {
        Box::pin(async move {
            let command = str_arg(&input, "command").unwrap_or("").to_string();
            let mut call = json!({
                "action": "exec",
                "command": command,
                "timeout": timeout_secs(&input),
                "background": input.get("background").and_then(Value::as_bool).unwrap_or(false),
                "description": str_arg(&input, "description").unwrap_or(""),
            });
            if let Some(cwd) = str_arg(&input, "cwd") {
                call["cwd"] = json!(cwd);
            }
            // A workflow's command step (the one context the engine trusts
            // with plugin auth) is parsed by the next step, not read by a
            // model: its output is stdout alone, and a failure carries stderr.
            if ctx.trusted_plugin_env {
                call["raw"] = json!(true);
            }
            let cwd = str_arg(&input, "cwd").map(str::to_string).or_else(|| ctx.cwd.clone());
            let result = self.0.shell.execute(ctx, call).await;
            // A file the command wrote (`tee`, `>`) is the run's own change:
            // the read ledger follows it, so the next edit isn't warned about it.
            if !result.is_error {
                for target in crate::policy::shell_write_targets(&command) {
                    let path = std::path::Path::new(&target);
                    let abs = match (path.is_relative(), cwd.as_deref()) {
                        (true, Some(c)) => std::path::Path::new(c).join(path),
                        _ => path.to_path_buf(),
                    };
                    self.0.file.note_shell_write(&ctx.session_key, &abs.to_string_lossy());
                }
            }
            result
        })
    }
}

// ── read_output / stop_task ────────────────────────────────────────

/// What `read_output` and `stop_task` reach besides background commands:
/// helpers (the orchestrator, with the task table behind it), runs, and
/// workflow runs (`stop_task` only).
#[derive(Clone)]
pub struct Helpers {
    pub orchestrator: crate::OrchestratorHandle,
    pub store: Option<Arc<db::Store>>,
    pub runs: Option<crate::run_querier::RunQuerierHandle>,
    pub workflows: crate::workflows::WorkflowManagerCell,
}

impl Helpers {
    /// A helper's status from the orchestrator, else its row in the task table.
    async fn status(&self, ctx: &ToolContext, id: &str) -> ToolResult {
        if let Some(orch) = self.orchestrator.get()
            && let Ok(status) = orch.status(id, &ctx.session_key).await
        {
            return ToolResult::ok(status);
        }
        match self.store.as_deref().map(|s| s.get_pending_task(id)) {
            Some(Ok(Some(task))) => {
                let mut result = format!(
                    "Task: {}\nType: {}\nStatus: {}\nDescription: {}",
                    task.id,
                    task.task_type,
                    task.status,
                    task.description.as_deref().unwrap_or("-"),
                );
                if let Some(ref output) = task.output {
                    result.push_str(&format!("\nOutput:\n{output}"));
                }
                if let Some(ref err) = task.last_error {
                    result.push_str(&format!("\nError: {err}"));
                }
                ToolResult::ok(result)
            }
            Some(Err(e)) => ToolResult::error(format!("Could not look up {id}: {e}")),
            _ => ToolResult::error(format!(
                "Nothing has the id '{id}'. Ids come from run_command with background: true (bg-…) \
                 and from delegate in this conversation."
            )),
        }
    }

    /// Stop a helper, else a run the caller may stop.
    async fn stop(&self, ctx: &ToolContext, id: &str) -> ToolResult {
        let helper = match self.orchestrator.get() {
            Some(orch) => orch.cancel(id, &ctx.session_key).await.map(|()| format!("Stopped helper {id}")),
            None => match self.store.as_deref().map(|s| s.cancel_task(id)) {
                Some(Ok(())) => Ok(format!(
                    "Marked helper {id} stopped; no helper was running to stop."
                )),
                Some(Err(e)) => Err(e.to_string()),
                None => Err("no helpers are running".to_string()),
            },
        };
        let helper_err = match helper {
            Ok(done) => return ToolResult::ok(done),
            Err(e) => e,
        };
        if let Some(querier) = self.runs.as_ref().and_then(|h| h.get()) {
            // The primary employee sees every run; an employee only its own.
            let caller = {
                let id = types::keyparser::extract_agent_id(&ctx.session_key);
                if id.is_empty() { "main".to_string() } else { id }
            };
            match querier.cancel_run(id, &caller).await {
                Ok(true) => return ToolResult::ok(format!("Stopped run {id}")),
                Ok(false) => {}
                Err(e) => return ToolResult::error(e),
            }
        }
        let workflows = self.workflows.read().unwrap().clone();
        if let Some(manager) = workflows
            && manager.cancel(id).await.is_ok()
        {
            return ToolResult::ok(format!("Stopped workflow run {id}"));
        }
        ToolResult::error(format!(
            "Nothing running has the id '{id}' ({helper_err}). Ids come from run_command with \
             background: true (bg-…), from delegate, from list_runs, and from run_workflow."
        ))
    }
}

pub struct ReadOutputTool {
    pub machine: Arc<Machine>,
    pub helpers: Helpers,
}

impl DynTool for ReadOutputTool {
    fn name(&self) -> &str {
        "read_output"
    }

    fn description(&self) -> String {
        "Reads the output of a background command or a helper.\n\
         - `task_id` is the id run_command (background: true) or delegate gave you.\n\
         - A command's output is what it printed since you last read it, with whether it is still running."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string", "description": "The background command's or helper's id." }
            },
            "required": ["task_id"]
        })
    }

    fn search_hint(&self) -> &str {
        "check background command or helper output"
    }

    fn read_only(&self, _input: &Value) -> bool {
        true
    }

    /// A background command's output is shell control: its key is the
    /// shell's, so the limits on running commands hold for it; a helper's
    /// status is this tool's own key.
    fn rule_key(&self, input: &Value) -> String {
        if is_command_id(task_id(input)) { "read_command_output" } else { "read_output" }.to_string()
    }

    fn capability(&self, input: &Value) -> Option<&'static str> {
        is_command_id(task_id(input)).then_some("shell")
    }

    fn max_result_chars(&self, _input: &Value) -> Option<usize> {
        Some(crate::MAX_SUBPROCESS_OUTPUT)
    }

    fn activity(&self, input: &Value) -> String {
        format!("checking {}", task_id(input))
    }

    fn outcome(&self, input: &Value) -> String {
        format!("Checked {}", task_id(input))
    }

    fn execute_dyn<'a>(&'a self, ctx: &'a ToolContext, input: Value) -> Fut<'a> {
        Box::pin(async move {
            let id = task_id(&input);
            if is_command_id(id) {
                return self.machine.shell.execute(ctx, json!({"action": "poll", "session_id": id})).await;
            }
            self.helpers.status(ctx, id).await
        })
    }
}

pub struct StopTaskTool {
    pub machine: Arc<Machine>,
    pub helpers: Helpers,
}

impl DynTool for StopTaskTool {
    fn name(&self) -> &str {
        "stop_task"
    }

    fn description(&self) -> String {
        "Stops a background command, a helper, a run or a workflow run.\n\
         - `task_id` is the id run_command (background: true), delegate, list_runs or run_workflow gave you.\n\
         - To end another process on this computer, use run_command (kill)."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string", "description": "The background command's, helper's, run's or workflow run's id." }
            },
            "required": ["task_id"]
        })
    }

    fn search_hint(&self) -> &str {
        "stop background command helper or run"
    }

    /// Stopping a background command is shell control (see `read_output`).
    fn rule_key(&self, input: &Value) -> String {
        if is_command_id(task_id(input)) { "stop_command" } else { "stop_task" }.to_string()
    }

    fn capability(&self, input: &Value) -> Option<&'static str> {
        is_command_id(task_id(input)).then_some("shell")
    }

    fn activity(&self, input: &Value) -> String {
        format!("stopping {}", task_id(input))
    }

    fn outcome(&self, input: &Value) -> String {
        format!("Stopped {}", task_id(input))
    }

    fn execute_dyn<'a>(&'a self, ctx: &'a ToolContext, input: Value) -> Fut<'a> {
        Box::pin(async move {
            let id = task_id(&input);
            if is_command_id(id) {
                return self.machine.shell.execute(ctx, json!({"action": "kill", "session_id": id})).await;
            }
            self.helpers.stop(ctx, id).await
        })
    }
}

// ── list_processes / send_input ────────────────────────────────────

pub struct ListProcessesTool(pub Arc<Machine>);

impl DynTool for ListProcessesTool {
    fn name(&self) -> &str {
        "list_processes"
    }

    fn description(&self) -> String {
        "Lists your background commands, or the processes running on this computer.\n\
         - No arguments: your background commands, running and recently finished.\n\
         - `filter`: processes whose line contains it · `pid`: one process in detail."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "filter": { "type": "string", "description": "Show running processes whose line contains this text." },
                "pid": { "type": "integer", "description": "Show this one process in detail." }
            }
        })
    }

    fn search_hint(&self) -> &str {
        "list running processes background commands"
    }

    fn read_only(&self, _input: &Value) -> bool {
        true
    }

    fn capability(&self, _input: &Value) -> Option<&'static str> {
        Some("shell")
    }

    fn activity(&self, _input: &Value) -> String {
        "listing processes".to_string()
    }

    fn outcome(&self, _input: &Value) -> String {
        "Listed processes".to_string()
    }

    fn execute_dyn<'a>(&'a self, ctx: &'a ToolContext, input: Value) -> Fut<'a> {
        Box::pin(async move {
            let pid = input.get("pid").and_then(Value::as_i64).filter(|p| *p > 0);
            let call = match (pid, str_arg(&input, "filter")) {
                (Some(pid), _) => json!({"action": "info", "pid": pid}),
                (None, Some(filter)) => json!({"action": "list", "filter": filter}),
                (None, None) => json!({"action": "list"}),
            };
            self.0.shell.execute(ctx, call).await
        })
    }
}

pub struct SendInputTool(pub Arc<Machine>);

impl DynTool for SendInputTool {
    fn name(&self) -> &str {
        "send_input"
    }

    fn description(&self) -> String {
        "Sends text to a background command's input, as if typed.\n\
         - End the text with a newline to submit a line."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string", "description": "The background command's id (bg-…)." },
                "text": { "type": "string", "description": "The text to send." }
            },
            "required": ["task_id", "text"]
        })
    }

    fn search_hint(&self) -> &str {
        "type input into background command"
    }

    fn capability(&self, _input: &Value) -> Option<&'static str> {
        Some("shell")
    }

    fn activity(&self, input: &Value) -> String {
        format!("sending input to {}", task_id(input))
    }

    fn outcome(&self, input: &Value) -> String {
        format!("Sent input to {}", task_id(input))
    }

    fn execute_dyn<'a>(&'a self, ctx: &'a ToolContext, input: Value) -> Fut<'a> {
        Box::pin(async move {
            let call = json!({
                "action": "write",
                "session_id": task_id(&input),
                "data": input.get("text").and_then(Value::as_str).unwrap_or(""),
            });
            self.0.shell.execute(ctx, call).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::ProcessRegistry;

    fn machine() -> Arc<Machine> {
        Arc::new(Machine::new(Arc::new(ProcessRegistry::new()), None))
    }

    fn helpers() -> Helpers {
        Helpers { orchestrator: crate::orchestrator::new_handle(), store: None, runs: None, workflows: Default::default() }
    }

    /// A background command's whole life through the command tools: start,
    /// read, list, type into, stop.
    #[tokio::test]
    async fn a_background_command_is_read_listed_fed_and_stopped() {
        let m = machine();
        let ctx = ToolContext::new(crate::origin::Origin::User);
        let start = RunCommandTool(m.clone())
            .execute_dyn(&ctx, json!({"command": "sleep 5", "description": "Wait", "background": true}))
            .await;
        assert!(!start.is_error, "{}", start.content);
        let id = start.content.split("**").nth(1).expect("session id between ** markers").to_string();
        assert!(start.content.contains("read_output"), "the next step names the tool: {}", start.content);

        let read = ReadOutputTool { machine: m.clone(), helpers: helpers() }
            .execute_dyn(&ctx, json!({"task_id": id}))
            .await;
        assert!(!read.is_error && read.content.contains("Status: Running"), "{}", read.content);

        let list = ListProcessesTool(m.clone()).execute_dyn(&ctx, json!({})).await;
        assert!(!list.is_error && list.content.contains(&id), "{}", list.content);

        let typed = SendInputTool(m.clone()).execute_dyn(&ctx, json!({"task_id": id, "text": "x\n"})).await;
        assert!(!typed.is_error && typed.content.starts_with("Wrote 2 bytes"), "{}", typed.content);

        let stop = StopTaskTool { machine: m, helpers: helpers() }.execute_dyn(&ctx, json!({"task_id": id})).await;
        assert!(!stop.is_error && stop.content.contains("Killed session"), "{}", stop.content);
    }

    /// A background command's output and stop are shell control (the
    /// shell's keys and capability); a helper's are the tools' own keys.
    #[test]
    fn a_background_command_keys_as_shell_and_a_helper_as_itself() {
        let read = ReadOutputTool { machine: machine(), helpers: helpers() };
        let stop = StopTaskTool { machine: machine(), helpers: helpers() };
        let cmd = json!({"task_id": "bg-1a2b3c4d"});
        let helper = json!({"task_id": "sa-42"});
        assert_eq!((read.rule_key(&cmd).as_str(), read.capability(&cmd)), ("read_command_output", Some("shell")));
        assert_eq!((stop.rule_key(&cmd).as_str(), stop.capability(&cmd)), ("stop_command", Some("shell")));
        assert_eq!((read.rule_key(&helper).as_str(), read.capability(&helper)), ("read_output", None));
        assert_eq!((stop.rule_key(&helper).as_str(), stop.capability(&helper)), ("stop_task", None));
        assert!(read.read_only(&cmd) && !stop.read_only(&cmd));
    }

    /// A workflow run's id stops the run through the workflow manager.
    #[tokio::test]
    async fn a_workflow_run_id_stops_the_workflow_run() {
        let manager = Arc::new(crate::workflows::TestManager::default());
        let h = helpers();
        *h.workflows.write().unwrap() = Some(manager.clone() as Arc<dyn crate::workflows::WorkflowManager>);
        let ctx = ToolContext::new(crate::origin::Origin::User);
        let r = StopTaskTool { machine: machine(), helpers: h }.execute_dyn(&ctx, json!({"task_id": "run-7"})).await;
        assert!(!r.is_error && r.content == "Stopped workflow run run-7", "{}", r.content);
        assert_eq!(*manager.calls.lock().unwrap(), ["cancel run-7"]);
    }

    /// An id that is neither a command nor a known helper says where ids
    /// come from.
    #[tokio::test]
    async fn an_unknown_helper_id_says_where_ids_come_from() {
        let ctx = ToolContext::new(crate::origin::Origin::User);
        let r = ReadOutputTool { machine: machine(), helpers: helpers() }
            .execute_dyn(&ctx, json!({"task_id": "sa-nothing"}))
            .await;
        assert!(r.is_error && r.content.contains("delegate"), "{}", r.content);
        let r = StopTaskTool { machine: machine(), helpers: helpers() }
            .execute_dyn(&ctx, json!({"task_id": "sa-nothing"}))
            .await;
        assert!(r.is_error && r.content.contains("list_runs"), "{}", r.content);
    }

    /// `timeout` is milliseconds, capped at ten minutes. It is how long the
    /// call waits, not a budget that stops the call: a command still running
    /// then moves to the background, so the tool has no timeout of its own.
    #[test]
    fn timeout_is_milliseconds_and_the_call_has_no_budget_of_its_own() {
        assert_eq!(timeout_secs(&json!({})), 120);
        assert_eq!(timeout_secs(&json!({"timeout": 1500})), 2);
        assert_eq!(timeout_secs(&json!({"timeout": 9_000_000})), 600);
        let run = RunCommandTool(machine());
        assert_eq!(run.execution_timeout(&json!({"command": "x", "timeout": 400_000})), None);
        assert_eq!(run.execution_timeout(&json!({"command": "x", "background": true})), None);
    }

    /// The owner reads `description`; a call without one shows the command.
    #[test]
    fn the_label_is_the_description() {
        let run = RunCommandTool(machine());
        let input = json!({"command": "ls -la /tmp", "description": "List the temp folder"});
        assert_eq!((run.activity(&input), run.outcome(&input)), ("List the temp folder".into(), "List the temp folder".into()));
        let bare = json!({"command": format!("echo {}", "x".repeat(100))});
        assert!(run.activity(&bare).starts_with("running `echo x"));
        assert!(run.outcome(&bare).ends_with("…`"), "long commands are cut: {}", run.outcome(&bare));
        assert_eq!(run.rule_field(&input), Some(RuleField::CommandPrefix("ls -la /tmp".into())));
        assert_eq!(run.capability(&input), Some("shell"));
    }

    /// A file the command writes is the run's own change: an edit after it
    /// carries no unread warning.
    #[tokio::test]
    async fn a_file_a_command_writes_is_in_the_ledger() {
        let m = machine();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.txt");
        let ctx = ToolContext::new(crate::origin::Origin::User);
        let r = RunCommandTool(m.clone())
            .execute_dyn(&ctx, json!({"command": format!("echo hello > {}", path.display()), "description": "Write a file"}))
            .await;
        assert!(!r.is_error, "{}", r.content);
        let e = crate::file_tools::EditFileTool(m)
            .execute_dyn(&ctx, json!({"path": path.to_string_lossy(), "old_string": "hello", "new_string": "bye"}))
            .await;
        assert!(!e.is_error && !e.content.contains("WARNING"), "{}", e.content);
    }
}
