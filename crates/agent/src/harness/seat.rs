//! The seat a turn runs in: its permission grant, taint, memory scope,
//! isolation and outside origins, resolved once per turn. WP1.3 moves the
//! resolution here from `runner.rs`; the grant is resolved by
//! `permissions::resolve_grant`.

use super::SeatRequest;

/// A resolved seat. WP1.3 adds the company-memory context when it moves the
/// resolution that builds it.
pub struct Seat {
    pub request: SeatRequest,
    pub memory: crate::memory::MemoryScope,
    pub execution_mode: tools::ExecutionMode,
    /// The run's permissions: mode, rules and ceiling.
    pub grant: std::sync::Arc<types::permissions::Grant>,
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
