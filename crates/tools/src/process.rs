use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::debug;
use uuid::Uuid;

/// Every background session's id starts with this, which is how a `bg-…`
/// id is told apart from a helper's or a run's.
pub const SESSION_ID_PREFIX: &str = "bg-";

/// Background sessions alive at once. Past this the model is told to end one:
/// each is a process tree that nothing else will ever stop.
pub const MAX_BACKGROUND_SESSIONS: usize = 8;

/// Take the complete UTF-8 prefix out of `carry`, leaving any partial
/// trailing character for the next read. A 4 KB read can split a multi-byte
/// character; the old code dropped the whole chunk when it did, so non-ASCII
/// output arrived with holes.
fn drain_utf8(carry: &mut Vec<u8>) -> String {
    let valid = match std::str::from_utf8(carry) {
        Ok(_) => carry.len(),
        Err(e) => e.valid_up_to(),
    };
    let text = String::from_utf8_lossy(&carry[..valid]).into_owned();
    carry.drain(..valid);
    // Bytes that can never complete a character (an invalid sequence, not a
    // partial one) must not sit in the carry forever.
    if let Err(e) = std::str::from_utf8(carry)
        && e.error_len().is_some()
    {
        let bad = String::from_utf8_lossy(carry).into_owned();
        carry.clear();
        return text + bad.as_str();
    }
    text
}

/// Every command runs as the leader of its own process group, and the group
/// is what gets killed. Killing only the `sh -c` leaves whatever it started
/// (a dev server, a watcher, a `&` job) reparented to init, running forever.
pub fn in_own_group(cmd: &mut Command) {
    #[cfg(unix)]
    cmd.process_group(0);
}

/// Kill a whole process group. Nothing to do for pid 0 or off unix.
pub fn kill_group(pid: u32) {
    #[cfg(unix)]
    if pid > 0 {
        // SAFETY: killpg on a pid this process spawned; a stale pgid returns ESRCH.
        unsafe {
            libc::killpg(pid as libc::pid_t, libc::SIGKILL);
        }
    }
}

/// How long this module waits for a killed child to be reaped before it stops
/// waiting.
///
/// A process in uninterruptible sleep — `D` state, typically blocked inside a
/// directory read on a FUSE or virtiofs mount — cannot be killed by anyone,
/// root included: the SIGKILL is recorded and only lands if the kernel ever
/// returns from that call, which it may never do. Waiting on such a child
/// therefore never ends, so every kill here is "signal the group, reap with
/// this bound, move on": the tool gives its answer, and the stuck process
/// stays until the mount answers or the box reboots. Not walking into such a
/// mount in the first place is the only thing that prevents it; that is what
/// `walk_bounds::FOREIGN_FS` and `find -xdev` are for.
const REAP_BOUND: std::time::Duration = std::time::Duration::from_secs(2);

/// A child spawned as the leader of its own process group, so that killing it
/// kills the whole tree it started.
///
/// This is the one way a tool starts a child it intends to stop on a deadline.
/// The gate ran `sh -c 'find / -name "image.png" | head -10'` through the
/// shell door; killing the `sh` alone left the `find` running, reparented to
/// init, still walking — and on the CI VM stuck for good in `D` state on a
/// virtiofs mount, sixteen of them at a load average near 22. The group is
/// what gets signalled, so an ordinary grandchild dies with the wrapper.
pub struct GroupChild {
    child: Child,
    /// The group id, which is the leader's pid, kept separately: `Child::id`
    /// goes to `None` the moment the leader is reaped.
    pgid: u32,
}

impl GroupChild {
    /// Spawn `cmd` as a process-group leader. The caller sets the stdio it
    /// wants first; everything else about killing the child is handled here.
    pub fn spawn(mut cmd: Command) -> std::io::Result<Self> {
        in_own_group(&mut cmd);
        cmd.kill_on_drop(true);
        let child = cmd.spawn()?;
        let pgid = child.id().unwrap_or(0);
        Ok(Self { child, pgid })
    }

    /// The leader's pid while it is still running.
    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }

    pub fn stdout(&mut self) -> Option<tokio::process::ChildStdout> {
        self.child.stdout.take()
    }

    pub fn stderr(&mut self) -> Option<tokio::process::ChildStderr> {
        self.child.stderr.take()
    }

    /// Wait for the leader to exit on its own.
    pub async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.child.wait().await
    }

    /// Kill the whole group and reap the leader, waiting no longer than
    /// `REAP_BOUND` for it. A leader that cannot be reaped in that time is
    /// one nothing can kill (see `REAP_BOUND`); the caller must not hang on
    /// it, so this returns either way.
    pub async fn kill_and_reap(&mut self) {
        kill_group(self.pgid);
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(REAP_BOUND, self.child.wait()).await;
    }
}

impl Drop for GroupChild {
    fn drop(&mut self) {
        // Completion, timeout or a cancelled turn all land here. `killpg`
        // never blocks, not even on a child that cannot die.
        kill_group(self.pgid);
    }
}

