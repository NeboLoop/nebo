use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::debug;
use uuid::Uuid;

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

/// Owns a foreground command's process group: dropping it, on completion,
/// timeout or a cancelled turn, kills the group.
struct GroupGuard(u32);
impl Drop for GroupGuard {
    fn drop(&mut self) {
        kill_group(self.0);
    }
}

/// Run `cmd` to completion or `timeout`. `Ok(None)` is a timeout. Either way
/// the command's whole process group is gone when this returns; a server the
/// model wants kept alive belongs in a background session.
pub async fn output_within(
    mut cmd: Command,
    timeout: std::time::Duration,
) -> std::io::Result<Option<std::process::Output>> {
    in_own_group(&mut cmd);
    cmd.kill_on_drop(true);
    let child = cmd.spawn()?;
    let _group = GroupGuard(child.id().unwrap_or(0));
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(out) => out.map(Some),
        Err(_) => Ok(None),
    }
}

/// A background shell session.
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
    stdin_tx: Option<tokio::sync::mpsc::Sender<Vec<u8>>>,
    kill_tx: Option<tokio::sync::oneshot::Sender<()>>,
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
}

/// Manages background shell processes.
pub struct ProcessRegistry {
    running: Arc<Mutex<HashMap<String, Arc<BackgroundSession>>>>,
    finished: Arc<Mutex<HashMap<String, Arc<BackgroundSession>>>>,
}

