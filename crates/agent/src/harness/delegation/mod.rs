//! Helpers: one delegation model on the one loop. A helper runs in the
//! background by default, returns only its final message, and reports once
//! through a notification. Status, list and stop are scoped to the caller's
//! own session.

pub mod child;
pub mod collect;
pub mod notify;

pub use notify::render_notification;

use super::turn::TurnContext;

/// What the model asks a helper to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperSpec {
    pub description: String,
    pub prompt: String,
    pub kind: HelperKind,
    pub background: bool,
    pub isolation: Option<Isolation>,
    pub model: Option<String>,
}

/// A helper's type. Explore and Plan are enforced tool sets: no writes, no
/// mutating shell, no helper tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperKind {
    General,
    Explore,
    Plan,
}

/// Where an isolated helper works: its own copy of the project
/// (`crate::worktree::Isolation` prepares it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Isolation {
    Worktree,
}

/// What a launch returned.
#[derive(Debug, Clone)]
pub enum Launch {
    Background { task_id: String },
    Finished(Completion),
}

/// A finished helper, coworker or workflow run.
#[derive(Debug, Clone)]
pub struct Completion {
    pub task_id: String,
    pub description: String,
    pub status: CompletionStatus,
    pub result: String,
    pub usage: ai::UsageInfo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionStatus {
    Done,
    Partial { why: String },
    Failed { error: String },
    Stopped,
}

/// Launch a helper.
pub async fn launch(_cx: &TurnContext, _spec: HelperSpec) -> Launch {
    unimplemented!("WP2.8")
}
