//! The agreed goal: set by the owner (`/goal`) or proposed by the model and
//! approved; checked at turn end by a transcript-only done check whose
//! reason quotes the transcript.

use super::turn::TurnContext;

/// A session's agreed goal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgreedGoal {
    pub session_id: String,
    pub condition: String,
    pub source: GoalSource,
    pub status: GoalStatus,
    pub turns: u32,
    pub last_reason: Option<String>,
    /// Conditions the owner declined; never proposed again.
    pub declined: Vec<String>,
}

/// How the goal came to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalSource {
    OwnerCommand,
    ProposedApproved,
    OwnersOwnWords,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalStatus {
    Active,
    Met,
    Impossible,
    Paused(Pause),
    Cleared,
}

/// Why the goal stopped being pursued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pause {
    CheckUnavailable,
    UnmetTooOften,
    LimitReached,
    Stopped,
}

/// The done check's answer; `reason` quotes the transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalVerdict {
    pub met: bool,
    pub impossible: bool,
    pub reason: String,
}

/// Run the done check. `None` = the check was unavailable; the goal pauses.
pub async fn check_goal(_cx: &TurnContext, _goal: &AgreedGoal) -> Option<GoalVerdict> {
    unimplemented!("WP2.4")
}

/// The goal store over the `session_goals` table. WP2.4 adds the table and
/// its get / set / clear / record_check / record_decline.
pub struct GoalStore<'a>(pub &'a db::Store);
