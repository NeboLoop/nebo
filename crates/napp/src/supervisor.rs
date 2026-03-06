use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use tracing::{info, warn};

/// Restart policy state for an app.
struct RestartState {
    restart_count: u32,
    last_restart: Instant,
    backoff: Duration,
    window_start: Instant,
}

impl RestartState {
    fn new() -> Self {
        Self {
            restart_count: 0,
            // Set far in the past so first restart is immediate
            last_restart: Instant::now() - Duration::from_secs(3600),
            backoff: Duration::from_secs(10),
            window_start: Instant::now(),
        }
    }

    /// Check if restart is allowed (max 5 per hour).
    fn can_restart(&self) -> bool {
        if self.window_start.elapsed() > Duration::from_secs(3600) {
            return true; // Reset window
        }
        self.restart_count < 5
    }

    /// Record a restart and calculate next backoff.
    fn record_restart(&mut self) {
        // Reset window if needed
        if self.window_start.elapsed() > Duration::from_secs(3600) {
            self.restart_count = 0;
            self.window_start = Instant::now();
            self.backoff = Duration::from_secs(10);
        }

        self.restart_count += 1;
        self.last_restart = Instant::now();
        // Exponential backoff: 10s, 20s, 40s, 80s, 160s (cap at 5min)
        self.backoff = (self.backoff * 2).min(Duration::from_secs(300));
    }
}

/// Supervisor monitors running apps and restarts them on failure.
pub struct Supervisor {
    states: RwLock<HashMap<String, RestartState>>,
    check_interval: Duration,
}

impl Supervisor {
    pub fn new() -> Self {
        Self {
            states: RwLock::new(HashMap::new()),
            check_interval: Duration::from_secs(15),
        }
    }

    /// Register an app for supervision.
    pub async fn watch(&self, app_id: &str) {
        let mut states = self.states.write().await;
        states.insert(app_id.to_string(), RestartState::new());
    }

    /// Unregister an app from supervision.
    pub async fn unwatch(&self, app_id: &str) {
        let mut states = self.states.write().await;
        states.remove(app_id);
    }

    /// Check if an app should be restarted.
    pub async fn should_restart(&self, app_id: &str) -> bool {
        let states = self.states.read().await;
        match states.get(app_id) {
            Some(state) => {
                if !state.can_restart() {
                    warn!(app = app_id, "restart limit exceeded (5/hour)");
                    return false;
                }
                // Respect backoff
                state.last_restart.elapsed() >= state.backoff
            }
            None => false,
        }
    }

    /// Record a restart event.
    pub async fn record_restart(&self, app_id: &str) {
        let mut states = self.states.write().await;
        if let Some(state) = states.get_mut(app_id) {
            state.record_restart();
            info!(
                app = app_id,
                count = state.restart_count,
                backoff_secs = state.backoff.as_secs(),
                "recorded restart"
            );
        }
    }

    /// Get restart count for an app.
    pub async fn restart_count(&self, app_id: &str) -> u32 {
        let states = self.states.read().await;
        states.get(app_id).map(|s| s.restart_count).unwrap_or(0)
    }

    /// Get the check interval.
    pub fn check_interval(&self) -> Duration {
        self.check_interval
    }
}

impl Default for Supervisor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_supervisor_watch_unwatch() {
        let sup = Supervisor::new();
        sup.watch("app-1").await;
        assert!(sup.should_restart("app-1").await);
        sup.unwatch("app-1").await;
        assert!(!sup.should_restart("app-1").await);
    }

    #[tokio::test]
    async fn test_restart_count() {
        let sup = Supervisor::new();
        sup.watch("app-1").await;
        sup.record_restart("app-1").await;
        sup.record_restart("app-1").await;
        assert_eq!(sup.restart_count("app-1").await, 2);
    }
}