/// Run `cmd` to completion or `timeout`. `Ok(None)` is a timeout. Either way
/// the command's whole process group has been signalled by the time this
/// returns, so an ordinary grandchild — the `find` behind a `find … | head`,
/// a `&` job, a watcher — dies with the wrapper rather than outliving it. The
/// one thing that survives is a process nothing can kill; see `REAP_BOUND`.
/// A server the model wants kept alive belongs in a background session.
/// What a bounded command produced: its full output, or, when the timeout
/// killed it, everything it had printed by then. Discarding that output
/// turned a five-minute wait loop into "killed, partial output discarded",
/// and the model re-ran the wait from scratch (2026-09-05).
pub enum Outcome {
    Done(std::process::Output),
    TimedOut { stdout: Vec<u8>, stderr: Vec<u8> },
}

pub async fn output_within(
    mut cmd: Command,
    timeout: std::time::Duration,
) -> std::io::Result<Outcome> {
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = GroupChild::spawn(cmd)?;
    let stdout_buf: Arc<Mutex<Vec<u8>>> = Arc::default();
    let stderr_buf: Arc<Mutex<Vec<u8>>> = Arc::default();
    let readers = [
        (child.stdout().map(|s| Box::pin(s) as std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>>), stdout_buf.clone()),
        (child.stderr().map(|s| Box::pin(s) as std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>>), stderr_buf.clone()),
    ]
    .into_iter()
    .filter_map(|(pipe, buf)| {
        let mut pipe = pipe?;
        Some(tokio::spawn(async move {
            let mut chunk = [0u8; 8192];
            while let Ok(n) = pipe.read(&mut chunk).await {
                if n == 0 {
                    break;
                }
                buf.lock().await.extend_from_slice(&chunk[..n]);
            }
        }))
    })
    .collect::<Vec<_>>();
    let status = tokio::time::timeout(timeout, child.wait()).await;
    if status.is_err() {
        child.kill_and_reap().await;
    }
    // Readers end at EOF once the process (group) is gone. One shared bound
    // across both, because a child the kill never reached — one in
    // uninterruptible sleep, one that escaped the group — holds the pipe open
    // and would otherwise hold up the answer.
    let drained_by = tokio::time::Instant::now() + REAP_BOUND;
    for r in readers {
        let _ = tokio::time::timeout_at(drained_by, r).await;
    }
    let stdout = std::mem::take(&mut *stdout_buf.lock().await);
    let stderr = std::mem::take(&mut *stderr_buf.lock().await);
    match status {
        Ok(status) => Ok(Outcome::Done(std::process::Output { status: status?, stdout, stderr })),
        Err(_) => Ok(Outcome::TimedOut { stdout, stderr }),
    }
}

/// The session that started a command, told when it ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    pub session_key: String,
    /// What the command does, in the words the owner read.
    pub description: String,
}

/// How a command starts. Every `run_command` process is one of these, on
/// the one registry: a foreground command is waited on by its call and moves
/// to the background at its timeout (Claude Code's Bash does the same,
/// `BashTool.tsx` onTimeout → startBackgrounding); a background command
/// returns its id at once. Either way its end reaches its caller, once it is
/// in the background.
#[derive(Debug, Clone)]
pub enum Spawn {
    Foreground,
    Background(Option<Caller>),
}

/// A command that finished in the background, for the session that started
/// it (Claude Code's task notification for a background shell,
/// `LocalShellTask.tsx` enqueueShellNotification).
#[derive(Debug, Clone)]
pub struct CommandExit {
    pub caller: Caller,
    pub task_id: String,
    pub command: String,
    /// `None`: ended by a signal.
    pub exit_code: Option<i32>,
    /// Everything it printed, stdout and stderr as they came.
    pub output: String,
}

/// How much of a finished command's output its notification carries; the
/// rest is one `read_output` away.
const EXIT_TAIL_CHARS: usize = 1200;

impl CommandExit {
    /// The notification body: what ended and how, the end of its output, and
    /// where the rest is.
    pub fn render(&self) -> String {
        let status = match self.exit_code {
            Some(0) => "completed (exit code 0)".to_string(),
            Some(code) => format!("failed with exit code {code}"),
            None => "was ended by a signal".to_string(),
        };
        let what = if self.caller.description.is_empty() {
            self.command.as_str()
        } else {
            self.caller.description.as_str()
        };
        let chars = self.output.chars().count();
        let tail: String = self.output.chars().skip(chars.saturating_sub(EXIT_TAIL_CHARS)).collect();
        let output = if self.output.trim().is_empty() {
            "It printed nothing.".to_string()
        } else if chars > EXIT_TAIL_CHARS {
            format!("The end of its output:\n{}", tail.trim_end())
        } else {
            format!("Its output:\n{}", tail.trim_end())
        };
        format!(
            "Background command {id} \"{what}\" {status}.\n{output}\nThe whole output: read_output(task_id: \"{id}\").",
            id = self.task_id
        )
    }
}

