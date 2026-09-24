//! The seat a turn runs in: permissions, approval mode, grants, policy,
//! paths, taint, memory scope, isolation and outside origins, resolved once
//! per turn. WP1.3 moves the resolution here from `runner.rs`.

use super::SeatRequest;

/// A resolved seat. WP1.3 adds the company-memory context when it moves the
/// resolution that builds it.
pub struct Seat {
    pub request: SeatRequest,
    pub memory: crate::memory::MemoryScope,
    pub execution_mode: tools::ExecutionMode,
}

/// Resolve the seat `req` asks for on session `key`.
pub fn resolve_seat(
    _store: &db::Store,
    _reg: &tools::AgentRegistry,
    _key: &str,
    _req: SeatRequest,
) -> Seat {
    unimplemented!("WP1.3")
}

/// How a seat's tool calls are approved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalMode {
    /// Anything the seat's rules do not already allow asks the owner.
    Ask,
    /// File edits inside the seat's paths run without asking; the rest asks.
    AcceptEdits,
    /// Read-only until the owner approves the plan.
    Plan,
    /// Everything the seat's rules do not deny runs without asking.
    FullAccess,
    /// Anything that would ask is refused instead.
    NeverAsk,
    /// A classifier over the transcript decides what would ask.
    Automatic,
    /// A helper's or unattended run's asks go up to whoever can answer them
    /// (the parent turn, or the owner over comm).
    Relay,
}

/// What the approval check decides for one tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Allow,
    Deny,
    Ask,
}
