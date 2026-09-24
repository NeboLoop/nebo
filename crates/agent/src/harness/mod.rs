//! The harness: the one loop every turn runs through — owner chat, helpers,
//! coworker runs, scheduled runs, voice and MCP runs, workflow activities.
//!
//! This tree replaces `Runner` (`runner.rs`), `steering.rs`, `turn_decide.rs`
//! and `goals.rs` at cutover; until then nothing on main calls it. The work
//! packages of the harness build plan fill it: Phase 1 moves code here from
//! `runner.rs` without changing behaviour, Phase 2 builds the new turn on
//! the moved parts. A function whose body is `unimplemented!("WPx.y")` is
//! filled by that package and has no caller before it.

pub mod after_turn;
pub mod compact;
pub mod conversation;
pub mod delegation;
pub mod events;
pub mod goal;
pub mod memory_context;
pub mod model_call;
pub mod permissions;
pub mod prompt;
pub mod recap;
pub mod reminders;
pub mod seat;
pub mod session_gate;
pub mod telemetry;
pub mod tool_round;
pub mod tool_surface;
pub mod turn;
pub mod turn_end;
pub mod usage;
pub mod workflow_turn;

use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::{RwLock, mpsc};
use tokio_util::sync::CancellationToken;

use crate::concurrency::ConcurrencyController;
use crate::runner::WorkflowMode;
use crate::selector::ModelSelector;
use crate::session::SessionManager;
use session_gate::{ActiveTurns, RunProgress};

/// The facade every caller starts a turn through: the services a turn runs
/// against. Cheap to clone; a running turn owns a clone. WP2.9 points the
/// callers here and deletes `Runner`.
#[derive(Clone)]
pub struct Harness {
    pub(crate) sessions: SessionManager,
    pub(crate) store: Arc<db::Store>,
    pub(crate) tools: Arc<tools::Registry>,
    pub(crate) providers: Arc<RwLock<Vec<Arc<dyn ai::Provider>>>>,
    pub(crate) selector: Arc<ModelSelector>,
    pub(crate) concurrency: Arc<ConcurrencyController>,
    pub(crate) hooks: Arc<napp::HookDispatcher>,
    /// Issues the credential a CLI provider's tool calls carry back over
    /// /agent/mcp.
    pub(crate) tool_credentials: Option<crate::tool_credentials::ToolCredentials>,
    pub(crate) agent_registry: tools::AgentRegistry,
    pub(crate) skill_loader: Option<Arc<tools::skills::Loader>>,
    pub(crate) ask_channels: Option<tools::AskChannels>,
    pub(crate) approval_channels: Option<tools::ApprovalChannels>,
    pub(crate) embedding_provider: Option<Arc<dyn ai::EmbeddingProvider>>,
    /// The hybrid search the memory tool uses: the turn's recall runs on it.
    pub(crate) hybrid_searcher: Option<Arc<dyn tools::HybridSearcher>>,
    pub(crate) title_sink: Option<Arc<dyn after_turn::ChatTitleSink>>,
    /// Owner-facing events outside a turn's stream (`turn_recap`).
    pub(crate) broadcast: Option<crate::agent_worker::NotifyFn>,
    pub(crate) active_turns: ActiveTurns,
}

