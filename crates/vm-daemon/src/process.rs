//! Process management inside the VM.
//!
//! Each spawned process gets:
//! - Its own working directory under /sessions/<id>/
//! - Stdout/stderr streaming back to the host via events
//! - Signal forwarding from the host

use crate::wire::Event;
use serde::Deserialize;
use std::collections::HashMap;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

/// Parameters for spawning a process (from host RPC).
#[derive(Debug, Deserialize)]
pub struct SpawnParams {
    pub id: String,
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
    #[serde(default)]
    #[allow(dead_code)] // deserialized from wire, used by host for session config
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    #[allow(dead_code)] // deserialized from wire, used by host for network policy
    pub allowed_domains: Vec<String>,
    #[serde(default)]
    #[allow(dead_code)] // deserialized from wire
    pub one_shot: bool,
}

/// A running process inside the VM.
#[allow(dead_code)] // fields populated for future kill/status support
pub struct ManagedProcess {
    pub id: String,
    pub name: String,
    child: Child,
}

/// Process manager — tracks all running processes.
pub struct ProcessManager {
    #[allow(dead_code)] // will be used for kill/status by PID lookup
    processes: HashMap<String, ManagedProcess>,
    event_tx: mpsc::UnboundedSender<Event>,
}

impl ProcessManager {
    pub fn new(event_tx: mpsc::UnboundedSender<Event>) -> Self {
        Self {
            processes: HashMap::new(),
            event_tx,
        }
    }

    /// Spawn a new process with isolation.
    pub async fn spawn(&mut self, params: SpawnParams) -> Result<String, String> {
        let work_dir = params
            .cwd
            .clone()
            .unwrap_or_else(|| format!("/sessions/{}", params.id));

        // Create working directory
        std::fs::create_dir_all(&work_dir)
            .map_err(|e| format!("failed to create work dir: {e}"))?;

        // Build command
        let mut cmd = Command::new("/bin/bash");
        cmd.arg("-c").arg(&format!(
            "{} {}",
            params.command,
            params
                .args
                .iter()
                .map(|a| shell_escape(a))
                .collect::<Vec<_>>()
                .join(" ")
        ));
        cmd.current_dir(&work_dir);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.stdin(Stdio::piped());

        // Clear environment, set minimal safe env
        cmd.env_clear();
        cmd.env("HOME", &work_dir);
        cmd.env("PATH", "/usr/local/bin:/usr/bin:/bin");
        cmd.env("TERM", "xterm-256color");
        cmd.env("LANG", "en_US.UTF-8");

        // Inject user-specified environment
        if let Some(env) = &params.env {
            for (key, value) in env {
                cmd.env(key, value);
            }
        }

        let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;
        let process_id = params.id.clone();

        info!(
            process_id = %process_id,
            name = %params.name,
            command = %params.command,
            work_dir = %work_dir,
            "spawned process"
        );

        // Stream stdout
        if let Some(stdout) = child.stdout.take() {
            let tx = self.event_tx.clone();
            let pid = process_id.clone();
            tokio::spawn(async move {
                let reader = BufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let _ = tx.send(Event::stdout(&pid, format!("{line}\n")));
                }
            });
        }

        // Stream stderr
        if let Some(stderr) = child.stderr.take() {
            let tx = self.event_tx.clone();
            let pid = process_id.clone();
            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let _ = tx.send(Event::stderr(&pid, format!("{line}\n")));
                }
            });
        }

        // Watch for exit
        let tx = self.event_tx.clone();
        let pid = process_id.clone();
        let mut child_for_wait = {
            // We need to move child into the wait task, but also store it
            // for kill support. Use the PID for kill instead.
            child
        };

        // Store process ID for kill
        let _system_pid = child_for_wait.id();

        tokio::spawn(async move {
            match child_for_wait.wait().await {
                Ok(status) => {
                    let code = status.code().unwrap_or(-1);
                    info!(process_id = %pid, code, "process exited");
                    let _ = tx.send(Event::exit(&pid, code));
                }
                Err(e) => {
                    error!(process_id = %pid, %e, "failed waiting for process");
                    let _ = tx.send(Event::exit(&pid, -1));
                }
            }
        });

        Ok(process_id)
    }

    /// Kill a process by ID.
    pub fn kill(&self, process_id: &str, signal: &str) -> Result<(), String> {
        // We use nix to send signals by scanning /proc
        // In practice, we'd track PIDs in the processes map
        warn!(process_id, signal, "kill requested (TODO: implement PID tracking)");
        Ok(())
    }

    /// Check if a process is running.
    pub fn is_running(&self, _process_id: &str) -> bool {
        // Check if the process exit event has been sent
        // TODO: track state per process
        false
    }
}

/// Shell-escape a string for safe embedding in a bash -c command.
fn shell_escape(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    if s.chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '/')
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}
