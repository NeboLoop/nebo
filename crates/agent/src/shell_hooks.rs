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
/// The most a hook may ask for; the dispatcher's own deadline sits just above
/// it, and a file asking for more is clamped (and told so in the log).
pub const MAX_HOOK_TIMEOUT_SECS: u64 = 900;
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
    let mut file = file;
    for h in file.pre_tool.iter_mut().chain(file.post_tool.iter_mut()) {
        if h.name.trim().is_empty() || h.command.trim().is_empty() {
            return Err(format!("{}: every hook needs `name` and `command`", path.display()));
        }
        if h.timeout_secs > MAX_HOOK_TIMEOUT_SECS {
            warn!(hook = %h.name, asked = h.timeout_secs, "hook timeout clamped to {MAX_HOOK_TIMEOUT_SECS}s");
            h.timeout_secs = MAX_HOOK_TIMEOUT_SECS;
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
/// Runs a folder's `.nebo/hooks.yaml` around the tool calls that touch that
/// folder. Registered once, unconditionally; the hooks file is looked up per
/// call from the folder the call works in, so an employee that moves to another
/// project mid-session gets that project's hooks, and a folder without a file
/// gets nothing.
pub struct ShellHookCaller {
    /// Parsed files by path, with the mtime they were read at.
    cache: std::sync::Mutex<std::collections::HashMap<PathBuf, (std::time::SystemTime, Arc<HooksFile>)>>,
}

impl Default for ShellHookCaller {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellHookCaller {
    pub fn new() -> Self {
        Self { cache: std::sync::Mutex::new(std::collections::HashMap::new()) }
    }

    /// The folder a call works in: its own `cwd` (shell), the folder of its
    /// `path` (file actions; a not-yet-written file still has a folder), else
    /// the run's cwd, else the process cwd. Relative paths resolve against
    /// the run's cwd.
    pub fn workspace_for(input: &serde_json::Value, run_cwd: &str) -> PathBuf {
        let base = if run_cwd.is_empty() {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        } else {
            PathBuf::from(run_cwd)
        };
        if let Some(cwd) = input.get("cwd").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            return base.join(cwd);
        }
        if let Some(path) = input.get("path").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            let full = base.join(path);
            return if full.is_dir() { full } else { full.parent().map(Path::to_path_buf).unwrap_or(base) };
        }
        base
    }

    /// The hooks file for a workspace and the project root to run hooks in
    /// (the folder holding `.nebo`). Re-read when the file's mtime changes.
    fn resolve(&self, workspace: &Path) -> Option<(Arc<HooksFile>, PathBuf)> {
        let path = find_hooks_file(workspace)?;
        let root = path.parent()?.parent()?.to_path_buf();
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
        if let Ok(cache) = self.cache.lock() {
            if let Some((seen, file)) = cache.get(&path) {
                if *seen == mtime {
                    return Some((file.clone(), root));
                }
            }
        }
        let file = match load(&path) {
            Ok(f) => Arc::new(f),
            Err(e) => {
                warn!(error = %e, "hooks file ignored");
                return None;
            }
        };
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(path, (mtime, file.clone()));
        }
        Some((file, root))
    }

    async fn pre(&self, payload: Vec<u8>) -> Result<(Vec<u8>, bool), String> {
        let p: crate::hooks::ToolPreExecutePayload = serde_json::from_slice(&payload).map_err(|e| e.to_string())?;
        let mut resp = crate::hooks::ToolPreExecuteResponse { blocked: false, blocked_message: None, input: None };
        let Some((file, root)) = self.resolve(&Self::workspace_for(&p.input, &p.cwd)) else {
            return Ok((serde_json::to_vec(&resp).map_err(|e| e.to_string())?, false));
        };
        let mut current_input = p.input.clone();
        for hook in &file.pre_tool {
            if !hook.matches(&p.tool_name, &current_input) {
                continue;
            }
            let body = serde_json::to_vec(&crate::hooks::ToolPreExecutePayload { input: current_input.clone(), ..clone_pre(&p) })
                .map_err(|e| e.to_string())?;
            match run(hook, &body, &root).await {
                Outcome::Blocking(stderr) => {
                    resp.blocked = true;
                    resp.blocked_message = Some(format!("[hook {}]: {stderr}", hook.name));
                    break;
                }
                Outcome::Note(stdout) => {
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
        let Some((file, root)) = self.resolve(&Self::workspace_for(&p.tool_input, &p.cwd)) else {
            return Ok((serde_json::to_vec(&resp).map_err(|e| e.to_string())?, false));
        };
        for hook in file.post_tool.iter().filter(|h| h.matches(&p.tool_name, &p.tool_input)) {
            match run(hook, &payload, &root).await {
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

/// Register the shell-hook caller on the ONE dispatcher plugins use. Called
/// once at startup with no folder in hand: which `.nebo/hooks.yaml` applies is
/// decided per call from the folder that call works in.
pub fn register_workspace_hooks(dispatcher: &napp::HookDispatcher) {
    let caller = Arc::new(ShellHookCaller::new());
    let deadline = Some(Duration::from_secs(MAX_HOOK_TIMEOUT_SECS + 5));
    dispatcher.register_with_timeout("tool.pre_execute", "shell-hooks", napp::hooks::HookType::Filter, 100, caller.clone(), deadline);
    dispatcher.register_with_timeout("tool.post_execute", "shell-hooks", napp::hooks::HookType::Filter, 100, caller, deadline);
    debug!("shell hooks registered; .nebo/hooks.yaml is resolved per call");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(cmd: &str) -> Hook {
        Hook { name: "t".into(), command: cmd.into(), tool: None, resource: vec![], action: vec![], timeout_secs: 5 }
    }

    /// A project folder with a `.nebo/hooks.yaml` (and a `.git` so the walk
    /// stops there, never at some stray file above the temp dir).
    fn project(yaml: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::create_dir_all(dir.path().join(".nebo")).unwrap();
        std::fs::write(dir.path().join(".nebo/hooks.yaml"), yaml).unwrap();
        dir
    }

    fn post_payload(cwd: &str, input: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&crate::hooks::ToolPostExecutePayload {
            tool_name: "os".into(), result: "Edited a.rs".into(), is_error: false, session_id: "s".into(),
            tool_use_id: "c1".into(), tool_input: input, cwd: cwd.into(), agent_id: None,
        }).unwrap()
    }

    fn pre_payload(cwd: &str, input: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&crate::hooks::ToolPreExecutePayload {
            tool_name: "os".into(), input, session_id: "s".into(), tool_use_id: "c".into(), cwd: cwd.into(), agent_id: None,
        }).unwrap()
    }

    #[tokio::test]
    async fn exit_zero_appends_stdout_as_a_note() {
        let out = run(&hook("echo 'tests: 12 passed'"), b"{}", Path::new(".")).await;
        assert_eq!(out, Outcome::Note("tests: 12 passed".into()));
    }

    #[tokio::test]
    async fn exit_two_reaches_the_model_and_sets_is_error() {
        let p = project("post_tool:\n  - name: t\n    command: \"echo 'FAILED: 1 test' >&2; exit 2\"\n");
        let caller = ShellHookCaller::new();
        let (bytes, _) = caller.post(post_payload(p.path().to_str().unwrap(), serde_json::json!({"action": "edit"}))).await.unwrap();
        let resp: crate::hooks::ToolPostExecuteResponse = serde_json::from_slice(&bytes).unwrap();
        assert!(resp.is_error);
        assert!(resp.result.starts_with("Edited a.rs"), "the tool's own result stays first");
        assert!(resp.result.contains("[hook t]: FAILED: 1 test"), "{}", resp.result);
    }

    #[tokio::test]
    async fn other_exits_are_owner_only() {
        let p = project("post_tool:\n  - name: t\n    command: \"echo boom >&2; exit 1\"\n");
        let caller = ShellHookCaller::new();
        let (bytes, _) = caller.post(post_payload(p.path().to_str().unwrap(), serde_json::Value::Null)).await.unwrap();
        let resp: crate::hooks::ToolPostExecuteResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(resp.result, "Edited a.rs", "the model sees nothing from an exit-1 hook");
        assert!(!resp.is_error);
    }

    #[tokio::test]
    async fn pre_tool_exit_two_is_a_refusal_and_exit_zero_json_rewrites_input() {
        let p = project(concat!(
            "pre_tool:\n",
            "  - name: lock\n    tool: os\n    action: exec\n",
            "    command: \"echo '{\\\"input\\\":{\\\"action\\\":\\\"exec\\\",\\\"command\\\":\\\"cargo build --locked\\\"}}'\"\n",
            "  - name: deny\n    tool: os\n    action: delete\n    command: \"echo 'not here' >&2; exit 2\"\n",
        ));
        let cwd = p.path().to_str().unwrap();
        let caller = ShellHookCaller::new();
        let (bytes, handled) = caller.pre(pre_payload(cwd, serde_json::json!({"action": "exec", "command": "cargo build"}))).await.unwrap();
        let resp: crate::hooks::ToolPreExecuteResponse = serde_json::from_slice(&bytes).unwrap();
        assert!(!handled && !resp.blocked);
        assert_eq!(resp.input.unwrap()["command"], "cargo build --locked");
        let (bytes, handled) = caller.pre(pre_payload(cwd, serde_json::json!({"action": "delete", "command": ""}))).await.unwrap();
        let resp: crate::hooks::ToolPreExecuteResponse = serde_json::from_slice(&bytes).unwrap();
        assert!(handled && resp.blocked);
        assert_eq!(resp.blocked_message.as_deref(), Some("[hook deny]: not here"));
    }

    /// The gap this closes (Stage 4 known limit): hooks used to load once from
    /// the server's folder. Now the folder a call touches decides, so two
    /// projects in one session each get their own file, a call whose `path`
    /// lies in another project follows the path, and a folder with no file
    /// runs nothing. One caller serves all of them.
    #[tokio::test]
    async fn hooks_follow_the_folder_the_call_touches() {
        let a = project("post_tool:\n  - name: which\n    command: echo project-A\n");
        let b = project("post_tool:\n  - name: which\n    command: echo project-B\n");
        let none = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(none.path().join(".git")).unwrap();
        let caller = ShellHookCaller::new();
        let note = |bytes: Vec<u8>| -> String {
            let r: crate::hooks::ToolPostExecuteResponse = serde_json::from_slice(&bytes).unwrap();
            r.result
        };
        // run cwd in A, relative path: A's hook
        let (bytes, _) = caller.post(post_payload(a.path().to_str().unwrap(), serde_json::json!({"action": "edit", "path": "src/lib.rs"}))).await.unwrap();
        assert!(note(bytes).ends_with("[hook which]\nproject-A"));
        // run cwd still A, but the call's path is inside B: B's hook
        let in_b = b.path().join("src/main.rs");
        let (bytes, _) = caller.post(post_payload(a.path().to_str().unwrap(), serde_json::json!({"action": "write", "path": in_b}))).await.unwrap();
        assert!(note(bytes).ends_with("[hook which]\nproject-B"));
        // a shell call with its own cwd in B
        let (bytes, _) = caller.post(post_payload(a.path().to_str().unwrap(), serde_json::json!({"action": "exec", "cwd": b.path()}))).await.unwrap();
        assert!(note(bytes).ends_with("[hook which]\nproject-B"));
        // a folder with no hooks file: untouched result
        let (bytes, _) = caller.post(post_payload(none.path().to_str().unwrap(), serde_json::json!({"action": "edit", "path": "x.rs"}))).await.unwrap();
        assert_eq!(note(bytes), "Edited a.rs");
    }

    #[tokio::test]
    async fn an_edited_hooks_file_is_reread_and_an_oversized_timeout_is_clamped() {
        let p = project("post_tool:\n  - name: which\n    command: echo one\n    timeout_secs: 99999\n");
        let caller = ShellHookCaller::new();
        let cwd = p.path().to_str().unwrap();
        let (bytes, _) = caller.post(post_payload(cwd, serde_json::json!({"action": "edit", "path": "a"}))).await.unwrap();
        let r: crate::hooks::ToolPostExecuteResponse = serde_json::from_slice(&bytes).unwrap();
        assert!(r.result.ends_with("one"));
        let (file, _) = caller.resolve(p.path()).unwrap();
        assert_eq!(file.post_tool[0].timeout_secs, MAX_HOOK_TIMEOUT_SECS);
        // Land the rewrite on a later mtime tick so the cache sees a change.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(p.path().join(".nebo/hooks.yaml"), "post_tool:\n  - name: which\n    command: echo two\n").unwrap();
        let (bytes, _) = caller.post(post_payload(cwd, serde_json::json!({"action": "edit", "path": "a"}))).await.unwrap();
        let r: crate::hooks::ToolPostExecuteResponse = serde_json::from_slice(&bytes).unwrap();
        assert!(r.result.ends_with("two"), "{}", r.result);
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