/// Where a finished background command is reported. Filled late by the
/// server, like the notify function: the registry exists before the wake
/// rail does.
pub type ExitSink = Arc<dyn Fn(CommandExit) + Send + Sync>;

/// Where a session stands, decided under one lock so a move to the
/// background and the process's end cannot cross.
#[derive(Debug, Default)]
struct Lifecycle {
    /// A call is waiting on it; it is nobody's background work yet.
    foreground: bool,
    /// Told when it ends. Cleared by a stop the caller asked for: nobody
    /// needs telling what they did themselves.
    notify: Option<Caller>,
    ended: bool,
}

/// A shell session in the registry.
#[derive(Debug)]
pub struct BackgroundSession {
    pub id: String,
    pub pid: u32,
    pub command: String,
    pub exited: bool,
    pub exit_code: Option<i32>,
    output: Arc<Mutex<String>>,
    pending_stdout: Arc<Mutex<Vec<u8>>>,
    pending_stderr: Arc<Mutex<Vec<u8>>>,
    lifecycle: Arc<std::sync::Mutex<Lifecycle>>,
    stdin_tx: Option<tokio::sync::mpsc::Sender<Vec<u8>>>,
}

impl BackgroundSession {
    pub async fn get_output(&self) -> String {
        self.output.lock().await.clone()
    }

    pub async fn drain_pending(&self) -> (Vec<u8>, Vec<u8>) {
        let stdout = std::mem::take(&mut *self.pending_stdout.lock().await);
        let stderr = std::mem::take(&mut *self.pending_stderr.lock().await);
        (stdout, stderr)
    }

    fn lifecycle(&self) -> std::sync::MutexGuard<'_, Lifecycle> {
        self.lifecycle.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A started command: its id, the session, and its exit status when it ends
/// (`None` when it could not be waited on or was stopped).
pub struct Started {
    pub session: Arc<BackgroundSession>,
    pub exited: tokio::sync::oneshot::Receiver<Option<std::process::ExitStatus>>,
}

type Sessions = Arc<Mutex<HashMap<String, Arc<BackgroundSession>>>>;

/// Every `run_command` process.
#[derive(Clone, Default)]
pub struct ProcessRegistry {
    running: Sessions,
    finished: Sessions,
    exit_sink: Arc<std::sync::RwLock<Option<ExitSink>>>,
}

impl ProcessRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Where finished background commands are reported.
    pub fn set_exit_sink(&self, sink: ExitSink) {
        *self.exit_sink.write().unwrap_or_else(|e| e.into_inner()) = Some(sink);
    }

