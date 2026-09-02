//! Shell-command hooks (coding parity, Stage 4).
//!
//! A repo's `.nebo/hooks.yaml` (walked up from the working directory like
//! `.nebo.md`) declares commands to run around tool calls. They register on
//! the ONE hook dispatcher plugins use (`napp::HookDispatcher`), as filters,
//! so a formatter's or test runner's verdict reaches the model through the
//! same seam a plugin hook would.
//!
//! Exit-code contract, copied from the reference because agents that have
//! read its docs assume it, and documented loudly because the natural
//! `exit 1` on failure is the one that reaches nobody:
//!   0     stdout is attached to the tool result as a titled note
//!   2     stderr reaches the MODEL as an error note (pre: the call is refused)
//!   other stderr is logged for the owner; the model sees nothing
//!
//! ```yaml
//! post_tool:
//!   - name: cargo-test
//!     tool: os            # optional filters; omitted = any
//!     action: [write, edit]
//!     command: cargo test -p web 2>&1 | tail -n 20
//!     timeout_secs: 600   # default 600; the reference's tool-hook default
//! pre_tool:
//!   - name: no-force-push
//!     tool: os
//!     command: 'jq -e ".input.command | test(\"push --force\") | not" >/dev/null || { echo "force push is off-limits" >&2; exit 2; }'
//! ```
//!
//! The payload arrives as JSON on stdin (`ToolPreExecutePayload` /
//! `ToolPostExecutePayload`). A pre hook that exits 0 with a JSON object on
//! stdout carrying `input` rewrites the call's arguments.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tracing::{debug, warn};

/// Default per-hook deadline. Sized for a build, not a linter.
pub const DEFAULT_TIMEOUT_SECS: u64 = 600;
/// Stdout attached to a result is capped here; a hook that wants the model
/// to see less should print less.
pub const NOTE_CAP_CHARS: usize = 4_000;
/// Only this exit code reaches the model.
pub const BLOCKING_EXIT: i32 = 2;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct HooksFile {
    #[serde(default)]
    pub pre_tool: Vec<Hook>,
    #[serde(default)]
    pub post_tool: Vec<Hook>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Hook {
    pub name: String,
    pub command: String,
    /// Tool name filter (e.g. "os"). Omitted = every tool.
    #[serde(default)]
    pub tool: Option<String>,
    /// Resource / action filters on the call's input (`resource`, `action`).
    #[serde(default, deserialize_with = "one_or_many")]
    pub resource: Vec<String>,
    #[serde(default, deserialize_with = "one_or_many")]
    pub action: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

fn one_or_many<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(match Option::<OneOrMany>::deserialize(d)? {
        Some(OneOrMany::One(s)) => vec![s],
        Some(OneOrMany::Many(v)) => v,
        None => Vec::new(),
    })
}

impl Hook {
    /// Does this hook apply to a call? Filters are ANDed; an empty filter
    /// matches everything.
    pub fn matches(&self, tool: &str, input: &serde_json::Value) -> bool {
        if self.tool.as_deref().is_some_and(|t| t != tool) {
            return false;
        }
        let resource = tools::OsTool::resolved_resource(input);
        if !self.resource.is_empty() && !self.resource.iter().any(|r| r == resource) {
            return false;
        }
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
        if !self.action.is_empty() && !self.action.iter().any(|a| a == action) {
            return false;
        }
        true
    }
}

/// Find `.nebo/hooks.yaml` from `start` upward, stopping at the git root or
/// the filesystem root (the `.nebo.md` walk).
pub fn find_hooks_file(start: &Path) -> Option<PathBuf> {
    let mut dir = start;
    loop {
        let candidate = dir.join(".nebo").join("hooks.yaml");
        if candidate.is_file() {
            return Some(candidate);
        }
        if dir.join(".git").exists() {
            return None;
        }
        match dir.parent() {
            Some(p) if p != dir => dir = p,
            _ => return None,
        }
    }
}

pub fn load(path: &Path) -> Result<HooksFile, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let file: HooksFile = serde_yaml::from_str(&raw).map_err(|e| format!("parse {}: {e}", path.display()))?;
    for h in file.pre_tool.iter().chain(file.post_tool.iter()) {
        if h.name.trim().is_empty() || h.command.trim().is_empty() {
            return Err(format!("{}: every hook needs `name` and `command`", path.display()));
        }
    }
    Ok(file)
}

/// What one hook run produced, mapped by the exit-code contract.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Exit 0: stdout (capped) for the model as a note.
    Note(String),
    /// Exit 2: stderr for the model as an error.
    Blocking(String),
    /// Any other exit or a failure to run: stderr for the owner's log only.
    OwnerOnly { exit: Option<i32>, stderr: String },
}

