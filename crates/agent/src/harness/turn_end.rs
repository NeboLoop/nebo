//! The one end-of-turn hook. When the model answers without tool calls,
//! every registered check runs; one that says continue sends the loop into
//! another step with its reminder, one that says exit ends the turn.

use super::TurnMode;
use super::turn::{TurnContext, TurnExit, TurnState};

/// A check the turn must pass before it ends.
#[async_trait::async_trait]
pub trait EndCheck: Send + Sync {
    fn name(&self) -> &'static str;
    async fn check(&self, cx: &TurnContext, st: &TurnState) -> EndVerdict;
}

/// What an end check decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndVerdict {
    Stop,
    Continue { reminder: String },
    Exit(TurnExit),
}

/// The checks a turn of `mode` runs at its end. The base holds one, the
/// agreed-goal check for chat turns; WP2.4 registers it. Until then no mode
/// has a check and every turn ends when the model answers.
pub fn registry(_mode: &TurnMode) -> Vec<Box<dyn EndCheck>> {
    Vec::new()
}