    /// Start `cmd` (the shell running `command`, its env and cwd already
    /// set) as a session. A background start is refused past the session
    /// cap; a foreground one never is, since its call is waiting on it.
    pub async fn spawn(&self, mut cmd: Command, command: &str, spawn: Spawn) -> std::io::Result<Started> {
        let (foreground, notify) = match spawn {
            Spawn::Foreground => (true, None),
            Spawn::Background(caller) => {
                let running = self.running.lock().await;
                let background: Vec<String> = running
                    .values()
                    .filter(|s| !s.lifecycle().foreground)
                    .map(|s| format!("{} ({})", s.id, s.command))
                    .collect();
                if background.len() >= MAX_BACKGROUND_SESSIONS {
                    let mut ids = background;
                    ids.sort();
                    return Err(std::io::Error::other(format!(
                        "{MAX_BACKGROUND_SESSIONS} background sessions are already running; stop one first with stop_task: {}",
                        ids.join(", ")
                    )));
                }
                (false, caller)
            }
        };
        hide_window(&mut cmd);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        // A foreground command has nobody to type into it: input it waits
        // for would hold its call to the timeout. A background one takes
        // send_input.
        cmd.stdin(if foreground { Stdio::null() } else { Stdio::piped() });
        in_own_group(&mut cmd);
        let child = cmd.spawn()?;

        let pid = child.id().unwrap_or(0);
        // The shutdown handler kills registered children, so a Nebo restart
        // takes its sessions with it instead of orphaning them.
        napp::child_guard::register_child(pid);
        let session_id = format!("{SESSION_ID_PREFIX}{}", &Uuid::new_v4().to_string()[..8]);

        let (stdin_tx, stdin_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(32);
        let (exit_tx, exited) = tokio::sync::oneshot::channel();

        let session = Arc::new(BackgroundSession {
            id: session_id.clone(),
            pid,
            command: command.to_string(),
            exited: false,
            exit_code: None,
            output: Arc::default(),
            pending_stdout: Arc::default(),
            pending_stderr: Arc::default(),
            lifecycle: Arc::new(std::sync::Mutex::new(Lifecycle { foreground, notify, ended: false })),
            stdin_tx: Some(stdin_tx),
        });

        self.running.lock().await.insert(session_id, session.clone());

        let registry = self.clone();
        let s = session.clone();
        tokio::spawn(async move { registry.handle_process(child, s, stdin_rx, exit_tx).await });

        Ok(Started { session, exited })
    }

    /// Hand a foreground command, still running, to the background: it keeps
    /// running and `caller` is told when it ends. False when it already
    /// ended, in which case its call gets its result as usual.
    pub fn move_to_background(&self, session: &BackgroundSession, caller: Option<Caller>) -> bool {
        let mut life = session.lifecycle();
        if life.ended {
            return false;
        }
        life.foreground = false;
        life.notify = caller;
        true
    }

    /// Read the session's output until it ends, then settle it: a
    /// foreground command's status goes to its call, a background one is
    /// kept for read_output and its caller is told. A stop (`kill_session`)
    /// ends it the same way, with nobody told.
    async fn handle_process(
        self,
        mut child: Child,
        session: Arc<BackgroundSession>,
        mut stdin_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
        exit_tx: tokio::sync::oneshot::Sender<Option<std::process::ExitStatus>>,
    ) {
        let pid = session.pid;
        let session_id = session.id.clone();
        let reader = |pipe: Option<Box<dyn tokio::io::AsyncRead + Send + Unpin>>, pending: Arc<Mutex<Vec<u8>>>| {
            let output = session.output.clone();
            tokio::spawn(async move {
                let Some(mut pipe) = pipe else { return };
                let mut buf = [0u8; 4096];
                let mut carry: Vec<u8> = Vec::new();
                loop {
                    match pipe.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let data = &buf[..n];
                            carry.extend_from_slice(data);
                            output.lock().await.push_str(&drain_utf8(&mut carry));
                            pending.lock().await.extend_from_slice(data);
                        }
                    }
                }
            })
        };
        let stdout_handle = reader(
            child.stdout.take().map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
            session.pending_stdout.clone(),
        );
        let stderr_handle = reader(
            child.stderr.take().map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
            session.pending_stderr.clone(),
        );
        let mut child_stdin = child.stdin.take();
        let stdin_handle = tokio::spawn(async move {
            while let Some(bytes) = stdin_rx.recv().await {
                if let Some(ref mut stdin) = child_stdin {
                    let _ = stdin.write_all(&bytes).await;
                }
            }
        });

        let status = child.wait().await.ok();
        let exit_code = status.and_then(|s| s.code());
        debug!(session = %session_id, exit_code = ?exit_code, "command exited");
        // The leader is gone; whatever it left behind in the group goes too.
        kill_group(pid);
        napp::child_guard::unregister_child(pid);

        // The output drains once the group is gone, bounded: a child the kill
        // cannot reach holds the pipe open (see REAP_BOUND).
        let drained_by = tokio::time::Instant::now() + REAP_BOUND;
        let _ = tokio::time::timeout_at(drained_by, stdout_handle).await;
        let _ = tokio::time::timeout_at(drained_by, stderr_handle).await;
        stdin_handle.abort();