/// Run one hook with the payload on stdin.
pub async fn run(hook: &Hook, payload: &[u8], cwd: &Path) -> Outcome {
    use tokio::io::AsyncWriteExt;
    let (shell, shell_args) = tools::process::shell_command();
    let mut cmd = tokio::process::Command::new(&shell);
    cmd.args(&shell_args)
        .arg(&hook.command)
        .current_dir(if cwd.as_os_str().is_empty() { Path::new(".") } else { cwd })
        .env("NEBO_HOOK", &hook.name)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Outcome::OwnerOnly { exit: None, stderr: format!("could not start: {e}") },
    };
    if let Some(mut stdin) = child.stdin.take() {
        // A hook that does not read stdin closes it; that is not an error.
        let _ = stdin.write_all(payload).await;
        drop(stdin);
    }
    let out = match tokio::time::timeout(Duration::from_secs(hook.timeout_secs), child.wait_with_output()).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => return Outcome::OwnerOnly { exit: None, stderr: format!("could not run: {e}") },
        Err(_) => {
            return Outcome::OwnerOnly {
                exit: None,
                stderr: format!("timed out after {}s", hook.timeout_secs),
            }
        }
    };
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    match out.status.code() {
        Some(0) => Outcome::Note(cap(&stdout)),
        Some(BLOCKING_EXIT) => Outcome::Blocking(cap(if stderr.is_empty() { "(no stderr output)" } else { &stderr })),
        exit => Outcome::OwnerOnly { exit, stderr },
    }
}

fn cap(s: &str) -> String {
    if s.chars().count() <= NOTE_CAP_CHARS {
        s.to_string()
    } else {
        let head: String = s.chars().take(NOTE_CAP_CHARS).collect();
        format!("{head}\n[hook output trimmed to {NOTE_CAP_CHARS} chars]")
    }
}

/// The dispatcher-facing caller: one per hooks file, filtering by hook name.
pub struct ShellHookCaller {
    file: HooksFile,
    cwd: PathBuf,
}

impl ShellHookCaller {
    pub fn new(file: HooksFile, cwd: PathBuf) -> Self {
        Self { file, cwd }
    }

    /// Longest deadline among the hooks, so the dispatcher never cuts a
    /// build short; each hook still runs under its own `timeout_secs`.
    pub fn deadline(&self) -> Duration {
        let max = self
            .file
            .pre_tool
            .iter()
            .chain(self.file.post_tool.iter())
            .map(|h| h.timeout_secs)
            .max()
            .unwrap_or(DEFAULT_TIMEOUT_SECS);
        Duration::from_secs(max + 5)
    }

    async fn pre(&self, payload: Vec<u8>) -> Result<(Vec<u8>, bool), String> {
        let p: crate::hooks::ToolPreExecutePayload = serde_json::from_slice(&payload).map_err(|e| e.to_string())?;
        let mut resp = crate::hooks::ToolPreExecuteResponse { blocked: false, blocked_message: None, input: None };
        let mut current_input = p.input.clone();
        for hook in &self.file.pre_tool {
            if !hook.matches(&p.tool_name, &current_input) {
                continue;
            }
            let body = serde_json::to_vec(&crate::hooks::ToolPreExecutePayload { input: current_input.clone(), ..clone_pre(&p) })
                .map_err(|e| e.to_string())?;
            match run(hook, &body, &self.cwd).await {
                Outcome::Blocking(stderr) => {
                    resp.blocked = true;
                    resp.blocked_message = Some(format!("[hook {}]: {stderr}", hook.name));
                    break;
                }
                Outcome::Note(stdout) => {
                    // Exit 0 with a JSON object carrying `input` rewrites the call.
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&stdout) {
                        if let Some(input) = v.get("input").filter(|i| i.is_object()) {
                            current_input = input.clone();
                            resp.input = Some(input.clone());
                        }
                    }
                }
                Outcome::OwnerOnly { exit, stderr } => {
                    warn!(hook = %hook.name, ?exit, %stderr, "pre-tool hook failed (not shown to the model)");
                }
            }
        }
        Ok((serde_json::to_vec(&resp).map_err(|e| e.to_string())?, resp.blocked))
    }

    async fn post(&self, payload: Vec<u8>) -> Result<(Vec<u8>, bool), String> {
        let p: crate::hooks::ToolPostExecutePayload = serde_json::from_slice(&payload).map_err(|e| e.to_string())?;
        let mut resp = crate::hooks::ToolPostExecuteResponse { result: p.result.clone(), is_error: p.is_error };
        for hook in self.file.post_tool.iter().filter(|h| h.matches(&p.tool_name, &p.tool_input)) {
            match run(hook, &payload, &self.cwd).await {
                Outcome::Note(stdout) if !stdout.is_empty() => {
                    resp.result.push_str(&format!("\n\n[hook {}]\n{stdout}", hook.name));
                }
                Outcome::Note(_) => debug!(hook = %hook.name, "post-tool hook ran quietly"),
                Outcome::Blocking(stderr) => {
                    resp.result.push_str(&format!("\n\n[hook {}]: {stderr}", hook.name));
                    resp.is_error = true;
                }
                Outcome::OwnerOnly { exit, stderr } => {
                    warn!(hook = %hook.name, ?exit, %stderr, "post-tool hook failed (not shown to the model)");
                }
            }
        }
        Ok((serde_json::to_vec(&resp).map_err(|e| e.to_string())?, false))
    }
}

