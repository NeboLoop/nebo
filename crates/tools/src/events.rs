//! Event bus for workflow-to-workflow and system events.
//!
//! The EventBus provides best-effort event delivery via an unbounded mpsc channel.
//! Events are consumed by the EventDispatcher (in the workflow crate) which matches
//! them against role-owned event subscriptions and triggers workflows.

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
