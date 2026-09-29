//! Presence tracking for the owner's focus state: the [`PresenceTracker`]
//! records per-session presence (focused, unfocused, away).

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
}
