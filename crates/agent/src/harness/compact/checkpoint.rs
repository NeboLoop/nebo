//! A checkpoint: the conversation summarized into one boundary row that
//! keeps every owner message verbatim and quotes the next step. The
//! conversation loads from the latest boundary on.

use super::super::turn::TurnContext;

/// A written checkpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub boundary_id: String,
    pub summary: String,
    /// What `restore` re-attached after the boundary.
    pub restore: Vec<String>,
}

/// Why a checkpoint was taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointReason {
    Threshold,
    Overflow,
    OwnerAsked,
}

/// Checkpoint the turn's conversation.
pub async fn checkpoint(_cx: &TurnContext, _why: CheckpointReason) -> Result<Checkpoint, String> {
    unimplemented!("WP2.6")
}
