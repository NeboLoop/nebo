//! Persistent subprocess daemon for Windows desktop automation.
//!
//! Keeps a single PowerShell process alive across multiple operations, eliminating
//! the ~500-1000ms startup cost per command. Scripts are written to stdin, output
//! is read until a sentinel delimiter.

use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

const SENTINEL: &str = "___NEBO_END___";

pub struct DesktopDaemon {
    inner: Mutex<Option<DaemonProcess>>,
}

struct DaemonProcess {
    child: Child,
    stdin: tokio::process::ChildStdin,
    reader: BufReader<tokio::process::ChildStdout>,
}

impl DesktopDaemon {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    /// Execute a PowerShell script via the persistent process.
    /// Auto-starts the process on first call and restarts on crash.
    pub async fn execute(&self, script: &str, timeout: Duration) -> Result<String, String> {
        let mut guard = self.inner.lock().await;

        // Ensure process is alive
        if guard.is_none() || !Self::is_alive(guard.as_mut().unwrap()) {
            *guard = Some(Self::spawn().await?);
        }

        // Write script + sentinel marker to stdin
        let payload = format!(
            "{}\nWrite-Output '{}'\n",
            script, SENTINEL
        );

        // Write phase — if it fails, clear the process for restart
        {
            let proc = guard.as_mut().unwrap();
            if let Err(e) = proc.stdin.write_all(payload.as_bytes()).await {
                *guard = None;
                return Err(format!("PowerShell stdin write failed: {}", e));
            }
            if let Err(e) = proc.stdin.flush().await {
                *guard = None;
                return Err(format!("PowerShell stdin flush failed: {}", e));
            }
        }

        // Read output lines until sentinel or timeout
        let mut output = String::new();
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            let mut line = String::new();
            let proc = guard.as_mut().unwrap();
            let read_result = tokio::time::timeout_at(
                deadline,
                proc.reader.read_line(&mut line),
            )
            .await;

            match read_result {
                Ok(Ok(0)) => {
                    // EOF — process exited
                    *guard = None;
                    return if output.is_empty() {
                        Err("PowerShell process exited unexpectedly".to_string())
                    } else {
                        Ok(output.trim().to_string())
                    };
                }
                Ok(Ok(_)) => {
                    let trimmed = line.trim_end();
                    if trimmed == SENTINEL {
                        break;
                    }
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str(trimmed);
                }
                Ok(Err(e)) => {
                    *guard = None;
                    return Err(format!("PowerShell read error: {}", e));
                }
                Err(_) => {
                    // Timeout — kill the stuck process
                    if let Some(mut proc) = guard.take() {
                        let _ = proc.child.kill().await;
                    }
                    return Err("PowerShell script timed out".to_string());
                }
            }
        }

        Ok(output.trim().to_string())
    }

    fn is_alive(proc: &mut DaemonProcess) -> bool {
        // try_wait returns Ok(Some(status)) if exited, Ok(None) if still running
        matches!(proc.child.try_wait(), Ok(None))
    }

    async fn spawn() -> Result<DaemonProcess, String> {
        let mut child = Command::new("powershell")
            .args(["-NoProfile", "-NoLogo", "-Command", "-"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("Failed to start PowerShell: {}", e))?;

        let stdin = child.stdin.take().ok_or("No stdin")?;
        let stdout = child.stdout.take().ok_or("No stdout")?;

        Ok(DaemonProcess {
            child,
            stdin,
            reader: BufReader::new(stdout),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_daemon_creation() {
        let daemon = DesktopDaemon::new();
        let guard = daemon.inner.lock().await;
        assert!(guard.is_none());
    }
}