fn clone_pre(p: &crate::hooks::ToolPreExecutePayload) -> crate::hooks::ToolPreExecutePayload {
    crate::hooks::ToolPreExecutePayload {
        tool_name: p.tool_name.clone(),
        input: p.input.clone(),
        session_id: p.session_id.clone(),
        tool_use_id: p.tool_use_id.clone(),
        cwd: p.cwd.clone(),
        agent_id: p.agent_id.clone(),
    }
}

#[async_trait::async_trait]
impl napp::hooks::HookCaller for ShellHookCaller {
    async fn call_filter(&self, hook: &str, payload: Vec<u8>) -> Result<(Vec<u8>, bool), String> {
        match hook {
            "tool.pre_execute" => self.pre(payload).await,
            "tool.post_execute" => self.post(payload).await,
            other => Err(format!("shell hooks do not handle {other}")),
        }
    }
    async fn call_action(&self, _hook: &str, _payload: Vec<u8>) -> Result<(), String> {
        Ok(())
    }
}

/// Load the workspace's hooks file (if any) and register it on the shared
/// dispatcher. Called once at server start; `app_id` keeps it distinct from
/// plugin subscriptions.
pub fn register_workspace_hooks(dispatcher: &napp::HookDispatcher, cwd: &Path) -> Option<PathBuf> {
    let path = find_hooks_file(cwd)?;
    let file = match load(&path) {
        Ok(f) => f,
        Err(e) => {
            warn!(error = %e, "hooks file ignored");
            return None;
        }
    };
    let caller = Arc::new(ShellHookCaller::new(file.clone(), cwd.to_path_buf()));
    let deadline = Some(caller.deadline());
    if !file.pre_tool.is_empty() {
        dispatcher.register_with_timeout("tool.pre_execute", "shell-hooks", napp::hooks::HookType::Filter, 100, caller.clone(), deadline);
    }
    if !file.post_tool.is_empty() {
        dispatcher.register_with_timeout("tool.post_execute", "shell-hooks", napp::hooks::HookType::Filter, 100, caller, deadline);
    }
    debug!(path = %path.display(), pre = file.pre_tool.len(), post = file.post_tool.len(), "shell hooks registered");
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(cmd: &str) -> Hook {
        Hook { name: "t".into(), command: cmd.into(), tool: None, resource: vec![], action: vec![], timeout_secs: 5 }
    }

    #[tokio::test]
    async fn exit_zero_appends_stdout_as_a_note() {
        let out = run(&hook("echo 'tests: 12 passed'"), b"{}", Path::new(".")).await;
        assert_eq!(out, Outcome::Note("tests: 12 passed".into()));
    }

    #[tokio::test]
    async fn exit_two_reaches_the_model_and_sets_is_error() {
        let file = HooksFile { pre_tool: vec![], post_tool: vec![hook("echo 'FAILED: 1 test' >&2; exit 2")] };
        let caller = ShellHookCaller::new(file, PathBuf::from("."));
        let payload = serde_json::to_vec(&crate::hooks::ToolPostExecutePayload {
            tool_name: "os".into(), result: "Edited a.rs".into(), is_error: false, session_id: "s".into(),
            tool_use_id: "c1".into(), tool_input: serde_json::json!({"action": "edit"}), cwd: ".".into(), agent_id: None,
        }).unwrap();
        let (bytes, _) = caller.post(payload).await.unwrap();
        let resp: crate::hooks::ToolPostExecuteResponse = serde_json::from_slice(&bytes).unwrap();
        assert!(resp.is_error);
        assert!(resp.result.starts_with("Edited a.rs"), "the tool's own result stays first");
        assert!(resp.result.contains("[hook t]: FAILED: 1 test"), "{}", resp.result);
    }

    #[tokio::test]
    async fn other_exits_are_owner_only() {
        let file = HooksFile { pre_tool: vec![], post_tool: vec![hook("echo boom >&2; exit 1")] };
        let caller = ShellHookCaller::new(file, PathBuf::from("."));
        let payload = serde_json::to_vec(&crate::hooks::ToolPostExecutePayload {
            tool_name: "os".into(), result: "Edited".into(), is_error: false, session_id: "s".into(),
            tool_use_id: String::new(), tool_input: serde_json::Value::Null, cwd: String::new(), agent_id: None,
        }).unwrap();
        let (bytes, _) = caller.post(payload).await.unwrap();
        let resp: crate::hooks::ToolPostExecuteResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(resp.result, "Edited", "the model sees nothing from an exit-1 hook");
        assert!(!resp.is_error);
    }

    #[tokio::test]
    async fn pre_tool_exit_two_is_a_refusal_and_exit_zero_json_rewrites_input() {
        let file = HooksFile {
            pre_tool: vec![
                Hook { name: "lock".into(), command: r#"echo '{"input":{"action":"exec","command":"cargo build --locked"}}'"#.into(), tool: Some("os".into()), resource: vec![], action: vec!["exec".into()], timeout_secs: 5 },
                Hook { name: "deny".into(), command: "echo 'not here' >&2; exit 2".into(), tool: Some("os".into()), resource: vec![], action: vec!["delete".into()], timeout_secs: 5 },
            ],
            post_tool: vec![],
        };
        let caller = ShellHookCaller::new(file, PathBuf::from("."));
        let pre = |action: &str, command: &str| serde_json::to_vec(&crate::hooks::ToolPreExecutePayload {
            tool_name: "os".into(), input: serde_json::json!({"action": action, "command": command}), session_id: "s".into(),
            tool_use_id: "c".into(), cwd: ".".into(), agent_id: None,
        }).unwrap();
        let (bytes, handled) = caller.pre(pre("exec", "cargo build")).await.unwrap();
        let resp: crate::hooks::ToolPreExecuteResponse = serde_json::from_slice(&bytes).unwrap();
        assert!(!handled && !resp.blocked);
        assert_eq!(resp.input.unwrap()["command"], "cargo build --locked");
        let (bytes, handled) = caller.pre(pre("delete", "")).await.unwrap();
        let resp: crate::hooks::ToolPreExecuteResponse = serde_json::from_slice(&bytes).unwrap();
        assert!(handled && resp.blocked);
        assert_eq!(resp.blocked_message.as_deref(), Some("[hook deny]: not here"));
    }

    #[tokio::test]
    async fn hook_timeout_kills_and_reports() {
        let mut h = hook("sleep 5; echo late");
        h.timeout_secs = 1;
        let started = std::time::Instant::now();
        let out = run(&h, b"{}", Path::new(".")).await;
        assert!(matches!(out, Outcome::OwnerOnly { exit: None, ref stderr } if stderr.contains("timed out")), "{out:?}");
        assert!(started.elapsed() < Duration::from_secs(4));
    }

    #[test]
    fn payload_has_agent_id_only_inside_a_subagent() {
        let main = serde_json::to_value(crate::hooks::ToolPostExecutePayload {
            tool_name: "os".into(), result: "r".into(), is_error: false, session_id: "s".into(),
            tool_use_id: "c".into(), tool_input: serde_json::Value::Null, cwd: "/w".into(), agent_id: None,
        }).unwrap();
        assert!(main.get("agent_id").is_none());
        let sub = serde_json::to_value(crate::hooks::ToolPostExecutePayload {
            tool_name: "os".into(), result: "r".into(), is_error: false, session_id: "subagent:p:t".into(),
            tool_use_id: "c".into(), tool_input: serde_json::Value::Null, cwd: "/w".into(), agent_id: Some("sa-1".into()),
        }).unwrap();
        assert_eq!(sub["agent_id"], "sa-1");
    }

    #[test]
    fn hooks_file_parses_filters_and_finds_itself_from_a_subdirectory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".nebo")).unwrap();
        std::fs::create_dir_all(dir.path().join("src/deep")).unwrap();
        std::fs::write(
            dir.path().join(".nebo/hooks.yaml"),
            "post_tool:\n  - name: fmt\n    tool: os\n    action: [write, edit]\n    command: cargo fmt\n",
        )
        .unwrap();
        let found = find_hooks_file(&dir.path().join("src/deep")).unwrap();
        let file = load(&found).unwrap();
        assert_eq!(file.post_tool[0].timeout_secs, DEFAULT_TIMEOUT_SECS);
        assert!(file.post_tool[0].matches("os", &serde_json::json!({"action": "edit", "path": "a"})));
        assert!(!file.post_tool[0].matches("os", &serde_json::json!({"action": "read", "path": "a"})));
        assert!(!file.post_tool[0].matches("web", &serde_json::json!({"action": "edit"})));
        // A git root stops the walk.
        std::fs::create_dir_all(dir.path().join("other/.git")).unwrap();
        assert!(find_hooks_file(&dir.path().join("other")).is_none());
    }
}