impl Harness {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        store: Arc<db::Store>,
        tools: Arc<tools::Registry>,
        providers: Vec<Arc<dyn ai::Provider>>,
        selector: ModelSelector,
        concurrency: Arc<ConcurrencyController>,
        hooks: Arc<napp::HookDispatcher>,
        tool_credentials: Option<crate::tool_credentials::ToolCredentials>,
        agent_registry: tools::AgentRegistry,
        skill_loader: Option<Arc<tools::skills::Loader>>,
    ) -> Self {
        Self {
            sessions: SessionManager::new(store.clone()),
            store,
            tools,
            providers: Arc::new(RwLock::new(providers)),
            selector: Arc::new(selector),
            concurrency,
            hooks,
            tool_credentials,
            agent_registry,
            skill_loader,
            ask_channels: None,
            approval_channels: None,
            embedding_provider: None,
            hybrid_searcher: None,
            title_sink: None,
            broadcast: None,
            active_turns: Default::default(),
        }
    }

    /// Admit the turn and drive it on its own task; its events stream on the
    /// handle. On a busy session the input is queued into the running turn
    /// and the handle carries the busy line.
    pub async fn start_turn(&self, req: TurnRequest) -> Result<TurnHandle, HarnessError> {
        turn::start(self.clone(), req).await
    }

    /// Whether a turn is running on `key`.
    pub fn is_session_busy(&self, key: &str) -> bool {
        session_gate::session_is_busy(&self.active_turns, key)
    }

    /// The running turn's live counters on `key`, if one is running.
    pub fn active_turn_status(&self, key: &str) -> Option<types::api::ActiveTurnStatus> {
        session_gate::active_turn_status(&self.active_turns, key)
    }
}

/// Why a turn could not start.
#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("the turn could not start: {0}")]
    Failed(String),
}

/// A started turn: its event stream and its id.
pub struct TurnHandle {
    pub events: mpsc::Receiver<ai::StreamEvent>,
    pub turn_id: String,
}

/// Everything a caller says about the turn it wants.
pub struct TurnRequest {
    pub session_key: String,
    pub input: TurnInput,
    pub seat: SeatRequest,
    pub mode: TurnMode,
    pub delivery: Delivery,
    pub cancel: CancellationToken,
    pub progress: Option<RunProgress>,
}

/// What starts the turn.
pub enum TurnInput {
    /// The owner's message.
    Owner {
        text: String,
        images: Vec<ai::ImageContent>,
        attachments: Vec<comm::wire::Attachment>,
    },
    /// A prompt the platform writes and the owner never sees (christening,
    /// a voice task).
    Platform { text: String },
    /// A helper, coworker or workflow result woke an idle session.
    Notification(delegation::Completion),
    /// A workflow seed or a resume: the conversation already holds the input.
    None,
}

/// Which kind of turn this is.
pub enum TurnMode {
    /// An owner chat turn. Plan mode is the seat's approval mode.
    Chat,
    Helper {
        parent_session_key: String,
        kind: delegation::HelperKind,
        depth: u8,
    },
    /// A workflow activity. Boxed: the config is far larger than the other
    /// modes.
    Workflow(Box<WorkflowMode>),
    Fork(ForkKind),
}

/// A forked turn over a finished conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkKind {
    /// The self-improvement review.
    Review,
}

/// Where the turn's words go. WP2.9 adds the comm reply facts when it moves
/// the callers onto `start_turn`.
pub struct Delivery {
    pub channel: String,
    pub channel_ctx: Option<tools::ChannelContext>,
    /// Team roster, @mention and room briefing; rides as a `RunBriefing` fact.
    pub mention_briefing: Option<String>,
}

/// The seat a turn asks for; `seat::resolve_seat` turns it into a `Seat`.
#[derive(Clone)]
pub struct SeatRequest {
    pub agent_id: String,
    pub user_id: String,
    pub origin: tools::Origin,
    /// Which entry started this run.
    pub door: types::permissions::Door,
    /// A run override of the employee's mode (e.g. Plan); `None` = its own.
    pub mode: Option<types::permissions::Mode>,
    /// The parent's grant (a helper) or the creator's (a created employee):
    /// the run can only narrow it.
    pub ceiling: Option<types::permissions::Ceiling>,
    pub cwd: Option<String>,
    pub seed_taint: Vec<types::provenance::ProvenanceClass>,
    pub audience: Option<String>,
    pub tool_allowlist: Option<HashSet<String>>,
    pub tool_denial_hint: Option<String>,
    pub handoff_depth: u8,
    pub model_override: String,
    pub model_preference: Option<String>,
    pub personality_snippet: Option<String>,
    pub tool_scope: Option<String>,
}