impl ProcessRegistry {
    pub fn new() -> Self {
        Self {
            running: Arc::new(Mutex::new(HashMap::new())),
            finished: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Spawn a background process and return its session.
    pub async fn spawn_background(
        &self,
        command: &str,
        cwd: Option<&str>,
        extra_env: &[(String, String)],
    ) -> Result<String, String> {
        let (shell, shell_args) = shell_command();
        let mut cmd = Command::new(shell);
        for arg in &shell_args {
            cmd.arg(arg);
        }
        cmd.arg(command);

        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }

        hide_window(&mut cmd);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.stdin(Stdio::piped());
        cmd.env_clear();
        for (k, v) in sanitized_env() {
            cmd.env(k, v);
        }
        for (k, v) in extra_env {
            cmd.env(k, v);
        }

        {
            let running = self.running.lock().await;
            if running.len() >= MAX_BACKGROUND_SESSIONS {
                let mut ids: Vec<String> = running.values().map(|s| format!("{} ({})", s.id, s.command)).collect();
                ids.sort();
                return Err(format!(
                    "{MAX_BACKGROUND_SESSIONS} background sessions are already running; kill one first: {}",
                    ids.join(", ")
                ));
            }
        }
        in_own_group(&mut cmd);
        let child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn '{}': {}", command, e))?;

        let pid = child.id().unwrap_or(0);
        // The shutdown handler kills registered children, so a Nebo restart
        // takes its background sessions with it instead of orphaning them.
        napp::child_guard::register_child(pid);
        let session_id = format!("bg-{}", &Uuid::new_v4().to_string()[..8]);

        let output = Arc::new(Mutex::new(String::new()));
        let pending_stdout = Arc::new(Mutex::new(Vec::new()));
        let pending_stderr = Arc::new(Mutex::new(Vec::new()));

        let (stdin_tx, stdin_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(32);
        let (kill_tx, kill_rx) = tokio::sync::oneshot::channel();

        let session = Arc::new(BackgroundSession {
            id: session_id.clone(),
            pid,
            command: command.to_string(),
            exited: false,
            exit_code: None,
            output: output.clone(),
            pending_stdout: pending_stdout.clone(),
            pending_stderr: pending_stderr.clone(),
            stdin_tx: Some(stdin_tx),
            kill_tx: Some(kill_tx),
        });

        self.running
            .lock()
            .await
            .insert(session_id.clone(), session.clone());

        // Spawn IO handler
        let running = self.running.clone();
        let finished = self.finished.clone();
        let sid = session_id.clone();

        tokio::spawn(async move {
            Self::handle_process(
                child,
                sid,
                output,
                pending_stdout,
                pending_stderr,
                stdin_rx,
                kill_rx,
                running,
                finished,
            )
            .await;
        });

        Ok(session_id)
    }

    async fn handle_process(
        mut child: Child,
        session_id: String,
        output: Arc<Mutex<String>>,
        pending_stdout: Arc<Mutex<Vec<u8>>>,
        pending_stderr: Arc<Mutex<Vec<u8>>>,
        mut stdin_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
        kill_rx: tokio::sync::oneshot::Receiver<()>,
        running: Arc<Mutex<HashMap<String, Arc<BackgroundSession>>>>,
        finished: Arc<Mutex<HashMap<String, Arc<BackgroundSession>>>>,
    ) {
        let pid = child.id().unwrap_or(0);
        let mut child_stdout = child.stdout.take();
        let mut child_stderr = child.stderr.take();
        let mut child_stdin = child.stdin.take();

        // Read stdout in background
        let stdout_output = output.clone();
        let stdout_pending = pending_stdout.clone();
        let stdout_handle = tokio::spawn(async move {
            if let Some(ref mut stdout) = child_stdout {
                let mut buf = [0u8; 4096];
                let mut carry: Vec<u8> = Vec::new();
                loop {
                    match stdout.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => {
                            let data = &buf[..n];
                            carry.extend_from_slice(data);
                            stdout_output.lock().await.push_str(&drain_utf8(&mut carry));
                            stdout_pending.lock().await.extend_from_slice(data);
                        }
                        Err(_) => break,
                    }
                }
            }
        });

        // Read stderr in background
        let stderr_output = output.clone();
        let stderr_pending = pending_stderr.clone();
        let stderr_handle = tokio::spawn(async move {
            if let Some(ref mut stderr) = child_stderr {
                let mut buf = [0u8; 4096];
                let mut carry: Vec<u8> = Vec::new();
                loop {
                    match stderr.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => {
                            let data = &buf[..n];
                            carry.extend_from_slice(data);
                            stderr_output.lock().await.push_str(&drain_utf8(&mut carry));
                            stderr_pending.lock().await.extend_from_slice(data);
                        }
                        Err(_) => break,
                    }
                }
            }
        });

        // Handle stdin writes and kill signal
        let stdin_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    data = stdin_rx.recv() => {
                        match data {
                            Some(bytes) => {
                                if let Some(ref mut stdin) = child_stdin {
                                    let _ = stdin.write_all(&bytes).await;
                                }
                            }
                            None => break,
                        }
                    }
                }
            }
        });

        // Wait for process to exit or kill signal
        tokio::select! {
            status = child.wait() => {
                let exit_code = status.ok().and_then(|s| s.code());
                debug!(session = %session_id, exit_code = ?exit_code, "background process exited");
                // The leader is gone; whatever it left behind in the group goes too.
                kill_group(pid);
                napp::child_guard::unregister_child(pid);

                // Wait for IO to drain
                let _ = stdout_handle.await;
                let _ = stderr_handle.await;
                stdin_handle.abort();

                // Move from running to finished
                let mut running_lock = running.lock().await;
                if let Some(sess) = running_lock.remove(&session_id) {
                    let finished_sess = Arc::new(BackgroundSession {
                        id: sess.id.clone(),
                        pid: sess.pid,
                        command: sess.command.clone(),
                        exited: true,
                        exit_code,
                        output: sess.output.clone(),
                        pending_stdout: sess.pending_stdout.clone(),
                        pending_stderr: sess.pending_stderr.clone(),
                        stdin_tx: None,
                        kill_tx: None,
                    });
                    finished.lock().await.insert(session_id, finished_sess);
                }
            }
            _ = kill_rx => {
                kill_group(pid);
                let _ = child.kill().await;
                napp::child_guard::unregister_child(pid);
                debug!(session = %session_id, "background process killed");
            }
        }
    }

    /// Get a session by ID (running or finished).
    pub async fn get_any_session(&self, id: &str) -> Option<Arc<BackgroundSession>> {
        if let Some(s) = self.running.lock().await.get(id) {
            return Some(s.clone());
        }
        self.finished.lock().await.get(id).cloned()
    }

    /// List running sessions.
    pub async fn list_running(&self) -> Vec<Arc<BackgroundSession>> {
        self.running.lock().await.values().cloned().collect()
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
        let mut ids: Vec<String> = self.running.lock().await.keys().cloned().collect();
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

    /// Kill a running session by sending the kill signal via oneshot channel.
    pub async fn kill_session(&self, id: &str) -> Result<(), String> {
        let mut running = self.running.lock().await;
        let Some(sess) = running.remove(id) else {
            drop(running);
            return Err(self.not_running(id).await);
        };
        // The group dies here, by pid: the oneshot below only tidies the IO
        // task, and it cannot fire while another handle to the session exists.
        kill_group(sess.pid);

        // We need mutable access to take the kill_tx. Since the session is wrapped in Arc,
        // and we just removed the only reference from the map, we try to unwrap.
        // If that fails (other references exist), we still drop which closes channels.
        if let Ok(mut owned) = Arc::try_unwrap(sess) {
            if let Some(kill_tx) = owned.kill_tx.take() {
                let _ = kill_tx.send(());
            }
        }
        // If Arc::try_unwrap fails, the session IO handler will detect that
        // the stdin_tx was dropped and the process will be cleaned up.
        Ok(())
    }
}

