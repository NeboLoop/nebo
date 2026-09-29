//! Per-run tool credentials.
//!
//! A CLI provider (the `claude` CLI as the model) runs its tool calls itself, over
//! the server's `/agent/mcp`. The server cannot tell such a call from any other
//! MCP client by where it comes from, so the run that spawns the CLI issues a
//! credential for that one provider call: random, held only in this process,
//! handed to the CLI in its MCP config, and revoked when the call ends. A tool
//! call carrying it executes as that run — the same context and grant as the
//! runner's own tool calls, through the same permission check. A call
//! without one is an outside MCP client.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tools::ToolContext;


/// The header a CLI provider's MCP config sends the credential in.
pub const HEADER: &str = "x-nebo-run-credential";

/// Everything a tool call needs to execute as the run that issued it.
#[derive(Clone)]
pub struct RunGrant {
    /// The context the run's own tool calls carry: origin, session, memory
    /// scope, the run's grant.
    pub ctx: ToolContext,
    pub agent_id: String,
}

/// The live credentials, shared by the runner (which issues them) and the
/// server's `/agent/mcp` (which honours them).
#[derive(Clone, Default)]
pub struct ToolCredentials(Arc<Mutex<HashMap<String, Arc<RunGrant>>>>);

impl ToolCredentials {
    /// Issue a credential for `grant`. It is valid until the returned guard
    /// is dropped.
    pub fn issue(&self, grant: RunGrant) -> CredentialGuard {
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        self.lock().insert(token.clone(), Arc::new(grant));
        CredentialGuard {
            token,
            credentials: self.clone(),
        }
    }

    /// The run a credential was issued for, while it is live.
    pub fn grant(&self, token: &str) -> Option<Arc<RunGrant>> {
        self.lock().get(token).cloned()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<RunGrant>>> {
        // A poisoned map still holds valid entries; the insert/remove that
        // panicked left it no worse than before.
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// Revokes its credential when dropped.
pub struct CredentialGuard {
    token: String,
    credentials: ToolCredentials,
}

impl CredentialGuard {
    pub fn token(&self) -> &str {
        &self.token
    }
}

impl Drop for CredentialGuard {
    fn drop(&mut self) {
        self.credentials.lock().remove(&self.token);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant() -> RunGrant {
        RunGrant {
            ctx: ToolContext::default(),
            agent_id: "a1".into(),
        }
    }

    #[test]
    fn a_credential_lives_as_long_as_its_guard() {
        let creds = ToolCredentials::default();
        let guard = creds.issue(grant());
        let token = guard.token().to_string();
        assert_eq!(token.len(), 64);
        assert_eq!(creds.grant(&token).map(|g| g.agent_id.clone()).as_deref(), Some("a1"));
        drop(guard);
        assert!(creds.grant(&token).is_none());
        assert!(creds.grant("").is_none());
    }

    #[test]
    fn every_credential_is_its_own() {
        let creds = ToolCredentials::default();
        let a = creds.issue(grant());
        let b = creds.issue(grant());
        assert_ne!(a.token(), b.token());
    }
}
