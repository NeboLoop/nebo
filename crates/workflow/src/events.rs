//! Event dispatcher — matches incoming events against agent-owned subscriptions
//! and triggers workflow runs.
//!
//! Duplicate prevention is handled upstream by the DedupeCache in watch_loop
//! (same payload won't emit twice within 10-minute TTL). Events that arrive
//! here are guaranteed unique and must fire instantly.

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
            // No time-based rate limiting — events must fire instantly.
            // Duplicate prevention is handled upstream by DedupeCache in watch_loop
            // (same payload won't emit twice within 10-minute TTL).

            while let Some(event) = rx.recv().await {
                let matches = self.match_event(&event).await;
                for sub in matches {
                    let mut inputs = sub.default_inputs.clone();
                    // Merge event payload into inputs
                    if let serde_json::Value::Object(ref mut map) = inputs {
                        map.insert("_event_source".to_string(), serde_json::json!(event.source));
                        map.insert(
                            "_event_payload".to_string(),
                            summarize_event_payload(&event.payload),
                        );
                        map.insert("_event_origin".to_string(), serde_json::json!(event.origin));
                    }

                    // Use run_inline with the inline definition from agent.json
                    if let Some(ref def_json) = sub.definition_json {
                        let detail = Some(format!("{}:{}", sub.binding_name, event.source));
                        match manager
                            .run_inline(
                                def_json.clone(),
                                inputs,
                                "event",
                                detail,
                                &sub.agent_source,
                                sub.emit_source.clone(),
                            )
                            .await
                        {
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

/// Summarize an event payload for workflow inputs.
///
/// Plugin event payloads (e.g. raw Gmail API responses) can be enormous (100KB+).
/// This extracts the useful fields into a clean, compact object the LLM can parse.
/// For email-like payloads with nested `payload.headers`, promotes From/To/Subject/Date
/// to top-level fields alongside `snippet`, `id`, `labels`, etc.
fn summarize_event_payload(payload: &serde_json::Value) -> serde_json::Value {
    let map = match payload.as_object() {
        Some(m) => m,
        None => return payload.clone(),
    };

    // If payload is small enough (<8KB serialized), pass it through
    let raw_len = serde_json::to_string(payload).map(|s| s.len()).unwrap_or(0);
    if raw_len < 8_000 {
        return payload.clone();
    }

    // Extract all scalar values (id, snippet, historyId, etc.)
    let mut summary = serde_json::Map::new();
    for (k, v) in map {
        match v {
            serde_json::Value::Object(_) => {}
            serde_json::Value::Array(arr) => {
                // Keep small arrays (e.g. labelIds), skip large ones
                if arr.len() <= 20 {
                    summary.insert(k.clone(), v.clone());
                }
            }
            _ => {
                summary.insert(k.clone(), v.clone());
            }
        }
    }

    // Promote nested headers (email-style payloads: payload.headers[{name, value}])
    if let Some(headers) = map
        .get("payload")
        .and_then(|p| p.get("headers"))
        .and_then(|h| h.as_array())
    {
        let promote = ["from", "to", "cc", "subject", "date", "reply-to"];
        for header in headers {
            if let (Some(name), Some(value)) = (
                header.get("name").and_then(|n| n.as_str()),
                header.get("value").and_then(|v| v.as_str()),
            ) {
                if promote.contains(&name.to_lowercase().as_str()) {
                    summary.insert(name.to_lowercase(), serde_json::json!(value));
                }
            }
        }
    }

    serde_json::Value::Object(summary)
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

    #[test]
    fn test_summarize_small_payload() {
        let small = serde_json::json!({"from": "alice@test.com", "subject": "Hello"});
        let result = summarize_event_payload(&small);
        assert_eq!(result, small); // unchanged — under 8KB
    }

    #[test]
    fn test_summarize_large_gmail_payload() {
        // Simulate a raw Gmail API response
        let mut headers = vec![];
        for _ in 0..50 {
            headers.push(serde_json::json!({"name": "Received", "value": "x".repeat(200)}));
        }
        headers.push(serde_json::json!({"name": "From", "value": "alice@test.com"}));
        headers.push(serde_json::json!({"name": "Subject", "value": "Meeting tomorrow"}));
        headers.push(serde_json::json!({"name": "To", "value": "bob@test.com"}));

        let payload = serde_json::json!({
            "id": "msg123",
            "threadId": "thread456",
            "snippet": "Hey, let's meet tomorrow at 3pm",
            "historyId": "12345",
            "labelIds": ["INBOX", "UNREAD"],
            "payload": {
                "headers": headers,
                "body": {"data": "x".repeat(10_000)},
                "mimeType": "text/html"
            }
        });

        let result = summarize_event_payload(&payload);
        let map = result.as_object().unwrap();

        // Scalars promoted
        assert_eq!(map["id"], "msg123");
        assert_eq!(map["snippet"], "Hey, let's meet tomorrow at 3pm");
        assert_eq!(map["threadId"], "thread456");

        // Headers promoted
        assert_eq!(map["from"], "alice@test.com");
        assert_eq!(map["subject"], "Meeting tomorrow");
        assert_eq!(map["to"], "bob@test.com");

        // Labels kept (small array)
        assert!(map.contains_key("labelIds"));

        // Massive nested payload removed
        assert!(!map.contains_key("payload"));
    }
}