        let (foreground, notify) = {
            let mut life = session.lifecycle();
            life.ended = true;
            (life.foreground, life.notify.take())
        };
        // A stopped session is already out of `running`; a foreground
        // command's result goes to its call alone.
        let removed = self.running.lock().await.remove(&session_id);
        if let (Some(sess), false) = (removed, foreground) {
            let finished_sess = Arc::new(BackgroundSession {
                id: sess.id.clone(),
                pid: sess.pid,
                command: sess.command.clone(),
                exited: true,
                exit_code,
                output: sess.output.clone(),
                pending_stdout: sess.pending_stdout.clone(),
                pending_stderr: sess.pending_stderr.clone(),
                lifecycle: sess.lifecycle.clone(),
                stdin_tx: None,
            });
            self.finished.lock().await.insert(session_id.clone(), finished_sess);
        }
        let _ = exit_tx.send(status);
        let Some(caller) = notify else { return };
        let sink = self.exit_sink.read().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(sink) = sink {
            sink(CommandExit {
                caller,
                task_id: session_id,
                command: session.command.clone(),
                exit_code,
                output: session.get_output().await,
            });
        }
    }

    /// Get a session by ID (running or finished).
    pub async fn get_any_session(&self, id: &str) -> Option<Arc<BackgroundSession>> {
        if let Some(s) = self.running.lock().await.get(id) {
            return Some(s.clone());
        }
        self.finished.lock().await.get(id).cloned()
    }

    /// Running background sessions (a foreground command is its call's).
    pub async fn list_running(&self) -> Vec<Arc<BackgroundSession>> {
        self.running.lock().await.values().filter(|s| !s.lifecycle().foreground).cloned().collect()
    }

    /// The background commands session `session_key` started that are
    /// still running: (the command's session, what it does), oldest id first.
    pub async fn running_for(&self, session_key: &str) -> Vec<(Arc<BackgroundSession>, Caller)> {
        let mut out: Vec<(Arc<BackgroundSession>, Caller)> = self
            .running
            .lock()
            .await
            .values()
            .filter_map(|s| {
                let life = s.lifecycle();
                let caller = life.notify.clone().filter(|c| !life.foreground && !life.ended && c.session_key == session_key)?;
                Some((s.clone(), caller))
            })
            .collect();
        out.sort_by(|a, b| a.0.id.cmp(&b.0.id));
        out
    }

    /// List finished sessions.
    pub async fn list_finished(&self) -> Vec<Arc<BackgroundSession>> {
        self.finished.lock().await.values().cloned().collect()
    }

    /// Why `id` is not a running session: it already exited (with what code),
    /// or it never existed (and which sessions do).
    async fn not_running(&self, id: &str) -> String {
        if let Some(done) = self.finished.lock().await.get(id) {
            return match done.exit_code {
                Some(c) => format!("session {} has already exited (exit code {})", id, c),
                None => format!("session {} has already exited (terminated by signal)", id),
            };
        }
        let mut ids: Vec<String> = self.list_running().await.iter().map(|s| s.id.clone()).collect();
        ids.sort();
        if ids.is_empty() {
            format!("no session {}; no background sessions are running", id)
        } else {
            format!("no session {}; running: {}", id, ids.join(", "))
        }
    }

    /// Write data to a session's stdin.
    pub async fn write_stdin(&self, id: &str, data: &[u8]) -> Result<(), String> {
        let running = self.running.lock().await;
        let Some(sess) = running.get(id) else {
            drop(running);
            return Err(self.not_running(id).await);
        };
        let tx = sess.stdin_tx.as_ref().ok_or("session stdin closed")?;
        tx.send(data.to_vec())
            .await
            .map_err(|e| format!("write error: {}", e))
    }

    /// Stop a running session. Its caller is not told it ended: the caller
    /// asked for the stop (Claude Code's `notified` flag, set by TaskStop).
    pub async fn kill_session(&self, id: &str) -> Result<(), String> {
        let mut running = self.running.lock().await;
        let Some(sess) = running.remove(id) else {
            drop(running);
            return Err(self.not_running(id).await);
        };
        sess.lifecycle().notify = None;
        // The group dies by pid; the session's own task sees the end and
        // tidies up.
        kill_group(sess.pid);
        Ok(())
    }
}

/// Get the shell command and args for the current platform.
pub fn shell_command() -> (String, Vec<String>) {
    #[cfg(target_os = "windows")]
    {
        (
            "powershell.exe".to_string(),
            vec!["-NoProfile".to_string(), "-Command".to_string()],
        )
    }
    #[cfg(not(target_os = "windows"))]
    {
        ("bash".to_string(), vec!["-c".to_string()])
    }
}

/// Strip PowerShell error-record decoration from stderr, keeping only the
/// human-readable message lines.
///
/// PowerShell 5.1 wraps every error in a multi-line record:
///
/// ```text
/// ls : Cannot find path 'C:\x' because it does not exist.
/// At line:1 char:1
/// + ls /x
/// + ~~~~~
///     + CategoryInfo          : ObjectNotFound: (C:\x:String) [...]
///     + FullyQualifiedErrorId : PathNotFound,...
/// ```
///
/// Only the first line carries information the model can act on; the rest
/// is position markers and exception taxonomy that bloats the context to
/// ~5x what bash emits for the same failure. Tool responses must stay
/// within the response budget on every platform, so the decoration is
/// dropped here. bash/zsh stderr never matches these patterns — this is
/// only compiled on Windows.
#[cfg(target_os = "windows")]
pub fn clean_powershell_stderr(stderr: &str) -> String {
    fn is_decoration(line: &str) -> bool {
        let trimmed = line.trim_start();
        // "At line:1 char:1" position header
        if trimmed.starts_with("At line:") && trimmed.contains("char:") {
            return true;
        }
        // "+ <command echo>" and "+ ~~~~" squiggle markers
        if let Some(rest) = trimmed.strip_prefix("+ ") {
            return rest.chars().all(|c| c == '~' || c.is_whitespace())
                || trimmed.starts_with("+ CategoryInfo")
                || trimmed.starts_with("+ FullyQualifiedErrorId")
                || !rest.is_empty(); // command echo line
        }
        false
    }

    let cleaned: Vec<&str> = stderr
        .lines()
        .map(|l| l.trim_end())
        .filter(|l| !l.is_empty() && !is_decoration(l))
        .collect();

    if cleaned.is_empty() {
        // Never erase a real error entirely — fall back to the raw text.
        stderr.trim_end().to_string()
    } else {
        cleaned.join("\n")
    }
}

