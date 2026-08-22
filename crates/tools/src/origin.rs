/// Origin identifies the source of a request flowing through the agent.
/// Used by Policy to enforce per-origin tool restrictions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Origin {
    /// Direct user interaction (web UI, CLI).
    User,
    /// Inter-agent communication (NeboAI, loopback).
    Comm,
    /// External app binary.
    App,
    /// Matched skill template.
    Skill,
    /// Internal system tasks (heartbeat, cron, recovery).
    System,
    /// External MCP client (Claude Desktop, Cursor, etc.).
    Mcp,
    /// Automated workflow activity (engine-driven, unattended). Never HITL —
    /// the ask tool is unavailable here (see `BotTool::handle_ask`).
    Workflow,
    /// An untrusted phone caller. A stranger on a line the business answers:
    /// their speech is DATA, their authority is nothing. Deny-by-default —
    /// delegated runs carry an explicit tool allowlist (the line's call tree
    /// or the take-a-message floor) and can never reach approval modals.
    Caller,
}

impl Default for Origin {
    fn default() -> Self {
        Origin::User
    }
}

/// Communication personality for a run, derived from `Origin`.
///
/// Orthogonal to `PromptMode` (Full/Minimal): a subagent is Minimal+Autonomous,
/// a heartbeat may be Full+Autonomous. This selects the system-prompt "voice"
/// (preamble + milestone updates vs. silent execution + a structured final
/// report), NOT how much of the prompt is assembled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ExecutionMode {
    /// A human is watching the live stream (direct chat). Allow a brief preamble
    /// before the first tool call and short milestone updates while working.
    #[default]
    Interactive,
    /// Background run (cron / comm / heartbeat / subagent). Execute silently and
    /// deliver a single structured final report.
    Autonomous,
}

impl From<Origin> for ExecutionMode {
    fn from(origin: Origin) -> Self {
        // Exhaustive match (no `_`): a future Origin variant must be classified.
        match origin {
            Origin::User => ExecutionMode::Interactive,
            // Caller is deliberately Autonomous: a phone caller must never
            // reach the ask tool or raise an approval modal on the owner's
            // desktop — there is no human in the loop they're entitled to.
            Origin::Comm
            | Origin::App
            | Origin::Skill
            | Origin::System
            | Origin::Mcp
            | Origin::Workflow
            | Origin::Caller => ExecutionMode::Autonomous,
        }
    }
}

/// Type alias for the shared ask-channels map used by `ask_user()`.
pub type AskChannels = std::sync::Arc<
    tokio::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<String>>>,
>;

/// Shared tool-approval channels map (keyed by tool_call_id). The runner inserts
/// a oneshot sender and emits a `StreamEvent::approval_request`; the WS handler
/// resolves it from the user's ApprovalModal choice. The value is the decision:
/// `"once"`, `"always"`, or `"deny"` (mirrors the `AskChannels` string idiom and
/// carries the modal's "Approve Always" flag). This is the ONE tool-approval
/// pathway (PERMISSIONS_SME §11) — do not add a parallel one.
pub type ApprovalChannels = std::sync::Arc<
    tokio::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<String>>>,
>;

/// Sentinel value the frontend sends as the `ask_response` when the user dismisses
/// (Skip / Esc) an ask widget instead of answering. The ask tool interprets it as
/// "no answer — make a reasonable assumption", keeping the run from hanging.
pub const SKIP_SENTINEL: &str = "__skip__";

/// Channel context — set when the current run was triggered by an inbound
/// channel message (Slack, Discord, Teams, etc.). Lets channel plugins'
/// CLI subcommands (e.g. `slack upload`) target the right destination
/// without the agent having to look up channel/thread IDs.
///
/// Propagated as env vars `NEBO_CHANNEL_KIND`, `NEBO_CHANNEL_ID`,
/// `NEBO_THREAD_TS` when the plugin tool invokes a plugin binary.
/// See `docs/publishers-guide/channel-plugins.md`.
#[derive(Debug, Clone, Default)]
pub struct ChannelContext {
    /// Plugin slug / channel kind — "slack", "discord", "teams", etc.
    pub kind: String,
    /// Platform channel ID — "C1234567890" on Slack, "guild/channel" on Discord.
    pub channel_id: String,
    /// Thread ID for threading replies. Slack: `thread_ts`. Discord: parent message ID.
    pub thread_ts: Option<String>,
}

