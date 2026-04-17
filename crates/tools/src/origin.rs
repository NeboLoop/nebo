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
    /// External MCP client (Claude Desktop, Cursor, etc.).
    Mcp,
}

impl Default for Origin {
    fn default() -> Self {
        Origin::User
    }
}

/// Type alias for the shared ask-channels map used by `ask_user()`.
pub type AskChannels = std::sync::Arc<
    tokio::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<String>>>,
>;

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
    /// Cancellation token from the parent run — propagated to sub-agents so that
    /// cancelling the parent also cancels any spawned children.
    pub cancel_token: tokio_util::sync::CancellationToken,
    /// Stream sender from the parent run — used by spawn_parallel to forward
    /// sub-agent progress events to the caller's event stream.
    pub stream_tx: Option<tokio::sync::mpsc::Sender<ai::StreamEvent>>,
    /// Run ID from the global RunRegistry — used by sub-agent spawning to link
    /// child runs to their parent via parent_run_id.
    pub run_id: Option<String>,
    /// Shared ask channels — tools call `ask_user()` to show a UI prompt and
    /// block until the user responds. The WS handler resolves the oneshot.
    pub ask_channels: Option<AskChannels>,
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

    /// Show a UI prompt to the user and block until they respond.
    ///
    /// Emits an `AskRequest` stream event (rendered as `AskWidget` in the frontend)
    /// and waits for the `ask_response` WebSocket message. Returns `None` if the
    /// stream or ask channels are not available (e.g. CLI mode).
    ///
    /// `widgets` should be a JSON array of widget definitions, e.g.:
    /// ```json
    /// [{"type": "checkbox", "label": "Pick calendars", "options": ["Work", "Personal"]}]
    /// ```
    pub async fn ask_user(&self, prompt: &str, widgets: serde_json::Value) -> Option<String> {
        let tx = self.stream_tx.as_ref()?;
        let channels = self.ask_channels.as_ref()?;

        let request_id = uuid::Uuid::new_v4().to_string();
        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();

        channels.lock().await.insert(request_id.clone(), resp_tx);

        let _ = tx
            .send(ai::StreamEvent::ask_request(&request_id, prompt, Some(widgets)))
            .await;

        resp_rx.await.ok()
    }
}
