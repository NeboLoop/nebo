//! Event dispatcher — matches incoming events against agent-owned subscriptions
//! and triggers workflow runs.

use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{info, warn};

use tools::events::Event;
use tools::workflows::WorkflowManager;

/// An event subscription registered by an agent.
#[derive(Debug, Clone)]
pub struct EventSubscription {
    /// Pattern to match against event source, e.g. "email.urgent" or "email.*".
    pub pattern: String,
    /// Default inputs to pass to the workflow.
    pub default_inputs: serde_json::Value,
    /// Agent that owns this subscription.
    pub agent_source: String,
    /// Binding name within the agent.
    pub binding_name: String,
    /// Inline workflow definition JSON (from agent.json binding).
    pub definition_json: Option<String>,
    /// Namespaced emit source for the last activity (e.g. "chief-of-staff.briefing.ready").
    pub emit_source: Option<String>,
}

/// Dispatches events to matching workflow subscriptions.
pub struct EventDispatcher {
    subscriptions: Arc<RwLock<Vec<EventSubscription>>>,
}

impl EventDispatcher {
    pub fn new() -> Self {
        Self {
            subscriptions: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Replace all subscriptions.
    pub async fn set_subscriptions(&self, subs: Vec<EventSubscription>) {
        let mut lock = self.subscriptions.write().await;
        *lock = subs;
    }

    /// Add a single subscription.
    pub async fn subscribe(&self, sub: EventSubscription) {
        let mut lock = self.subscriptions.write().await;
        lock.push(sub);
    }

    /// Remove subscriptions for a single agent binding.
    pub async fn unsubscribe_binding(&self, agent_id: &str, binding_name: &str) {
        let mut lock = self.subscriptions.write().await;
        lock.retain(|sub| !(sub.agent_source == agent_id && sub.binding_name == binding_name));
    }

    /// Remove all subscriptions for an agent.
    pub async fn unsubscribe_agent(&self, agent_id: &str) {
        let mut lock = self.subscriptions.write().await;
        lock.retain(|sub| sub.agent_source != agent_id);
    }

    /// Clear all subscriptions.
    pub async fn clear(&self) {
        let mut lock = self.subscriptions.write().await;
        lock.clear();
    }

    /// Find subscriptions matching an event source.
    pub async fn match_event(&self, event: &Event) -> Vec<EventSubscription> {
        let lock = self.subscriptions.read().await;
        lock.iter()
            .filter(|sub| source_matches(&sub.pattern, &event.source))
            .cloned()
            .collect()
    }

    /// Spawn the dispatcher loop. Reads events from the receiver, matches
    /// against subscriptions, and triggers workflow runs.
    pub fn spawn(
        self: Arc<Self>,
        mut rx: tokio::sync::mpsc::UnboundedReceiver<Event>,
        manager: Arc<dyn WorkflowManager>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                let matches = self.match_event(&event).await;
                for sub in matches {
                    let mut inputs = sub.default_inputs.clone();
                    // Merge event payload into inputs
                    if let serde_json::Value::Object(ref mut map) = inputs {
                        map.insert("_event_source".to_string(), serde_json::json!(event.source));
                        map.insert("_event_payload".to_string(), event.payload.clone());
                        map.insert("_event_origin".to_string(), serde_json::json!(event.origin));
                    }

                    // Use run_inline with the inline definition from agent.json
                    if let Some(ref def_json) = sub.definition_json {
                        let detail = Some(format!("{}:{}", sub.binding_name, event.source));
                        match manager.run_inline(def_json.clone(), inputs, "event", detail, &sub.agent_source, sub.emit_source.clone()).await {
                            Ok(run_id) => {
                                info!(
                                    agent = %sub.agent_source,
                                    binding = %sub.binding_name,
                                    run_id = %run_id,
                                    event_source = %event.source,
                                    "event triggered inline workflow run"
                                );
                            }
                            Err(e) => {
                                warn!(
                                    agent = %sub.agent_source,
                                    binding = %sub.binding_name,
                                    event_source = %event.source,
                                    error = %e,
                                    "failed to trigger inline workflow from event"
                                );
                            }
                        }
                    } else {
                        warn!(
                            agent = %sub.agent_source,
                            binding = %sub.binding_name,
                            event_source = %event.source,
                            "event subscription has no inline definition, skipping"
                        );
                    }
                }
            }
        })
    }
}

impl Default for EventDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

/// Match an event source against a subscription pattern.
///
/// Supports:
/// - Exact match: "email.urgent" matches "email.urgent"
/// - Wildcard suffix: "email.*" matches "email.urgent", "email.info", etc.
fn source_matches(pattern: &str, source: &str) -> bool {
    if pattern == source {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix(".*") {
        return source.starts_with(prefix) && source[prefix.len()..].starts_with('.');
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_source_matches_exact() {
        assert!(source_matches("email.urgent", "email.urgent"));
        assert!(!source_matches("email.urgent", "email.info"));
    }

    #[test]
    fn test_source_matches_wildcard() {
        assert!(source_matches("email.*", "email.urgent"));
        assert!(source_matches("email.*", "email.info"));
        assert!(!source_matches("email.*", "calendar.changed"));
        assert!(!source_matches("email.*", "emailurgent"));
    }
}