/// Context carried through tool execution for origin tracking and session info.
#[derive(Debug, Clone, Default)]
pub struct ToolContext {
    pub origin: Origin,
    pub session_key: String,
    pub session_id: String,
    pub user_id: String,
    /// Agent-to-agent handoff depth of the run this tool executes in (0 = not
    /// a handoff). The loop tool stamps it on outbound sends so receiving bots
    /// enforce the depth cap on tool-authored messages too.
    pub handoff_depth: u8,
    /// Per-entity permission overrides (tool category → allowed).
    pub entity_permissions: Option<std::collections::HashMap<String, bool>>,
    /// Per-employee three-state approval policy over gated interface operations
    /// (Always / Approval / Blocked). `None` = no per-employee overrides; the
    /// gate falls back to the seat's declared defaults. The single decision
    /// function both the chat gate and the workflow checkpoint consult.
    pub operation_policy: Option<crate::policy::OperationPolicy>,
    /// Per-entity resource grant overrides (resource → "allow"|"deny"|"inherit").
    pub resource_grants: Option<std::collections::HashMap<String, String>>,
    /// Dispatch-time tool whitelist for restricted internal runs (the
    /// self-improvement review fork). `Some` = only these tool names may
    /// execute; everything else is denied with a corrective error at the ONE
    /// dispatch choke point. `None` for every normal run. The full roster is
    /// still DECLARED to the model (prompt-cache byte parity) — restriction
    /// happens here, never by narrowing the registry.
    pub tool_whitelist: Option<std::collections::HashSet<String>>,
    /// Set when this run is the self-improvement review fork (or curator):
    /// the skill tool's writes target the LEARNED tree of this agent instead
    /// of user/skills/, and update/delete require the skill to have been
    /// loaded this run (read-before-write, see `skills_read`).
    pub learned_write_agent: Option<String>,
    /// With `learned_write_agent`: stage writes to pending_writes for Inbox
    /// approval instead of committing (employee's learning_mode = "staged").
    pub learned_write_staged: bool,
    /// Read marks for the review fork: learned-skill names loaded via the
    /// skill tool THIS run. update/delete on a learned skill is refused
    /// unless its name is here — the fork must write against actual on-disk
    /// content, never a recollection inferred from the transcript.
    pub skills_read: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
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
    /// Channel context (Slack/Discord/etc.) when this run was triggered by an
    /// inbound channel message. `None` for web UI, scheduled, or system runs.
    pub channel: Option<ChannelContext>,
    /// Resolved model of the run that invoked this tool ("provider/model").
    /// Sub-agent spawning inherits it when the caller gives no explicit
    /// model_override — without it sub-agents fall to the global default,
    /// which can be a different provider than the parent conversation.
    pub model_preference: Option<String>,
    /// Declared memory topic slugs for the active agent scope (agent.json
    /// `memory.topics`). The memory tool accepts these as `layer` values in
    /// addition to the built-in layers. Empty for the main bot.
    pub memory_topics: Vec<String>,
    /// Fail-closed memory isolation (set by the runner's ONE scope
    /// derivation): the active agent has `memory.context_isolated` but no
    /// isolation context could be derived for this run, so memory MUTATIONS
    /// (store/delete/clear) must be refused rather than land in the shared
    /// agent scope. Reads still serve the inherited chain. `false` for every
    /// normal run.
    pub memory_writes_disabled: bool,
    /// Capability categories the runner has cleared for execution this turn —
    /// pre-granted (capability ON), covered by Full Access, matched by the
    /// per-command allowlist, or user-approved via the ApprovalModal. In
    /// UNATTENDED runs an OFF capability is never inserted here — there is no
    /// autonomy bypass; the category stays ungranted and Phase 1b/1c
    /// hard-blocks (OFF means OFF when nobody can answer a prompt). The gate
    /// treats an OFF capability as allowed only when its category is present,
    /// so interactively "off" means ASK rather than a hard error. Empty for
    /// callers that don't run the approval gate (their OFF capabilities still
    /// hard-block, preserving enforcement).
    pub approved_categories: std::collections::HashSet<String>,
    /// Engine-stamped provenance snapshot of the run at this iteration — the
    /// classes of untrusted content the run has touched so far (accumulated by
    /// the runner from the static tool→class table). Read-only for tools; the
    /// coworker rail copies it onto outbound envelopes verbatim, and the
    /// memory tool checks it against `memory_write_bar`.
    pub run_taint: Vec<types::provenance::ProvenanceClass>,
    /// Effective memory write bar for this run's scope (agent config
    /// `memory.write_bar`, defaulted by the runner — `channel`+`phone` for
    /// context-isolated scopes). A memory MUTATION whose run taint intersects
    /// this set is refused at the store, never silently written.
    pub memory_write_bar: Vec<types::provenance::ProvenanceClass>,
    /// Recall-for-audience: this run is replying to a coworker NOT granted by
    /// the scope's `memory.share_with`. The memory tool serves `tacit/`
    /// (working style) only — non-tacit reads are refused with an
    /// "isn't shared with their role" correction. `false` for owner runs.
    pub audience_restricted: bool,
}

