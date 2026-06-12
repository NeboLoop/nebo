use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::debug;
use uuid::Uuid;

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

        let child = cmd.spawn().map_err(|e| format!("failed to spawn: {}", e))?;

        let pid = child.id().unwrap_or(0);
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
        let mut child_stdout = child.stdout.take();
        let mut child_stderr = child.stderr.take();
        let mut child_stdin = child.stdin.take();

        // Read stdout in background
        let stdout_output = output.clone();
        let stdout_pending = pending_stdout.clone();
        let stdout_handle = tokio::spawn(async move {
            if let Some(ref mut stdout) = child_stdout {
                let mut buf = [0u8; 4096];
                loop {
                    match stdout.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => {
                            let data = &buf[..n];
                            if let Ok(text) = std::str::from_utf8(data) {
                                stdout_output.lock().await.push_str(text);
                            }
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
                loop {
                    match stderr.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => {
                            let data = &buf[..n];
                            if let Ok(text) = std::str::from_utf8(data) {
                                stderr_output.lock().await.push_str(text);
                            }
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
                let _ = child.kill().await;
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

    /// Write data to a session's stdin.
    pub async fn write_stdin(&self, id: &str, data: &[u8]) -> Result<(), String> {
        let running = self.running.lock().await;
        let sess = running
            .get(id)
            .ok_or_else(|| format!("session not found: {}", id))?;
        let tx = sess.stdin_tx.as_ref().ok_or("session stdin closed")?;
        tx.send(data.to_vec())
            .await
            .map_err(|e| format!("write error: {}", e))
    }

    /// Kill a running session by sending the kill signal via oneshot channel.
    pub async fn kill_session(&self, id: &str) -> Result<(), String> {
        let mut running = self.running.lock().await;
        let sess = running
            .remove(id)
            .ok_or_else(|| format!("session not found: {}", id))?;

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
/// This sets the CREATE_NO_WINDOW creation flag to suppress it.
/// No-op on non-Windows platforms.
#[cfg(target_os = "windows")]
pub fn hide_window(cmd: &mut tokio::process::Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
pub fn hide_window(_cmd: &mut tokio::process::Command) {
    // No-op on Unix
}

/// Configure a std::process::Command to not flash a console window on Windows.
#[cfg(target_os = "windows")]
pub fn hide_window_std(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    cmd.creation_flags(CREATE_NO_WINDOW);
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
