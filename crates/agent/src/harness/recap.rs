//! The owner recap: one or two plain sentences written after a chat turn for
//! the owner coming back to the thread. Stored and emitted, never read back
//! into a model request.

use super::turn::TurnContext;

/// Write the recap for the turn just finished; `None` when none was written.
pub async fn write_recap(_cx: &TurnContext) -> Option<String> {
    unimplemented!("WP2.5")
}
