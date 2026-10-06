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
//!
//! What a call hands the owner (its files, its card) cannot ride the MCP
//! reply: the CLI passes on only its text. So `/agent/mcp` keeps it here
//! under the call's tool-use id, and the run takes it back when the CLI
//! streams that call's result, the way a runner-executed call carries it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tools::{ToolContext, ToolResult};


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

/// One live credential: the run it executes as, and what its calls handed
/// the owner, by tool-use id, until the run takes it.
struct Live {
    grant: Arc<RunGrant>,
    handed: HashMap<String, Handed>,
}

/// What one call hands the owner beyond its text: the parts of its
/// `ToolResult` a runner-executed call's event carries.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Handed {
    pub image_url: Option<String>,
    pub more_files: Vec<String>,
    pub payload: Option<serde_json::Value>,
}

impl Handed {
    /// The parts of `result` the owner is handed, or `None` when it hands
    /// nothing beyond its text.
    pub fn of(result: &ToolResult) -> Option<Self> {
        let handed = Self {
            image_url: result.image_url.clone(),
            more_files: result.more_files.clone(),
            payload: result.payload.clone(),
        };
        (handed != Self::default()).then_some(handed)
    }
}

/// The live credentials, shared by the runner (which issues them) and the
/// server's `/agent/mcp` (which honours them).
#[derive(Clone, Default)]
pub struct ToolCredentials(Arc<Mutex<HashMap<String, Live>>>);

impl ToolCredentials {
    /// Issue a credential for `grant`. It is valid until the returned guard
    /// is dropped.
    pub fn issue(&self, grant: RunGrant) -> CredentialGuard {
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        self.lock().insert(token.clone(), Live { grant: Arc::new(grant), handed: HashMap::new() });
        CredentialGuard {
            token,
            credentials: self.clone(),
        }
    }

    /// The run a credential was issued for, while it is live.
    pub fn grant(&self, token: &str) -> Option<Arc<RunGrant>> {
        self.lock().get(token).map(|live| live.grant.clone())
    }

    /// Keep what call `tool_use_id` of the run holding `token` handed the
    /// owner, for the run to take. A revoked credential keeps nothing.
    pub fn hand(&self, token: &str, tool_use_id: &str, handed: Handed) {
        if let Some(live) = self.lock().get_mut(token) {
            live.handed.insert(tool_use_id.to_string(), handed);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Live>> {
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

    /// What call `tool_use_id` handed the owner, once: taking it removes it.
    pub fn take_handed(&self, tool_use_id: &str) -> Option<Handed> {
        self.credentials.lock().get_mut(&self.token)?.handed.remove(tool_use_id)
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
    fn a_call_hands_its_files_to_its_own_run_once() {
        let creds = ToolCredentials::default();
        let run = creds.issue(grant());
        let other = creds.issue(grant());
        let result = ToolResult::ok("shared").with_image_url("/f/a.md").with_image_url("/f/b.md");
        let handed = Handed::of(&result).expect("a file is handed");
        creds.hand(run.token(), "toolu_1", handed.clone());
        assert!(other.take_handed("toolu_1").is_none(), "another run's call");
        assert_eq!(run.take_handed("toolu_1"), Some(handed));
        assert!(run.take_handed("toolu_1").is_none(), "taken once");
        assert!(Handed::of(&ToolResult::ok("text only")).is_none());
    }

    #[test]
    fn a_revoked_credential_keeps_nothing() {
        let creds = ToolCredentials::default();
        let token = creds.issue(grant()).token().to_string();
        creds.hand(&token, "toolu_1", Handed { image_url: Some("/f/a.md".into()), ..Default::default() });
        assert!(creds.lock().is_empty());
    }

    #[test]
    fn every_credential_is_its_own() {
        let creds = ToolCredentials::default();
        let a = creds.issue(grant());
        let b = creds.issue(grant());
        assert_ne!(a.token(), b.token());
    }
}
