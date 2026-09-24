//! `drive_turn`: the turn state machine. Each step loads the conversation
//! since the last checkpoint, attaches the queued reminders, calls the
//! model and runs its tool calls; the turn ends when the model answers and
//! every end check lets it stop. WP2.3 builds it. Its items narrow to
//! `pub(crate)` once the facade calls them (WP2.9).

use std::collections::{BTreeSet, HashSet};

use super::seat::Seat;
use super::{TurnRequest, compact, goal, model_call, reminders, usage};

/// What one turn runs with.
pub struct TurnContext {
    pub request: TurnRequest,
    pub seat: Seat,
}

/// A turn's state across its steps.
pub struct TurnState {
    pub step: u32,
    pub transition: Transition,
    pub reminders: reminders::Reminders,
    /// Failover position, retry counters and output-cap escalation.
    pub call: model_call::CallState,
    /// Deferred tools loaded this session.
    pub loaded_tools: BTreeSet<String>,
    /// Memory ids this session was already shown; seeded at Prepare from
    /// `memory_context::surfaced_memories`.
    pub surfaced_memories: HashSet<i64>,
    pub end_checks_this_turn: u8,
    pub tokens: usage::TokenLedger,
    pub frozen_renderings: compact::trim::Frozen,
    pub read_ledger: crate::read_ledger::ReadLedger,
}

/// Why the loop is taking its next step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transition {
    First,
    AfterTools,
    MidTurnInput,
    CutoffResume { attempt: u8 },
    OutputEscalated,
    OverflowCheckpointed,
    TransientRetry { attempt: u8 },
    EndCheckContinue { check: &'static str, reason: String },
}

/// How a turn ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnExit {
    Answered,
    Cancelled,
    MaxSteps {
        steps: u32,
    },
    SpendCap,
    /// A tool ended the turn, with what only the owner can supply when the
    /// tool named it.
    TerminalTool {
        notice: String,
        need: Option<types::OwnerNeed>,
    },
    ProviderFailed(String),
    Refused(String),
    AwaitingApproval,
    PlanProposed,
    GoalMet {
        reason: String,
    },
    GoalImpossible {
        reason: String,
    },
    GoalPaused(goal::Pause),
}

/// Drive one turn to its exit.
pub async fn drive_turn(_cx: &TurnContext, _st: &mut TurnState) -> TurnExit {
    unimplemented!("WP2.3")
}
