//! Event bus for workflow-to-workflow and system events.
//!
//! The EventBus provides best-effort event delivery via an unbounded mpsc channel.
//! Events are consumed by the EventDispatcher (in the workflow crate) which matches
//! them against agent-owned event subscriptions and triggers workflows.

use tracing::warn;

/// An event emitted by a workflow activity, system, or external source.
#[derive(Debug, Clone)]
pub struct Event {
    /// Source identifier, e.g. "email.customer-service" or "workflow.email-triage.completed".
    pub source: String,
    /// Arbitrary payload data.
    pub payload: serde_json::Value,
    /// Origin trace, e.g. "workflow:email-triage:run-550e".
    pub origin: String,
    /// Unix epoch seconds.
    pub timestamp: u64,
}

/// Cloneable event emitter backed by an unbounded mpsc channel.
#[derive(Clone)]
pub struct EventBus {
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
}

impl EventBus {
    /// Create a new EventBus and its receiving half.
    pub fn new() -> (Self, tokio::sync::mpsc::UnboundedReceiver<Event>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (Self { tx }, rx)
    }

    /// Emit an event (best-effort — dropped if receiver is gone).
    pub fn emit(&self, event: Event) {
        if let Err(e) = self.tx.send(event) {
            warn!(source = %e.0.source, "event bus: receiver dropped, event lost");
        }
    }
}

// ── event addressing ─────────────────────────────────────────────────────

/// Registered company event names. A seat that emits one of these emits it
/// UN-namespaced, so a subscriber to `assignment.done` hears it from any
/// producer; the payload's `producer` field says who. Every other emit name
/// is addressed by the producing seat (see `emit_source_for`).
pub const COMPANY_EVENTS: &[&str] = &[
    "assignment.done",
    "assignment.blocked",
    "assignment.failed",
    "layers_changed",
    "facts_changed",
    "pack_installed",
    "pack_updated",
    "pack_removed",
];

pub fn is_company_event(name: &str) -> bool {
    COMPANY_EVENTS.contains(&name)
}

/// The ONE place an emit name becomes an event source. Every pathway that
/// raises a seat's event — emit_event, the graph executor, the sequential
/// executor, cron, a webhook, the API, a manual run — builds its source here
/// and nowhere else.
///
/// `producer` is the emitting seat's name or slug (either is accepted; it is
/// normalized to a slug). `name` is the event as the seat declared it.
///
/// Three rules, in order:
/// 1. A registered company event keeps its bare name — that is the whole
///    point of registering it.
/// 2. An address that ALREADY names the producing seat is left exactly as
///    written. A seat's own event is addressed one way and one way only, so
///    the slug is never doubled: `operations.procurement-coordinator.po-issued`
///    emitted by `procurement-coordinator` stays as it is, because the address
///    already says who produced the fact.
/// 3. Anything else is prefixed with the producing seat, so the address names
///    who spoke.
pub fn emit_source_for(producer: &str, name: &str) -> String {
    let name = name.trim();
    if name.is_empty() || is_company_event(name) {
        return name.to_string();
    }
    let slug = db::agent_slug(producer);
    if slug.is_empty() || name.split('.').any(|segment| segment == slug) {
        return name.to_string();
    }
    format!("{}.{}", slug, name)
}

#[cfg(test)]
mod addressing_tests {
    use super::*;

    #[test]
    fn a_registered_company_event_stays_bare() {
        assert_eq!(emit_source_for("bookkeeper", "assignment.done"), "assignment.done");
        assert_eq!(emit_source_for("bookkeeper", "facts_changed"), "facts_changed");
    }

    #[test]
    fn a_seats_own_event_is_addressed_by_the_seat() {
        assert_eq!(
            emit_source_for("bookkeeper", "briefing.ready"),
            "bookkeeper.briefing.ready"
        );
        // A display name is accepted and normalized the same way a slug is.
        assert_eq!(
            emit_source_for("Chief Of Staff", "briefing.ready"),
            "chief-of-staff.briefing.ready"
        );
    }

    #[test]
    fn the_slug_is_never_doubled() {
        // The address packages write: department.role.event. The role segment
        // already names the producer, so nothing is prefixed.
        for producer in ["procurement-coordinator", "Procurement Coordinator"] {
            assert_eq!(
                emit_source_for(producer, "operations.procurement-coordinator.po-issued"),
                "operations.procurement-coordinator.po-issued"
            );
        }
        // And an already-namespaced runtime address passed through twice is
        // still the same address.
        let once = emit_source_for("bookkeeper", "briefing.ready");
        assert_eq!(emit_source_for("bookkeeper", &once), once);
    }
}
