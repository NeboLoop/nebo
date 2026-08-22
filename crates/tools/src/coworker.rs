//! Coworker message rail — the seam between the `message` tool and the server's
//! dispatch pipeline (`server::coworker::CoworkerRailImpl`).
//!
//! Addressing determines mechanism: a NAMED employee is always reached by
//! message — delivered into their own lane, run under their own identity
//! (persona, memory scope, connected accounts, run receipt), visible as a
//! thread on both sides. There is no second way to reach a named agent;
//! anonymous parallel labor stays on `agent(resource: "task", action: "spawn")`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// One intra-bot coworker message (the local leg of the A2A envelope).
#[derive(Debug, Clone)]
pub struct CoworkerMessage {
    /// Sending agent id (empty = the main companion).
    pub from_agent_id: String,
    /// The sender's session key — where a fire-and-forget reply is appended
    /// when it arrives.
    pub sender_session_key: String,
    /// Target employee, by name or id. Resolved strictly by the rail against
    /// the installed roster — an unknown name is an error, never a minted
    /// identity.
    pub to: String,
    pub text: String,
    /// The requester's resolved memory scope, copied VERBATIM from
    /// `ToolContext.user_id` (the runner's canonical derivation). The rail
    /// extracts the matter (`:ctx:`) from it via `agent::memory` — carrying
    /// the matter on the envelope is what keys the target's thread and scope
    /// per-matter instead of pooling every matter into one default context
    /// (isolation audit 2026-08-22, leak #6). The tool never re-derives
    /// scopes.
    pub requester_scope: String,
    /// Agent-to-agent hop count (from `ToolContext.handoff_depth`). The rail
    /// enforces the chain cap so enlistment chains and A↔B cycles stay
    /// bounded.
    pub handoff_depth: u8,
    /// Wait for the coworker's reply (default). `false` = fire-and-forget:
    /// delivery is still acknowledged, and the reply is appended to the
    /// sender's session when it lands.
    pub wait: bool,
}

/// Delivery acknowledgment — a message is never silently dropped: either this
/// is returned (the message is persisted in the target's thread and their run
/// is enqueued) or the send errors.
#[derive(Debug, Clone)]
pub struct CoworkerDelivery {
    pub to_agent_id: String,
    pub to_name: String,
    /// The target-side thread (session key) the message was delivered into.
    pub thread_key: String,
    /// The coworker's reply (`wait: true` only).
    pub reply: Option<String>,
}

/// Implemented by the server (`CoworkerRailImpl`), consumed by the `message`
/// tool. `Pin<Box<dyn Future>>` for object safety — same seam shape as
/// `SubAgentOrchestrator`.
pub trait CoworkerRail: Send + Sync {
    fn send(
        &self,
        msg: CoworkerMessage,
    ) -> Pin<Box<dyn Future<Output = Result<CoworkerDelivery, String>> + Send + '_>>;
}

/// Late-bound cell: tool registration runs before `AppState` exists, so the
/// server fills this after startup — same pattern as `code_installer` and
/// `notify_fn`.
pub type CoworkerRailCell = Arc<std::sync::RwLock<Option<Arc<dyn CoworkerRail>>>>;

/// Create a new empty rail cell.
pub fn new_rail_cell() -> CoworkerRailCell {
    Arc::new(std::sync::RwLock::new(None))
}
