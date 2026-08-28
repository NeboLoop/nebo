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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::GuestEvent;

    fn event(event_type: &str) -> GuestEvent {
        GuestEvent {
            event_type: event_type.to_string(),
            id: "s1".to_string(),
            data: None,
            exit_code: None,
            signal: None,
            oom_kill_count: None,
            message: None,
            fatal: None,
            status: None,
            step: None,
        }
    }

    fn session() -> VmSession {
        VmSession::new(
            "s1".to_string(),
            "test-skill".to_string(),
            "/sessions/s1".to_string(),
            vec!["pypi.org".to_string()],
        )
    }

    /// INVARIANT: a new session starts in Created with empty output buffers,
    /// no exit code, no process id, and is not yet done.
    #[test]
    fn new_session_starts_created() {
        let s = session();
        assert_eq!(s.state, SessionState::Created);
        assert!(s.stdout.is_empty());
        assert!(s.stderr.is_empty());
        assert_eq!(s.exit_code, None);
        assert_eq!(s.process_id, None);
        assert!(!s.is_done());
    }

    /// INVARIANT: stdout and stderr events append to their own buffers in
    /// arrival order without cross-contamination.
    #[test]
    fn output_events_accumulate_per_stream() {
        let mut s = session();
        let mut out1 = event("stdout");
        out1.data = Some("hello ".to_string());
        let mut err1 = event("stderr");
        err1.data = Some("warn\n".to_string());
        let mut out2 = event("stdout");
        out2.data = Some("world".to_string());

        s.handle_event(&out1);
        s.handle_event(&err1);
        s.handle_event(&out2);

        assert_eq!(s.stdout, "hello world");
        assert_eq!(s.stderr, "warn\n");
        assert!(!s.is_done());
    }

    /// INVARIANT: an exit event without a signal transitions to Exited with the
    /// reported code, records exit_code, and marks the session done.
    #[test]
    fn exit_event_transitions_to_exited() {
        let mut s = session();
        let mut ev = event("exit");
        ev.exit_code = Some(3);
        s.handle_event(&ev);
        assert_eq!(s.state, SessionState::Exited { code: 3 });
        assert_eq!(s.exit_code, Some(3));
        assert!(s.is_done());
    }

    /// INVARIANT: an exit event carrying a signal transitions to Killed (not
    /// Exited), preserving the signal name.
    #[test]
    fn exit_with_signal_transitions_to_killed() {
        let mut s = session();
        let mut ev = event("exit");
        ev.exit_code = Some(137);
        ev.signal = Some("KILL".to_string());
        s.handle_event(&ev);
        assert_eq!(
            s.state,
            SessionState::Killed {
                signal: "KILL".to_string()
            }
        );
        assert!(s.is_done());
    }

    /// INVARIANT: an exit event with no exit_code records -1 rather than
    /// pretending success.
    #[test]
    fn exit_without_code_records_minus_one() {
        let mut s = session();
        s.handle_event(&event("exit"));
        assert_eq!(s.exit_code, Some(-1));
        assert_eq!(s.state, SessionState::Exited { code: -1 });
    }

    /// INVARIANT: an error event transitions to Failed, defaulting the message
    /// to "unknown error" when the guest sent none.
    #[test]
    fn error_event_transitions_to_failed() {
        let mut s = session();
        let mut ev = event("error");
        ev.message = Some("boom".to_string());
        s.handle_event(&ev);
        assert_eq!(
            s.state,
            SessionState::Failed {
                message: "boom".to_string()
            }
        );
        assert!(s.is_done());

        let mut s2 = session();
        s2.handle_event(&event("error"));
        assert_eq!(
            s2.state,
            SessionState::Failed {
                message: "unknown error".to_string()
            }
        );
    }

    /// INVARIANT: unrecognized event types leave the session state untouched.
    #[test]
    fn unknown_event_is_ignored() {
        let mut s = session();
        s.handle_event(&event("networkStatus"));
        assert_eq!(s.state, SessionState::Created);
        assert!(s.stdout.is_empty());
        assert!(!s.is_done());
    }
}
