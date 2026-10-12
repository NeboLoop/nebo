//! Persistent subprocess daemon for Windows desktop automation.
//!
//! Keeps a single PowerShell process alive across multiple operations, eliminating
//! the ~500-1000ms startup cost per command. Each script goes to stdin as ONE
//! line: `-Command -` runs a multi-line block only once a blank line follows
//! it, so a script sent as written waited out its timeout. The line runs the
//! script dot-sourced (its variables stay for the next one) with its error
//! stream merged into its output, and ends with a sentinel carrying whether
//! the script wrote an error.

use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

const SENTINEL: &str = "___NEBO_END___";

/// What running a script in the daemon came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ran {
    /// The script finished and wrote no error: its output.
    Ok(String),
    /// The script ran and failed (it wrote an error, threw, ended the
    /// session or timed out): what it printed, or why. It is never run again.
    Failed(String),
    /// The script never reached PowerShell: it is safe to run elsewhere.
    NotRun(String),
}

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
    pub async fn execute(&self, script: &str, timeout: Duration) -> Ran {
        let mut guard = self.inner.lock().await;

        // Ensure process is alive
        if guard.is_none() || !Self::is_alive(guard.as_mut().unwrap()) {
            match Self::spawn().await {
                Ok(proc) => *guard = Some(proc),
                Err(e) => return Ran::NotRun(e),
            }
        }

        // Write phase — if it fails, clear the process for restart
        {
            let proc = guard.as_mut().unwrap();
            if let Err(e) = proc.stdin.write_all(one_line(script).as_bytes()).await {
                *guard = None;
                return Ran::NotRun(format!("PowerShell stdin write failed: {}", e));
            }
            if let Err(e) = proc.stdin.flush().await {
                *guard = None;
                return Ran::NotRun(format!("PowerShell stdin flush failed: {}", e));
            }
        }

        // Read output lines until sentinel or timeout
        let mut output = String::new();
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            let mut line = String::new();
            let proc = guard.as_mut().unwrap();
            let read_result =
                tokio::time::timeout_at(deadline, proc.reader.read_line(&mut line)).await;

            match read_result {
                Ok(Ok(0)) => {
                    // EOF — the script ended the session (`exit`).
                    *guard = None;
                    return Ran::Failed(if output.trim().is_empty() {
                        "PowerShell exited before finishing; no output".to_string()
                    } else {
                        format!("PowerShell exited before finishing; partial output:\n{}", output.trim())
                    });
                }
                Ok(Ok(_)) => {
                    let trimmed = line.trim_end();
                    if let Some(status) = trimmed.strip_prefix(SENTINEL) {
                        let output = output.trim().to_string();
                        return if status == "0" { Ran::Ok(output) } else { Ran::Failed(output) };
                    }
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str(trimmed);
                }
                Ok(Err(e)) => {
                    *guard = None;
                    return Ran::Failed(format!("PowerShell read error: {}", e));
                }
                Err(_) => {
                    // Timeout — kill the stuck process
                    if let Some(mut proc) = guard.take() {
                        let _ = proc.child.kill().await;
                    }
                    return Ran::Failed(format!(
                        "PowerShell script timed out after {} ms; the process was killed",
                        timeout.as_millis()
                    ));
                }
            }
        }
    }

    fn is_alive(proc: &mut DaemonProcess) -> bool {
        // try_wait returns Ok(Some(status)) if exited, Ok(None) if still running
        matches!(proc.child.try_wait(), Ok(None))
    }

    async fn spawn() -> Result<DaemonProcess, String> {
        let mut child = command::new::<Command>("powershell", command::Console::Hidden)
            .args(["-NoProfile", "-NoLogo", "-Command", "-"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("Failed to start PowerShell: {}", e))?;

        let mut stdin = child.stdin.take().ok_or("No stdin")?;
        let stdout = child.stdout.take().ok_or("No stdout")?;
        // The session's output in UTF-8, once, before any script.
        stdin
            .write_all(command::POWERSHELL_UTF8.as_bytes())
            .await
            .map_err(|e| format!("PowerShell stdin write failed: {}", e))?;

        Ok(DaemonProcess {
            child,
            stdin,
            reader: BufReader::new(stdout),
        })
    }
}

/// `script` as the one stdin line the daemon runs: decoded from base64 (so
/// no quote or newline in it reaches the line), dot-sourced, its errors
/// merged into its output, then the sentinel with `1` when it wrote one.
fn one_line(script: &str) -> String {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(script.as_bytes());
    format!(
        "$__neboOk = $true; try {{ . ([ScriptBlock]::Create([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{encoded}')))) 2>&1 \
         | ForEach-Object {{ if ($_ -is [System.Management.Automation.ErrorRecord]) {{ $__neboOk = $false; \"$_\" }} else {{ $_ }} }} \
         | Out-String -Stream -Width 4096 }} catch {{ $__neboOk = $false; \"$_\" }}; \
         Write-Output ('{SENTINEL}' + [int](-not $__neboOk))\n"
    )
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

    #[test]
    fn a_script_is_sent_as_one_line() {
        let line = one_line("if ($true) {\n  'it''s \"here\"'\n}");
        assert_eq!(line.matches('\n').count(), 1);
        assert!(line.ends_with('\n'));
    }

    /// The session runs multi-line scripts, keeps variables between them,
    /// and tells a script that wrote an error from one that did not.
    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn the_session_runs_scripts_as_written() {
        let daemon = DesktopDaemon::new();
        let t = Duration::from_secs(20);
        assert_eq!(daemon.execute("$n = 41\nif ($true) {\n  'block ran'\n}", t).await, Ran::Ok("block ran".into()));
        assert_eq!(daemon.execute("$n + 1", t).await, Ran::Ok("42".into()));
        assert_eq!(daemon.execute("'before'; Write-Error 'boom'", t).await, Ran::Failed("before\nboom".into()));
        assert_eq!(
            daemon.execute("Get-Process -Name no-such-xyz -ErrorAction SilentlyContinue; 'quiet'", t).await,
            Ran::Ok("quiet".into())
        );
        assert!(matches!(daemon.execute("exit 1", t).await, Ran::Failed(_)));
        assert_eq!(daemon.execute("'back'", t).await, Ran::Ok("back".into()), "a new session after exit");
        assert_eq!(
            daemon.execute("'Microsoft' + [char]0xAE + ' Edge ' + [char]0x2014 + ' caf' + [char]0xE9", t).await,
            Ran::Ok("Microsoft® Edge — café".into()),
            "output is UTF-8"
        );
    }
}
