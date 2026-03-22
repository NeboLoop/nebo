/// Origin identifies the source of a request flowing through the agent.
/// Used by Policy to enforce per-origin tool restrictions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Origin {
    /// Direct user interaction (web UI, CLI).
    User,
    /// Inter-agent communication (NeboLoop, loopback).
    Comm,
    /// External app binary.
    App,
    /// Matched skill template.
    Skill,
    /// Internal system tasks (heartbeat, cron, recovery).
    System,
}

impl Default for Origin {
    fn default() -> Self {
        Origin::User
    }
}

/// Context carried through tool execution for origin tracking and session info.
#[derive(Debug, Clone, Default)]
pub struct ToolContext {
    pub origin: Origin,
    pub session_key: String,
    pub session_id: String,
    pub user_id: String,
    /// Per-entity permission overrides (tool category → allowed).
    pub entity_permissions: Option<std::collections::HashMap<String, bool>>,
    /// Per-entity resource grant overrides (resource → "allow"|"deny"|"inherit").
    pub resource_grants: Option<std::collections::HashMap<String, String>>,
    /// Allowed filesystem paths — if set, file writes and shell commands are restricted
    /// to these directories and their children. Empty = unrestricted.
    pub allowed_paths: Vec<String>,
}

impl ToolContext {
    pub fn new(origin: Origin) -> Self {
        Self {
            origin,
            ..Default::default()
        }
    }

    pub fn with_session(mut self, key: impl Into<String>, id: impl Into<String>) -> Self {
        self.session_key = key.into();
        self.session_id = id.into();
        self
    }
}
