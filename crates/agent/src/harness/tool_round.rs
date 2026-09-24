//! One round of tool calls: dispatch, gates, partition, run, caps, persist.
//! WP1.2 moves the body here from `runner.rs`. Items narrow to `pub(crate)`
//! once the turn driver calls them (WP2.3).

use super::events::TurnEvent;
use super::turn::{TurnContext, TurnState};

/// What a round produced.
pub struct RoundOutcome {
    pub events: Vec<TurnEvent>,
    /// A tool ended the turn with this notice. WP1.2 pairs it with the owner
    /// need once #237 (`types::OwnerNeed`) is on main.
    pub terminal: Option<String>,
    /// A workflow approval park or an exit primitive stopped the round.
    pub parked: bool,
    /// Deferred tools this round made callable.
    pub loaded_tools: Vec<String>,
}

/// Run the model's tool calls.
pub async fn run_tool_round(
    _cx: &TurnContext,
    _st: &mut TurnState,
    _calls: Vec<ai::ToolCall>,
) -> RoundOutcome {
    unimplemented!("WP1.2")
}
