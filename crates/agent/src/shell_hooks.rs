//! Shell-command hooks (coding parity, Stage 4).
//!
//! A repo's `.nebo/hooks.yaml` (walked up from the working directory like
//! `.nebo.md`) declares commands to run around tool calls. They register on
//! the ONE hook dispatcher plugins use (`napp::HookDispatcher`), as filters,
//! so a formatter's or test runner's verdict reaches the model through the
//! same seam a plugin hook would.
//!
//! Exit-code contract. The first two rows are the reference's, kept because
//! agents that have read its docs assume them. The rest used to be "logged for
//! the owner; the model sees nothing", which made the natural `exit 1` on a
//! failing check the one outcome nobody acted on (live 2026-09-03: a test
//! runner failed on every edit and the model reported done):
//!   0        stdout is attached to the tool result as a titled note
//!   2        stderr reaches the MODEL as an error note (pre: the call is refused)
//!   other    `[hook <name>] exited <code>:` + the last NOTE_CAP_CHARS of
//!            stderr, then stdout, reaches the model. post: as an error note
//!            (`is_error`). pre: as a note on the call's result; the call
//!            still runs, a failing pre hook never blocks
//!   timeout  `[hook <name>] did not finish within <n> s`, routed like "other"
//!   no start could not spawn or wait: `[hook <name>] could not run: <why>`,
//!            routed like "other"
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
//!
//! No hooks file at all: the project's own check is inferred from the nearest
//! workspace marker (`infer_hook`) and runs as a post hook on `os` write/edit,
//! through this same pathway. A hooks file that exists, even one declaring
//! `post_tool: []`, is the owner's explicit answer and disables inference.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use tracing::{debug, warn};

/// Default per-hook deadline. Sized for a build, not a linter.
pub const DEFAULT_TIMEOUT_SECS: u64 = 600;
/// The most a hook may ask for; the dispatcher's own deadline sits just above
/// it, and a file asking for more is clamped (and told so in the log).
pub const MAX_HOOK_TIMEOUT_SECS: u64 = 900;
/// Output attached to a result is capped here (stdout keeps its head, a
/// failure keeps its tail, where compilers put the verdict); a hook that
/// wants the model to see less should print less.
pub const NOTE_CAP_CHARS: usize = 4_000;
/// The exit code whose stderr is the model-facing verdict, and the only one
/// that refuses a pre call.
pub const BLOCKING_EXIT: i32 = 2;
/// Deadline for an inferred project check: long enough for an incremental
/// `cargo check`, short enough that a cold build is handed back to the model
/// to run itself instead of stalling every edit.
pub const INFERRED_CHECK_TIMEOUT_SECS: u64 = 120;
/// A burst of edits to one root pays for one inferred check: a second edit
/// within this window skips the check (debug log only).
pub const INFERRED_CHECK_DEBOUNCE_SECS: u64 = 30;
/// Inferred commands pipe through `tail`, which would launder the check's
/// exit code into tail's `0`; bash's pipefail keeps the verdict truthful.
/// PowerShell has no pipefail and no `tail`; the inferred commands are the
/// spec's and run as written there.
#[cfg(not(target_os = "windows"))]
const PIPEFAIL_PREFIX: &str = "set -o pipefail; ";
#[cfg(target_os = "windows")]
const PIPEFAIL_PREFIX: &str = "";

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
    /// Synthesized by `infer_hook` rather than declared in a file: debounced
    /// per root, and its timeout note tells the model to run the check itself.
    #[serde(skip)]
    pub inferred: bool,
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

