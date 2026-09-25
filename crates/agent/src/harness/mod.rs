//! The harness: the one loop every turn runs through — owner chat, helpers,
//! coworker runs, scheduled runs, voice and MCP runs, workflow activities.
//! Every caller starts a turn with [`Harness::start_turn`]; `turn::drive_turn`
//! is the loop.

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
use std::sync::{Arc, OnceLock};

use tokio::sync::{RwLock, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::concurrency::ConcurrencyController;
use crate::selector::ModelSelector;
use crate::session::SessionManager;
use session_gate::{ActiveTurns, RunProgress};
pub use workflow_turn::{WorkflowMode, WorkflowPark};

/// Where the harness tells the app what happens outside a turn's stream.
/// The server binds them once its state exists ([`Harness::bind`]); every
/// clone of the harness sees them.
#[derive(Default)]
pub struct Outlets {
    /// Chat titles, broadcast and pushed to the loop.
    pub title_sink: Option<Arc<dyn after_turn::ChatTitleSink>>,
    /// Owner-facing events outside a turn's stream (`turn_recap`).
    pub broadcast: Option<crate::agent_worker::NotifyFn>,
    /// Where the agreed goal's status, kickoffs and running work are told;
    /// without it no goal is checked.
    pub goal_observer: Option<Arc<dyn goal::GoalObserver>>,
}

/// The facade every caller starts a turn through: the services a turn runs
/// against. Cheap to clone; a running turn owns a clone.
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
    pub(crate) embedding_provider: Option<Arc<dyn ai::EmbeddingProvider>>,
    /// The hybrid search the memory tool uses: the turn's recall runs on it.
    pub(crate) hybrid_searcher: Option<Arc<dyn tools::HybridSearcher>>,
    pub(crate) outlets: Arc<OnceLock<Outlets>>,
    /// The goal check-ins waiting on background work, one per session.
    pub(crate) goal_check_ins: goal::CheckIns,
    pub(crate) active_turns: ActiveTurns,
    /// The owner's phone position, for the employees it is shared with.
    pub(crate) phone_locations: Arc<crate::phone_location::PhoneLocations>,
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
            embedding_provider: None,
            hybrid_searcher: None,
            outlets: Default::default(),
            goal_check_ins: Default::default(),
            active_turns: Default::default(),
            phone_locations: Default::default(),
        }
    }

    /// The owner's answers to a tool's question reach it here.
    pub fn with_ask_channels(mut self, channels: tools::AskChannels) -> Self {
        self.ask_channels = Some(channels);
        self
    }

    /// Embeds memories written after a turn and at a checkpoint.
    pub fn with_embedding_provider(mut self, provider: Arc<dyn ai::EmbeddingProvider>) -> Self {
        self.embedding_provider = Some(provider);
        self
    }

    /// The memory tool's own search: the turn's recall shares its index.
    pub fn with_hybrid_searcher(mut self, searcher: Arc<dyn tools::HybridSearcher>) -> Self {
        self.hybrid_searcher = Some(searcher);
        self
    }

    /// Bind the app's outlets. Once: a second bind is ignored.
    pub fn bind(&self, outlets: Outlets) {
        if self.outlets.set(outlets).is_err() {
            tracing::warn!("harness outlets bound twice; the first binding stays");
        }
    }

    pub(crate) fn title_sink(&self) -> Option<Arc<dyn after_turn::ChatTitleSink>> {
        self.outlets.get().and_then(|o| o.title_sink.clone())
    }

    pub(crate) fn broadcast(&self) -> Option<crate::agent_worker::NotifyFn> {
        self.outlets.get().and_then(|o| o.broadcast.clone())
    }

    pub(crate) fn goal_observer(&self) -> Option<Arc<dyn goal::GoalObserver>> {
        self.outlets.get().and_then(|o| o.goal_observer.clone())
    }

    pub fn sessions(&self) -> &SessionManager {
        &self.sessions
    }

    /// Where the owner's phone reports its position (`PUT /phone/location`).
    pub fn phone_locations(&self) -> &crate::phone_location::PhoneLocations {
        &self.phone_locations
    }

    pub fn store(&self) -> &Arc<db::Store> {
        &self.store
    }

    pub fn tools(&self) -> &Arc<tools::Registry> {
        &self.tools
    }

    pub fn selector(&self) -> &ModelSelector {
        &self.selector
    }

    pub fn concurrency(&self) -> &Arc<ConcurrencyController> {
        &self.concurrency
    }

    /// The providers every turn and side call shares.
    pub fn providers(&self) -> Arc<RwLock<Vec<Arc<dyn ai::Provider>>>> {
        self.providers.clone()
    }

    /// How many providers are loaded; 0 while they are being replaced.
    pub fn provider_count(&self) -> usize {
        self.providers.try_read().map(|p| p.len()).unwrap_or(0)
    }

    /// Replace the providers (the owner changed their keys or plan).
    pub async fn reload_providers(&self, providers: Vec<Arc<dyn ai::Provider>>) {
        let loaded: Vec<String> = providers.iter().map(|p| p.id().to_string()).collect();
        let count = providers.len();
        *self.providers.write().await = providers;
        self.selector.set_loaded_providers(loaded);
        self.selector.rebuild_fuzzy(&std::collections::HashMap::new());
        info!(count, "reloaded AI providers");
    }

    /// Title a chat whose turns were stored outside a turn (the voice loop
    /// writes its own rows): the same generator and sink a turn uses.
    pub fn spawn_title_generation(&self, session_id: &str, chat_id: &str) {
        after_turn::spawn_chat_title_generation(
            self.providers.clone(),
            self.store.clone(),
            chat_id.to_string(),
            session_id.to_string(),
            self.selector.get_cheapest_model(),
            self.title_sink(),
        );
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

    /// A turn running on `key` will still hear a row written now: it is
    /// running and its loop has not ended. A closing turn has made its last
    /// check for input, so a row written now needs a turn of its own.
    pub fn hears_new_rows(&self, key: &str) -> bool {
        session_gate::live_session_under(&self.active_turns, key)
            .is_some_and(|live| !session_gate::turn_is_closing(&self.active_turns, &live))
    }

    /// The running turn's live counters on `key`, if one is running.
    pub fn active_turn_status(&self, key: &str) -> Option<types::api::ActiveTurnStatus> {
        session_gate::active_turn_status(&self.active_turns, key)
    }

    /// The session a turn is live on under `key` (a workflow turn runs in an
    /// activity session under its run's key), if any.
    pub fn live_session_under(&self, key: &str) -> Option<String> {
        session_gate::live_session_under(&self.active_turns, key)
    }
}

/// FNV-1a over `data`: a cheap fingerprint for spotting a repeated result or
/// text. Not cryptographic.
pub(crate) fn simple_hash(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
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
    /// The self-improvement review. `staged`: learned skills wait in the
    /// Inbox for the owner instead of landing at once.
    Review { staged: bool },
}

/// Where the turn's words go.
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