impl Default for ProcessRegistry {
    fn default() -> Self {
        Self::new()
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

#[cfg(all(test, unix))]
mod group_tests {
    use super::*;
    use std::time::Duration;

    /// A grandchild's pid, written by the shell so the test can watch it die.
    fn pid_file() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("nebo-group-{}", Uuid::new_v4()))
    }
    fn alive(pid: i32) -> bool {
        // SAFETY: signal 0 only checks existence.
        unsafe { libc::kill(pid, 0) == 0 }
    }
    async fn grandchild_pid(file: &std::path::Path) -> i32 {
        for _ in 0..50 {
            if let Ok(t) = std::fs::read_to_string(file)
                && let Ok(pid) = t.trim().parse()
            {
                return pid;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("grandchild never reported its pid");
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
        assert!(out.is_none(), "expected a timeout");
        let pid = grandchild_pid(&file).await;
        settle().await;
        assert!(!alive(pid), "the backgrounded grandchild outlived the timeout");
        let _ = std::fs::remove_file(file);
    }

    #[tokio::test]
    async fn killing_a_background_session_takes_its_children_with_it() {
        let file = pid_file();
        let reg = ProcessRegistry::new();
        let id = reg
            .spawn_background(&format!("sleep 30 & echo $! > {}; wait", file.display()), None, &[])
            .await
            .unwrap();
        let pid = grandchild_pid(&file).await;
        assert!(alive(pid));
        reg.kill_session(&id).await.unwrap();
        settle().await;
        assert!(!alive(pid), "the session's grandchild outlived the kill");
        let _ = std::fs::remove_file(file);
    }

    #[tokio::test]
    async fn background_sessions_are_capped() {
        let reg = ProcessRegistry::new();
        let mut ids = Vec::new();
        for _ in 0..MAX_BACKGROUND_SESSIONS {
            ids.push(reg.spawn_background("sleep 30", None, &[]).await.unwrap());
        }
        let err = reg.spawn_background("sleep 30", None, &[]).await.unwrap_err();
        assert!(err.contains("kill one first"), "{err}");
        for id in ids {
            reg.kill_session(&id).await.unwrap();
        }
    }
}