/// Walk from `start` upward, asking `probe` at each folder, stopping after
/// the git root or at the filesystem root (the `.nebo.md` walk). The git
/// root itself is probed before the walk stops there.
fn find_up<T>(start: &Path, probe: impl Fn(&Path) -> Option<T>) -> Option<T> {
    let mut dir = start;
    loop {
        if let Some(found) = probe(dir) {
            return Some(found);
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

/// Find `.nebo/hooks.yaml` from `start` upward.
pub fn find_hooks_file(start: &Path) -> Option<PathBuf> {
    find_up(start, |dir| {
        let candidate = dir.join(".nebo").join("hooks.yaml");
        candidate.is_file().then_some(candidate)
    })
}

/// The project check a folder's root marker implies, when the folder has no
/// hooks file. First marker wins: Cargo, then Go, then a TypeScript project
/// with its own `tsc`, then a Python project with `ruff` on PATH (`on_path`
/// is the PATH lookup, injected so the table is testable without one).
/// Returns None for a folder with no marker; callers walk up (`find_up`).
pub fn infer_hook(dir: &Path, on_path: &dyn Fn(&str) -> bool) -> Option<Hook> {
    let (tool, check) = if dir.join("Cargo.toml").is_file() {
        ("cargo", "cargo check -q --message-format=short 2>&1 | tail -n 40")
    } else if dir.join("go.mod").is_file() {
        ("go", "go vet ./... 2>&1 | tail -n 40")
    } else if dir.join("tsconfig.json").is_file() && dir.join("node_modules/.bin/tsc").is_file() {
        ("tsc", "node_modules/.bin/tsc --noEmit -p . 2>&1 | tail -n 40")
    } else if (dir.join("pyproject.toml").is_file() || dir.join("setup.py").is_file()) && on_path("ruff") {
        ("ruff", "ruff check . 2>&1 | tail -n 40")
    } else {
        return None;
    };
    Some(Hook {
        name: format!("inferred-{tool}"),
        command: format!("{PIPEFAIL_PREFIX}{check}"),
        tool: Some("os".into()),
        resource: Vec::new(),
        action: vec!["write".into(), "edit".into()],
        timeout_secs: INFERRED_CHECK_TIMEOUT_SECS,
        inferred: true,
    })
}

/// Is an inferred check for a root due, given when it last ran? `None` means
/// it never ran this process.
pub fn inferred_check_due(last_run: Option<Instant>, now: Instant) -> bool {
    last_run.is_none_or(|at| now.duration_since(at) >= Duration::from_secs(INFERRED_CHECK_DEBOUNCE_SECS))
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
    /// Any other exit, the deadline, or a failure to start: the model-facing
    /// note, already titled `[hook <name>] ...` and capped. Post hooks attach
    /// it as an error; pre hooks attach it as a note and let the call run.
    Failed(String),
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
        Err(e) => return Outcome::Failed(format!("[hook {}] could not run: could not start: {e}", hook.name)),
    };
    if let Some(mut stdin) = child.stdin.take() {
        // A hook that does not read stdin closes it; that is not an error.
        let _ = stdin.write_all(payload).await;
        drop(stdin);
    }
    let out = match tokio::time::timeout(Duration::from_secs(hook.timeout_secs), child.wait_with_output()).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => return Outcome::Failed(format!("[hook {}] could not run: {e}", hook.name)),
        Err(_) => {
            let mut note = format!("[hook {}] did not finish within {} s", hook.name, hook.timeout_secs);
            if hook.inferred {
                note.push_str(&format!("; run `{}` yourself and fix what it reports", hook.command.trim_start_matches(PIPEFAIL_PREFIX)));
            }
            return Outcome::Failed(note);
        }
    };
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    match out.status.code() {
        Some(0) => Outcome::Note(cap(&stdout, false)),
        Some(BLOCKING_EXIT) => Outcome::Blocking(cap(if stderr.is_empty() { "(no stderr output)" } else { &stderr }, false)),
        exit => {
            let output = [stderr, stdout].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n");
            let output = if output.is_empty() { "(no output)".to_string() } else { cap(&output, true) };
            let exit = exit.map_or_else(|| "was killed by a signal".to_string(), |c| format!("exited {c}"));
            Outcome::Failed(format!("[hook {}] {exit}:\n{output}", hook.name))
        }
    }
}

/// Cap text at NOTE_CAP_CHARS, keeping the head (a note reads top-down) or
/// the tail (a failing check ends with its verdict).
fn cap(s: &str, keep_tail: bool) -> String {
    let total = s.chars().count();
    if total <= NOTE_CAP_CHARS {
        return s.to_string();
    }
    if keep_tail {
        let tail: String = s.chars().skip(total - NOTE_CAP_CHARS).collect();
        format!("[hook output trimmed to the last {NOTE_CAP_CHARS} chars]\n{tail}")
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
/// gets its inferred check (or nothing, when it has no root marker).
pub struct ShellHookCaller {
    /// Parsed files by path, with the mtime they were read at.
    cache: std::sync::Mutex<std::collections::HashMap<PathBuf, (std::time::SystemTime, Arc<HooksFile>)>>,
    /// When an inferred check last started, by project root (the debounce).
    inferred_runs: std::sync::Mutex<std::collections::HashMap<PathBuf, Instant>>,
}

impl Default for ShellHookCaller {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellHookCaller {
    pub fn new() -> Self {
        Self {
            cache: std::sync::Mutex::new(std::collections::HashMap::new()),
            inferred_runs: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
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

    /// The hooks for a workspace and the project root to run them in: the
    /// folder holding `.nebo` when a hooks file exists (re-read when its
    /// mtime changes; a file that fails to parse is ignored, and still
    /// disables inference), else the nearest folder with a root marker and
    /// its inferred check.
    fn resolve(&self, workspace: &Path) -> Option<(Arc<HooksFile>, PathBuf)> {
        let Some(path) = find_hooks_file(workspace) else {
            let (hook, root) = find_up(workspace, |dir| {
                infer_hook(dir, &|bin| which::which(bin).is_ok()).map(|h| (h, dir.to_path_buf()))
            })?;
            return Some((Arc::new(HooksFile { pre_tool: Vec::new(), post_tool: vec![hook] }), root));
        };
        let root = path.parent()?.parent()?.to_path_buf();
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
        if let Ok(cache) = self.cache.lock()
            && let Some((seen, file)) = cache.get(&path)
            && *seen == mtime
        {
            return Some((file.clone(), root));
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

    /// The debounce: may an inferred check start for `root` now? Records the
    /// start when it says yes, so a burst of edits pays once.
    fn inferred_check_permitted(&self, root: &Path) -> bool {
        let now = Instant::now();
        let Ok(mut runs) = self.inferred_runs.lock() else {
            return true;
        };
        if !inferred_check_due(runs.get(root).copied(), now) {
            return false;
        }
        runs.insert(root.to_path_buf(), now);
        true
    }

    async fn pre(&self, payload: Vec<u8>) -> Result<(Vec<u8>, bool), String> {
        let p: crate::hooks::ToolPreExecutePayload = serde_json::from_slice(&payload).map_err(|e| e.to_string())?;
        let mut resp = crate::hooks::ToolPreExecuteResponse { blocked: false, blocked_message: None, input: None, note: None };
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
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&stdout)
                        && let Some(input) = v.get("input").filter(|i| i.is_object())
                    {
                        current_input = input.clone();
                        resp.input = Some(input.clone());
                    }
                }
                Outcome::Failed(note) => {
                    warn!(hook = %hook.name, %note, "pre-tool hook failed; the call runs and the model is told");
                    let joined = resp.note.take().map_or(note.clone(), |prev| format!("{prev}\n\n{note}"));
                    resp.note = Some(joined);
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
            if hook.inferred && !self.inferred_check_permitted(&root) {
                debug!(hook = %hook.name, root = %root.display(), "inferred check ran within the debounce window; skipped");
                continue;
            }
            match run(hook, &payload, &root).await {
                Outcome::Note(stdout) if !stdout.is_empty() => {
                    resp.result.push_str(&format!("\n\n[hook {}]\n{stdout}", hook.name));
                }
                Outcome::Note(_) => debug!(hook = %hook.name, "post-tool hook ran quietly"),
                Outcome::Blocking(stderr) => {
                    resp.result.push_str(&format!("\n\n[hook {}]: {stderr}", hook.name));
                    resp.is_error = true;
                }
                Outcome::Failed(note) => {
                    warn!(hook = %hook.name, %note, "post-tool hook failed; shown to the model as an error");
                    resp.result.push_str(&format!("\n\n{note}"));
                    resp.is_error = true;
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
        Hook { name: "t".into(), command: cmd.into(), tool: None, resource: vec![], action: vec![], timeout_secs: 5, inferred: false }
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

    /// A project folder with a `.git` and no hooks file: inference territory.
    fn bare_project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
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

    /// The exit code a failing `cargo test` actually produces. It used to be
    /// owner-only; this test fails if exit 1 goes back to being silent.
    #[tokio::test]
    async fn exit_one_reaches_the_model_as_an_error_note() {
        let p = project("post_tool:\n  - name: t\n    command: \"echo boom >&2; echo 'test x ... FAILED'; exit 1\"\n");
        let caller = ShellHookCaller::new();
        let (bytes, _) = caller.post(post_payload(p.path().to_str().unwrap(), serde_json::Value::Null)).await.unwrap();
        let resp: crate::hooks::ToolPostExecuteResponse = serde_json::from_slice(&bytes).unwrap();
        assert!(resp.is_error, "a non-zero exit is an error the model must see");
        assert!(resp.result.starts_with("Edited a.rs"), "the tool's own result stays first");
        assert_eq!(
            resp.result,
            "Edited a.rs\n\n[hook t] exited 1:\nboom\ntest x ... FAILED",
            "stderr first, then stdout, under the exit-code title"
        );
    }

    #[tokio::test]
    async fn failure_output_keeps_its_tail_and_a_silent_failure_says_so() {
        let out = run(&hook("exit 3"), b"{}", Path::new(".")).await;
        assert_eq!(out, Outcome::Failed("[hook t] exited 3:\n(no output)".into()));
        // 6000 numbered lines on stdout: the verdict at the bottom survives, the top is cut.
        let out = run(&hook("seq 1 6000; exit 1"), b"{}", Path::new(".")).await;
        let Outcome::Failed(note) = out else { panic!("{out:?}") };
        assert!(note.starts_with(&format!("[hook t] exited 1:\n[hook output trimmed to the last {NOTE_CAP_CHARS} chars]\n")), "{note}");
        assert!(note.ends_with("\n5999\n6000"), "{}", &note[note.len() - 40..]);
        assert!(!note.contains("\n1\n2\n"), "the head is what gets cut");
    }

    #[tokio::test]
    async fn pre_hook_failure_is_a_note_not_a_block() {
        let p = project("pre_tool:\n  - name: lint\n    command: \"echo 'lint crashed' >&2; exit 1\"\n");
        let caller = ShellHookCaller::new();
        let (bytes, handled) = caller.pre(pre_payload(p.path().to_str().unwrap(), serde_json::json!({"action": "edit", "path": "a"}))).await.unwrap();
        let resp: crate::hooks::ToolPreExecuteResponse = serde_json::from_slice(&bytes).unwrap();
        assert!(!handled && !resp.blocked, "only exit 2 blocks");
        assert_eq!(resp.note.as_deref(), Some("[hook lint] exited 1:\nlint crashed"));
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
    /// and no root marker runs nothing. One caller serves all of them.
    #[tokio::test]
    async fn hooks_follow_the_folder_the_call_touches() {
        let a = project("post_tool:\n  - name: which\n    command: echo project-A\n");
        let b = project("post_tool:\n  - name: which\n    command: echo project-B\n");
        let none = bare_project();
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
        // a folder with no hooks file and no root marker: untouched result
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
    async fn hook_timeout_kills_and_reports_the_deadline() {
        let mut h = hook("sleep 5; echo late");
        h.timeout_secs = 1;
        let started = std::time::Instant::now();
        let out = run(&h, b"{}", Path::new(".")).await;
        assert_eq!(out, Outcome::Failed("[hook t] did not finish within 1 s".into()));
        assert!(started.elapsed() < Duration::from_secs(4));
        // An inferred check that times out hands the command back to the model.
        h.inferred = true;
        let out = run(&h, b"{}", Path::new(".")).await;
        assert_eq!(out, Outcome::Failed("[hook t] did not finish within 1 s; run `sleep 5; echo late` yourself and fix what it reports".into()));
    }

    /// The marker-to-command table and its precedence, on a fake root: no
    /// real cargo/go/tsc/ruff is invoked, only the files are consulted.
    #[test]
    fn inferred_check_follows_the_marker_table_in_priority_order() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let ruff_present = |bin: &str| bin == "ruff";
        let no_ruff = |_: &str| false;
        assert!(infer_hook(d, &ruff_present).is_none(), "no marker, no check");

        std::fs::write(d.join("setup.py"), "").unwrap();
        assert!(infer_hook(d, &no_ruff).is_none(), "a Python project without ruff on PATH gets no check");
        let h = infer_hook(d, &ruff_present).unwrap();
        assert_eq!(h.name, "inferred-ruff");
        assert!(h.command.ends_with("ruff check . 2>&1 | tail -n 40"), "{}", h.command);
        std::fs::remove_file(d.join("setup.py")).unwrap();
        std::fs::write(d.join("pyproject.toml"), "").unwrap();
        assert_eq!(infer_hook(d, &ruff_present).unwrap().name, "inferred-ruff", "pyproject.toml is the other Python marker");

        std::fs::write(d.join("tsconfig.json"), "{}").unwrap();
        assert_eq!(infer_hook(d, &ruff_present).unwrap().name, "inferred-ruff", "tsconfig without a local tsc is not a TypeScript check");
        std::fs::create_dir_all(d.join("node_modules/.bin")).unwrap();
        std::fs::write(d.join("node_modules/.bin/tsc"), "").unwrap();
        let h = infer_hook(d, &ruff_present).unwrap();
        assert_eq!(h.name, "inferred-tsc");
        assert!(h.command.ends_with("node_modules/.bin/tsc --noEmit -p . 2>&1 | tail -n 40"), "{}", h.command);

        std::fs::write(d.join("go.mod"), "module x\n").unwrap();
        let h = infer_hook(d, &ruff_present).unwrap();
        assert_eq!(h.name, "inferred-go");
        assert!(h.command.ends_with("go vet ./... 2>&1 | tail -n 40"), "{}", h.command);

        std::fs::write(d.join("Cargo.toml"), "[package]\n").unwrap();
        let h = infer_hook(d, &no_ruff).unwrap();
        assert_eq!(h.name, "inferred-cargo");
        assert!(h.command.ends_with("cargo check -q --message-format=short 2>&1 | tail -n 40"), "{}", h.command);
        // The shape every inferred hook shares.
        assert!(h.inferred);
        assert_eq!(h.timeout_secs, INFERRED_CHECK_TIMEOUT_SECS);
        assert_eq!(h.tool.as_deref(), Some("os"));
        assert_eq!(h.action, vec!["write".to_string(), "edit".to_string()]);
        assert!(h.matches("os", &serde_json::json!({"action": "edit", "path": "a.rs"})));
        assert!(!h.matches("os", &serde_json::json!({"action": "read", "path": "a.rs"})));
        assert!(!h.matches("os", &serde_json::json!({"action": "exec", "command": "ls"})));
    }

    #[test]
    fn inferred_check_is_debounced_per_root() {
        let now = Instant::now();
        assert!(inferred_check_due(None, now), "never ran: due");
        assert!(!inferred_check_due(Some(now), now), "just ran: not due");
        let window = Duration::from_secs(INFERRED_CHECK_DEBOUNCE_SECS);
        assert!(!inferred_check_due(now.checked_sub(window - Duration::from_secs(1)), now), "inside the window");
        assert!(inferred_check_due(now.checked_sub(window), now), "at the window's edge");
        // The live gate records the start, so the second edit to a root skips
        // and another root is unaffected.
        let caller = ShellHookCaller::new();
        assert!(caller.inferred_check_permitted(Path::new("/a")));
        assert!(!caller.inferred_check_permitted(Path::new("/a")), "a burst pays once");
        assert!(caller.inferred_check_permitted(Path::new("/b")), "per root");
    }

    #[test]
    fn a_hooks_file_even_an_empty_one_disables_inference() {
        // No file, a Cargo root, a call from a subfolder: the inferred cargo
        // check, run in the marker's folder. Nothing is executed here.
        let bare = bare_project();
        std::fs::write(bare.path().join("Cargo.toml"), "[package]\n").unwrap();
        std::fs::create_dir_all(bare.path().join("src")).unwrap();
        let caller = ShellHookCaller::new();
        let (file, root) = caller.resolve(&bare.path().join("src")).unwrap();
        assert_eq!(root, bare.path());
        assert!(file.pre_tool.is_empty());
        assert_eq!(file.post_tool.len(), 1);
        assert_eq!(file.post_tool[0].name, "inferred-cargo");
        assert!(file.post_tool[0].inferred);
        // The same root with an explicit `post_tool: []`: the owner's answer wins.
        let explicit = project("post_tool: []\n");
        std::fs::write(explicit.path().join("Cargo.toml"), "[package]\n").unwrap();
        let (file, root) = caller.resolve(explicit.path()).unwrap();
        assert_eq!(root, explicit.path());
        assert!(file.post_tool.is_empty(), "an explicit empty file runs nothing and infers nothing");
        // A bare root with no marker: nothing at all.
        let empty = bare_project();
        assert!(caller.resolve(empty.path()).is_none());
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
        assert!(!file.post_tool[0].inferred, "a declared hook is never debounced");
        assert!(file.post_tool[0].matches("os", &serde_json::json!({"action": "edit", "path": "a"})));
        assert!(!file.post_tool[0].matches("os", &serde_json::json!({"action": "read", "path": "a"})));
        assert!(!file.post_tool[0].matches("web", &serde_json::json!({"action": "edit"})));
        // A git root stops the walk.
        std::fs::create_dir_all(dir.path().join("other/.git")).unwrap();
        assert!(find_hooks_file(&dir.path().join("other")).is_none());
    }
}
