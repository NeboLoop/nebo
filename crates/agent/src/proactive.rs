//! Proactive behavior infrastructure — in-memory inbox for background agent results
//! and presence tracking for user focus state.
//!
//! The [`ProactiveInbox`] collects notifications from background tasks (heartbeats,
//! cron jobs, etc.) and drains them into the steering pipeline when the user's
//! session becomes active.
//!
//! The [`PresenceTracker`] records per-session user presence (focused, unfocused,
//! away) so steering generators can adapt behavior accordingly.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

// ── Presence ──────────────────────────────────────────────────────────

/// User presence state for a WebSocket session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    Focused,
    Unfocused,
    Away,
}

impl Presence {
    /// Parse from the wire format sent by the frontend.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "focused" => Some(Self::Focused),
            "unfocused" => Some(Self::Unfocused),
            "away" => Some(Self::Away),
            _ => None,
        }
    }

    /// Convert to a string for steering context.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Focused => "focused",
            Self::Unfocused => "unfocused",
            Self::Away => "away",
        }
    }
}

/// Per-session presence state with transition tracking.
#[derive(Debug, Clone)]
struct PresenceEntry {
    current: Presence,
    /// The previous presence state (for detecting "user returned" transitions).
    previous: Presence,
}

/// Thread-safe presence tracker — one instance shared across the server.
#[derive(Debug, Clone, Default)]
pub struct PresenceTracker {
    state: Arc<RwLock<HashMap<String, PresenceEntry>>>,
}

impl PresenceTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Update presence for a session. Records the previous state for transition detection.
    pub async fn set(&self, session_id: &str, presence: Presence) {
        let mut map = self.state.write().await;
        let entry = map
            .entry(session_id.to_string())
            .or_insert_with(|| PresenceEntry {
                current: Presence::Focused,
                previous: Presence::Focused,
            });
        entry.previous = entry.current;
        entry.current = presence;
    }

    /// Get current presence for a session. Returns None if never set.
    pub async fn get(&self, session_id: &str) -> Option<Presence> {
        let map = self.state.read().await;
        map.get(session_id).map(|e| e.current)
    }

    /// Check if the user just returned (transitioned from unfocused/away to focused).
    pub async fn just_returned(&self, session_id: &str) -> bool {
        let map = self.state.read().await;
        if let Some(entry) = map.get(session_id) {
            entry.current == Presence::Focused
                && (entry.previous == Presence::Unfocused || entry.previous == Presence::Away)
        } else {
            false
        }
    }

    /// Clear presence for a disconnected session.
    pub async fn remove(&self, session_id: &str) {
        let mut map = self.state.write().await;
        map.remove(session_id);
    }
}

// ── Proactive Inbox ──────────────────────────────────────────────────

/// Priority level for proactive items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Low,
    Normal,
    Urgent,
}

impl std::fmt::Display for Priority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Priority::Low => write!(f, "low"),
            Priority::Normal => write!(f, "normal"),
            Priority::Urgent => write!(f, "urgent"),
        }
    }
}

/// A single proactive notification from a background task.
#[derive(Debug, Clone)]
pub struct ProactiveItem {
    /// Source identifier, e.g. "heartbeat:gws-email", "cron:daily-brief".
    pub source: String,
    /// Human-readable summary, e.g. "3 urgent emails from your boss".
    pub summary: String,
    /// Priority level.
    pub priority: Priority,
    /// Unix timestamp when the item was created.
    pub created_at: i64,
}

/// Thread-safe in-memory inbox for background agent results.
///
/// Background tasks push items here. The steering pipeline drains them
/// at the start of each iteration, injecting summaries into the conversation.
#[derive(Debug, Clone, Default)]
pub struct ProactiveInbox {
    items: Arc<RwLock<HashMap<String, Vec<ProactiveItem>>>>,
}

impl ProactiveInbox {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an item to a session's inbox.
    pub async fn push(&self, session_id: &str, item: ProactiveItem) {
        let mut map = self.items.write().await;
        map.entry(session_id.to_string())
            .or_default()
            .push(item);
    }

    /// Take all pending items for a session (empties the inbox for that session).
    pub async fn drain(&self, session_id: &str) -> Vec<ProactiveItem> {
        let mut map = self.items.write().await;
        map.remove(session_id).unwrap_or_default()
    }

    /// Check if there are pending items for a session.
    pub async fn has_pending(&self, session_id: &str) -> bool {
        let map = self.items.read().await;
        map.get(session_id).is_some_and(|v| !v.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_presence_tracker() {
        let tracker = PresenceTracker::new();

        // Initially no presence
        assert!(tracker.get("sess1").await.is_none());

        // Set focused
        tracker.set("sess1", Presence::Focused).await;
        assert_eq!(tracker.get("sess1").await, Some(Presence::Focused));
        assert!(!tracker.just_returned("sess1").await);

        // Transition to away
        tracker.set("sess1", Presence::Away).await;
        assert_eq!(tracker.get("sess1").await, Some(Presence::Away));
        assert!(!tracker.just_returned("sess1").await);

        // Return to focused
        tracker.set("sess1", Presence::Focused).await;
        assert!(tracker.just_returned("sess1").await);

        // Remove
        tracker.remove("sess1").await;
        assert!(tracker.get("sess1").await.is_none());
    }

    #[tokio::test]
    async fn test_proactive_inbox() {
        let inbox = ProactiveInbox::new();

        assert!(!inbox.has_pending("sess1").await);

        inbox
            .push(
                "sess1",
                ProactiveItem {
                    source: "test".to_string(),
                    summary: "test item".to_string(),
                    priority: Priority::Normal,
                    created_at: 1000,
                },
            )
            .await;

        assert!(inbox.has_pending("sess1").await);

        let items = inbox.drain("sess1").await;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].summary, "test item");

        // After drain, inbox should be empty
        assert!(!inbox.has_pending("sess1").await);
    }
}
