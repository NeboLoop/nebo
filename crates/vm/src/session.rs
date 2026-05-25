//! VM session management — each agent/skill execution gets an isolated session.
//!
//! A session represents a running execution context inside the VM with its own:
//! - Working directory
//! - Environment variables
//! - Network allowlist
//! - Process group

use crate::error::VmResult;
use crate::rpc::{GuestEvent, SpawnParams, VmClient};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, info};

/// Session state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    /// Session created, process not yet spawned.
    Created,
    /// Process is running inside the VM.
    Running,
    /// Process exited normally.
    Exited { code: i32 },
    /// Process was killed by a signal.
    Killed { signal: String },
    /// Session encountered an error.
    Failed { message: String },
}

/// An isolated execution session inside the VM.
#[derive(Debug)]
pub struct VmSession {
    /// Unique session identifier.
    pub id: String,
    /// Human-readable name (e.g., skill name).
    pub name: String,
    /// Current state.
    pub state: SessionState,
    /// Working directory inside the VM.
    pub work_dir: String,
    /// Allowed network domains for this session.
    pub allowed_domains: Vec<String>,
    /// Accumulated stdout.
    pub stdout: String,
    /// Accumulated stderr.
    pub stderr: String,
    /// Exit code (set when process exits).
    pub exit_code: Option<i32>,
    /// Process ID inside the VM (set after spawn).
    pub process_id: Option<String>,
}

impl VmSession {
    /// Create a new session with the given parameters.
    pub fn new(id: String, name: String, work_dir: String, allowed_domains: Vec<String>) -> Self {
        Self {
            id,
            name,
            state: SessionState::Created,
            work_dir,
            allowed_domains,
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            process_id: None,
        }
    }

    /// Spawn the session's process via the VM client.
    pub async fn spawn(
        &mut self,
        client: &VmClient,
        command: &str,
        args: Vec<String>,
        env: Option<HashMap<String, String>>,
        timeout_secs: Option<u64>,
    ) -> VmResult<()> {
        let params = SpawnParams {
            id: self.id.clone(),
            name: self.name.clone(),
            command: command.to_string(),
            args,
            cwd: Some(self.work_dir.clone()),
            env,
            timeout_secs,
            allowed_domains: self.allowed_domains.clone(),
            one_shot: true,
        };

        let result = client.spawn(params).await?;
        self.process_id = Some(result.process_id);
        self.state = SessionState::Running;
        debug!(session = %self.id, "process spawned in VM");
        Ok(())
    }

    /// Handle an event from the guest daemon for this session.
    pub fn handle_event(&mut self, event: &GuestEvent) {
        match event.event_type.as_str() {
            "stdout" => {
                if let Some(ref data) = event.data {
                    self.stdout.push_str(data);
                }
            }
            "stderr" => {
                if let Some(ref data) = event.data {
                    self.stderr.push_str(data);
                }
            }
            "exit" => {
                let code = event.exit_code.unwrap_or(-1);
                self.exit_code = Some(code);
                self.state = if let Some(ref signal) = event.signal {
                    SessionState::Killed {
                        signal: signal.clone(),
                    }
                } else {
                    SessionState::Exited { code }
                };
                info!(
                    session = %self.id,
                    code,
                    oom = event.oom_kill_count.unwrap_or(0),
                    "VM process exited"
                );
            }
            "error" => {
                let msg = event
                    .message
                    .clone()
                    .unwrap_or_else(|| "unknown error".to_string());
                self.state = SessionState::Failed { message: msg };
            }
            _ => {}
        }
    }

    /// Whether the session has finished (exited, killed, or failed).
    pub fn is_done(&self) -> bool {
        matches!(
            self.state,
            SessionState::Exited { .. }
                | SessionState::Killed { .. }
                | SessionState::Failed { .. }
        )
    }
}