/// Canonical session key for an agent-bound workflow run: `agent:<id>:workflow:<run_id>`.
///
/// The ONE constructor for this key — tools that resolve per-agent state
/// (plugin account profiles, memory scope) parse the `agent:<id>:` prefix, so
/// every launch path must build it identically. Returns an empty string for
/// standalone (non-agent) runs, which resolve no agent state by design.
pub fn workflow_session_key(agent_id: &str, run_id: &str) -> String {
    if agent_id.is_empty() {
        String::new()
    } else {
        format!("agent:{}:workflow:{}", agent_id, run_id)
    }
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

    /// Whether the run's tool allowlist admits this call. `None` =
    /// unrestricted (every normal run). Entries are either a bare tool name
    /// ("skill" — the whole tool) or a `tool:resource` compound
    /// ("agent:memory") admitting only calls whose `resource` input matches —
    /// bare `os` in an allowlist would otherwise hand a restricted run the
    /// shell. Matching lives HERE so the runner's gate and the registry's
    /// choke point can never drift apart.
    pub fn whitelist_allows(&self, tool: &str, input: &serde_json::Value) -> bool {
        let Some(wl) = &self.tool_whitelist else {
            return true;
        };
        if wl.contains(tool) {
            return true;
        }
        if let Some(res) = input.get("resource").and_then(|v| v.as_str()) {
            if wl.contains(&format!("{tool}:{res}")) {
                return true;
            }
        }
        // Prefix entries ("mcp__monument__*") admit a tool family — how a
        // call-tree intent grants one MCP server's tools without naming each.
        wl.iter().any(|e| {
            e.strip_suffix('*')
                .is_some_and(|prefix| !prefix.is_empty() && tool.starts_with(prefix))
        })
    }

    /// Show a UI prompt to the user and block until they respond.
    ///
    /// Emits an `AskRequest` stream event (rendered as `AskWidget` in the frontend)
    /// and waits for the `ask_response` WebSocket message. Returns `None` if the
    /// stream or ask channels are not available (e.g. CLI mode). The frontend sends
    /// [`SKIP_SENTINEL`] as the value when the user dismisses the question, so the
    /// call always resolves and can never hang.
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
            .send(ai::StreamEvent::ask_request(
                &request_id,
                prompt,
                Some(widgets),
            ))
            .await;

        resp_rx.await.ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_mode_from_origin_table() {
        assert_eq!(ExecutionMode::from(Origin::User), ExecutionMode::Interactive);
        for o in [
            Origin::Comm,
            Origin::App,
            Origin::Skill,
            Origin::System,
            Origin::Mcp,
            Origin::Workflow,
            Origin::Caller,
        ] {
            assert_eq!(ExecutionMode::from(o), ExecutionMode::Autonomous);
        }
    }

    #[test]
    fn caller_origin_is_never_interactive() {
        // A phone caller must not reach ask/approval pathways: Interactive is
        // what unlocks them, and a stranger on the line is not the human in
        // the loop.
        assert_eq!(ExecutionMode::from(Origin::Caller), ExecutionMode::Autonomous);
    }

    #[test]
    fn workflow_origin_is_autonomous_not_hitl() {
        // Guards the ask-tool HITL gate: workflow activities must never read as
        // interactive, or the ask tool would block an unattended run.
        assert_eq!(
            ExecutionMode::from(Origin::Workflow),
            ExecutionMode::Autonomous
        );
    }

    #[test]
    fn execution_mode_default_matches_user_origin() {
        assert_eq!(
            ExecutionMode::default(),
            ExecutionMode::from(Origin::default())
        );
    }
}