/// Configure a Command to not flash a console window on Windows.
///
/// On Windows, subprocess spawning creates a visible console window by default.
/// This sets the types::constants::CREATE_NO_WINDOW creation flag to suppress it.
/// No-op on non-Windows platforms.
#[cfg(target_os = "windows")]
pub fn hide_window(cmd: &mut tokio::process::Command) {
    cmd.creation_flags(types::constants::CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
pub fn hide_window(_cmd: &mut tokio::process::Command) {
    // No-op on Unix
}

/// Configure a std::process::Command to not flash a console window on Windows.
#[cfg(target_os = "windows")]
pub fn hide_window_std(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(types::constants::CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
pub fn hide_window_std(_cmd: &mut std::process::Command) {
    // No-op on Unix
}

/// Return a sanitized copy of the environment.
/// Delegates to `napp::plugin_runtime::sanitized_env` — the canonical implementation.
pub fn sanitized_env() -> Vec<(String, String)> {
    napp::plugin_runtime::sanitized_env()
}

#[cfg(all(test, target_os = "windows"))]
mod ps_stderr_tests {
    use super::clean_powershell_stderr;

    #[test]
    fn strips_error_record_decoration() {
        let raw = "ls : Cannot find path 'C:\\nonexistent\\path' because it does not exist.\r\nAt line:1 char:1\r\n+ ls /nonexistent/path\r\n+ ~~~~~~~~~~~~~~~~~~~~\r\n    + CategoryInfo          : ObjectNotFound: (C:\\nonexistent\\path:String) [Get-ChildItem], ItemNotFoundException\r\n    + FullyQualifiedErrorId : PathNotFound,Microsoft.PowerShell.Commands.GetChildItemCommand\r\n \r\n";
        let cleaned = clean_powershell_stderr(raw);
        assert_eq!(
            cleaned,
            "ls : Cannot find path 'C:\\nonexistent\\path' because it does not exist."
        );
        assert!(cleaned.len() < 200);
    }

    #[test]
    fn keeps_wrapped_message_lines() {
        let raw = "nonexistent_command : The term 'nonexistent_command' is not recognized as the name of a cmdlet, function, script file, \r\nor operable program. Check the spelling of the name, or if a path was included, verify that the path is correct and \r\ntry again.\r\nAt line:1 char:1\r\n+ nonexistent_command --flag\r\n+ ~~~~~~~~~~~~~~~~~~~\r\n    + CategoryInfo          : ObjectNotFound: (nonexistent_command:String) [], CommandNotFoundException\r\n    + FullyQualifiedErrorId : CommandNotFoundException\r\n \r\n";
        let cleaned = clean_powershell_stderr(raw);
        assert!(cleaned.contains("is not recognized"));
        assert!(cleaned.contains("try again."));
        assert!(!cleaned.contains("CategoryInfo"));
        assert!(!cleaned.contains("At line:"));
    }

    #[test]
    fn falls_back_to_raw_when_everything_filtered() {
        // Pathological input that is all decoration — never return empty.
        let raw = "At line:1 char:1\r\n+ foo\r\n";
        let cleaned = clean_powershell_stderr(raw);
        assert!(!cleaned.is_empty());
    }

    #[test]
    fn plain_stderr_unchanged() {
        let raw = "warning: something simple\n";
        assert_eq!(clean_powershell_stderr(raw), "warning: something simple");
    }
}

#[cfg(test)]
mod utf8_tests {
    use super::drain_utf8;

    #[test]
    fn a_character_split_across_reads_is_kept_not_dropped() {
        let text = "héllo wörld";
        let bytes = text.as_bytes();
        // Split inside the two-byte "é".
        let mut carry: Vec<u8> = Vec::new();
        carry.extend_from_slice(&bytes[..2]);
        let first = drain_utf8(&mut carry);
        assert_eq!(first, "h");
        assert_eq!(carry, vec![bytes[1]], "the half character waits for the next read");
        carry.extend_from_slice(&bytes[2..]);
        let rest = drain_utf8(&mut carry);
        assert_eq!(first + rest.as_str(), text);
        assert!(carry.is_empty());
    }

    #[test]
    fn an_invalid_byte_is_replaced_not_stuck() {
        let mut carry = vec![0xff, b'o', b'k'];
        let out = drain_utf8(&mut carry);
        assert!(out.ends_with("ok"), "{out}");
        assert!(carry.is_empty());
    }
}

/// A shell command that leaves behind a grandchild the group kill cannot
/// reach: `perl` puts itself in its own process group before exec'ing
/// `sleep`, so `killpg` on the wrapper's group misses it, and it holds the
/// wrapper's output pipe open for as long as it lives. The `$!` written to
/// `pid_file` is that process, so a test can prove it survived. This is the
/// stand-in for the `D`-state child the tests are really about — a process in
/// uninterruptible sleep cannot be made on demand.
///
/// Shell job control (`set -m`) cannot stand in for it. `dash` is `/bin/sh`
/// on Debian and Ubuntu, CI runners included, and it turns job control off
/// when there is no tty ("sh: set: can't access tty; job control turned
/// off"), leaving the background job in the wrapper's group, where the kill
/// reaches it. Only `bash` — `/bin/sh` on macOS — honours `set -m` without a
/// tty, so the escape was real on the Mac the tests were written on and a
/// no-op on Linux, where the grandchild died with the group: the test then
/// panicked on the hosted runner and passed on the house VM, because a
/// grandchild killed a moment ago is still a zombie and `kill(pid, 0)`
/// answers for one (2026-09-21).
#[cfg(all(test, unix))]
pub(crate) fn escaped_child_command(pid_file: &std::path::Path) -> String {
    format!(
        "perl -e 'setpgrp 0,0; exec q(sleep), q(30)' & echo $! > {}; wait",
        pid_file.display()
    )
}

/// Is that process still there? Signal 0 only checks existence.
#[cfg(all(test, unix))]
pub(crate) fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 checks existence and delivers nothing.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// The grandchild's pid, as the shell wrote it to `file`.
#[cfg(all(test, unix))]
pub(crate) async fn grandchild_pid(file: &std::path::Path) -> i32 {
    for _ in 0..50 {
        if let Ok(t) = std::fs::read_to_string(file)
            && let Ok(pid) = t.trim().parse()
        {
            return pid;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("grandchild never reported its pid (is `perl` installed?)");
}

#[cfg(all(test, unix))]
mod group_tests {
    use super::*;
    use std::time::Duration;

    /// Where a grandchild reports its pid, so the test can watch it live or die.
    fn pid_file() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("nebo-group-{}", Uuid::new_v4()))
    }
    async fn settle() {
        tokio::time::sleep(Duration::from_millis(150)).await;
    }

    #[tokio::test]
    async fn a_timed_out_command_takes_its_children_with_it() {
        let file = pid_file();
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(format!("sleep 30 & echo $! > {}; wait", file.display()));
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
        let out = output_within(cmd, Duration::from_millis(400)).await.unwrap();
        assert!(matches!(out, Outcome::TimedOut { .. }), "expected a timeout");
        let pid = grandchild_pid(&file).await;
        settle().await;
        assert!(!alive(pid), "the backgrounded grandchild outlived the timeout");
        let _ = std::fs::remove_file(file);
    }

    /// A child the kill cannot reach must not hold up the answer.
    ///
    /// The gate's shell door went silent for the harness's whole 180 s under a
    /// 120 s timeout (`run-command-retry-spiral`, run 3): the `find` grandchild
    /// was in uninterruptible sleep on a virtiofs mount, and the kill-and-wait
    /// waited on it. Nothing in this module may block on a process that will
    /// not exit — it signals the group, reaps with a bound, and answers
    /// anyway. The stand-in for that process is
    /// [`escaped_child_command`]: a grandchild that puts itself in its own
    /// process group, which the group kill does not reach and which keeps the
    /// output pipe open.
    #[tokio::test]
    async fn a_child_the_kill_cannot_reach_does_not_hold_up_the_answer() {
        let file = pid_file();
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(escaped_child_command(&file));

        let started = std::time::Instant::now();
        let out = output_within(cmd, Duration::from_millis(300)).await.unwrap();
        let waited = started.elapsed();

        assert!(matches!(out, Outcome::TimedOut { .. }), "expected a timeout");
        assert!(
            waited < Duration::from_secs(10),
            "the answer waited on a process that would not die ({waited:?})"
        );

        // The answer can only have come back after the drain gave up on the
        // pipe the escaped child still holds. Without that the test proves
        // nothing: a grandchild that died with the group closes the pipe, the
        // drain ends at once, and the answer arrives in the timeout alone.
        assert!(
            waited >= REAP_BOUND,
            "nothing held the output pipe open — the answer came back in {waited:?}, \
             inside the drain bound, so the stand-in never escaped the group"
        );
        let escaped = grandchild_pid(&file).await;
        assert!(
            alive(escaped),
            "the stand-in needs a grandchild the group kill misses; pid {escaped} died"
        );
        // SAFETY: a pid this test created, killed so the test leaves nothing behind.
        unsafe { libc::kill(escaped, libc::SIGKILL) };
        let _ = std::fs::remove_file(file);
    }

    #[tokio::test]
    async fn a_timed_out_command_keeps_what_it_printed() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("echo before; sleep 30; echo after");
        let out = output_within(cmd, Duration::from_millis(500)).await.unwrap();
        match out {
            Outcome::TimedOut { stdout, .. } => {
                assert_eq!(String::from_utf8_lossy(&stdout).trim(), "before");
            }
            Outcome::Done(_) => panic!("expected a timeout"),
        }
    }

    fn sh(command: &str) -> Command {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd
    }

    fn caller() -> Caller {
        Caller { session_key: "agent:a1:web".into(), description: "build the site".into() }
    }

    /// A registry whose finished background commands land on a channel.
    fn reported() -> (ProcessRegistry, tokio::sync::mpsc::UnboundedReceiver<CommandExit>) {
        let reg = ProcessRegistry::new();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        reg.set_exit_sink(Arc::new(move |exit| {
            let _ = tx.send(exit);
        }));
        (reg, rx)
    }

    #[tokio::test]
    async fn killing_a_background_session_takes_its_children_with_it() {
        let file = pid_file();
        let reg = ProcessRegistry::new();
        let started = reg
            .spawn(sh(&format!("sleep 30 & echo $! > {}; wait", file.display())), "sleep", Spawn::Background(None))
            .await
            .unwrap();
        let pid = grandchild_pid(&file).await;
        assert!(alive(pid));
        reg.kill_session(&started.session.id).await.unwrap();
        settle().await;
        assert!(!alive(pid), "the session's grandchild outlived the kill");
        let _ = std::fs::remove_file(file);
    }

    #[tokio::test]
    async fn background_sessions_are_capped_and_foreground_commands_are_not() {
        let reg = ProcessRegistry::new();
        let mut ids = Vec::new();
        for _ in 0..MAX_BACKGROUND_SESSIONS {
            ids.push(reg.spawn(sh("sleep 30"), "sleep 30", Spawn::Background(None)).await.unwrap().session.id.clone());
        }
        let err = reg.spawn(sh("sleep 30"), "sleep 30", Spawn::Background(None)).await.err().expect("capped");
        assert!(err.to_string().contains("stop one first"), "{err}");
        let fg = reg.spawn(sh("echo still-runs"), "echo", Spawn::Foreground).await.expect("a foreground command is never capped");
        assert!(fg.exited.await.unwrap().is_some_and(|s| s.success()));
        for id in ids {
            reg.kill_session(&id).await.unwrap();
        }
    }

    /// A background command's end reaches the session that started it
    /// (Claude Code's task notification for a background shell).
    #[tokio::test]
    async fn a_finished_background_command_tells_its_caller() {
        let (reg, mut rx) = reported();
        let started = reg
            .spawn(sh("echo built 12 pages; exit 3"), "make site", Spawn::Background(Some(caller())))
            .await
            .unwrap();
        let exit = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("told in time").expect("told");
        assert_eq!(exit.caller, caller());
        assert_eq!(exit.task_id, started.session.id);
        assert_eq!(exit.exit_code, Some(3));
        let body = exit.render();
        assert!(
            body.starts_with(&format!("Background command {} \"build the site\" failed with exit code 3.\nIts output:\nbuilt 12 pages", started.session.id)),
            "{body}"
        );
        assert!(body.ends_with(&format!("read_output(task_id: \"{}\").", started.session.id)), "{body}");
        let kept = reg.get_any_session(&started.session.id).await.expect("kept for read_output");
        assert!(kept.exited && kept.get_output().await.contains("built 12 pages"));
    }

    /// A stop the caller asked for is not reported back to it, and a
    /// foreground command belongs to its call alone: neither is told.
    #[tokio::test]
    async fn a_stopped_or_foreground_command_tells_nobody() {
        let (reg, mut rx) = reported();
        let started = reg.spawn(sh("sleep 30"), "sleep 30", Spawn::Background(Some(caller()))).await.unwrap();
        reg.kill_session(&started.session.id).await.unwrap();
        let fg = reg.spawn(sh("echo done"), "echo done", Spawn::Foreground).await.unwrap();
        assert!(fg.exited.await.unwrap().is_some());
        assert!(reg.get_any_session(&fg.session.id).await.is_none(), "a foreground result is its call's alone");
        settle().await;
        assert!(rx.try_recv().is_err(), "nobody is told");
    }

    /// A foreground command moved to the background at its timeout keeps
    /// running and is reported like any background command.
    #[tokio::test]
    async fn a_moved_command_is_reported_when_it_ends() {
        let (reg, mut rx) = reported();
        let fg = reg.spawn(sh("sleep 0.5; echo finally"), "slow", Spawn::Foreground).await.unwrap();
        assert!(reg.list_running().await.is_empty(), "a foreground command is not listed as background work");
        assert!(reg.move_to_background(&fg.session, Some(caller())));
        let exit = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("told in time").expect("told");
        assert_eq!((exit.exit_code, exit.output.trim()), (Some(0), "finally"));
        assert!(!reg.move_to_background(&fg.session, None), "an ended command cannot move");
    }

    #[test]
    fn the_report_names_a_signal_and_cuts_long_output_to_its_end() {
        let exit = CommandExit {
            caller: Caller { session_key: "s".into(), description: String::new() },
            task_id: "bg-1".into(),
            command: "npm run dev".into(),
            exit_code: None,
            output: format!("{}END", "x".repeat(5000)),
        };
        let body = exit.render();
        assert!(body.starts_with("Background command bg-1 \"npm run dev\" was ended by a signal.\nThe end of its output:\n"), "{body}");
        assert!(body.contains("xEND\n") && body.len() < 1500, "{body}");
    }
}
