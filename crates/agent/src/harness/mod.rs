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

use std::collections::{HashMap, HashSet};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::runner::WorkflowMode;
use session_gate::{ActiveTurns, RunProgress};

/// The facade every caller starts a turn through. WP2.9 gives it the rest of
/// what `Runner` holds (sessions, tools, store, providers, concurrency,
/// selector, hooks, agent registry, ask/approval channels, embedding,
/// hybrid searcher, skill loader, title sink) when it points the callers here.
pub struct Harness {
    active_turns: ActiveTurns,
}

impl Harness {
    /// Admit, prepare and drive one turn; its events stream on the handle.
    pub async fn start_turn(&self, _req: TurnRequest) -> Result<TurnHandle, HarnessError> {
        unimplemented!("WP2.9")
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
pub struct SeatRequest {
    pub agent_id: String,
    pub user_id: String,
    pub origin: tools::Origin,
    pub permissions: Option<HashMap<String, bool>>,
    pub operation_policy: Option<tools::policy::OperationPolicy>,
    pub resource_grants: Option<HashMap<String, String>>,
    pub allowed_paths: Vec<String>,
    pub cwd: Option<String>,
    pub seed_taint: Vec<types::provenance::ProvenanceClass>,
    pub audience: Option<String>,
    pub tool_allowlist: Option<HashSet<String>>,
    pub tool_denial_hint: Option<String>,
    pub approval_mode: seat::ApprovalMode,
    pub handoff_depth: u8,
    pub model_override: String,
    pub model_preference: Option<String>,
    pub personality_snippet: Option<String>,
    pub tool_scope: Option<String>,
}
