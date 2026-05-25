use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// How often (in turns) to run memory extraction. Extraction only fires
/// on every Nth turn to reduce LLM calls while still capturing facts.
const EXTRACTION_TURN_INTERVAL: u32 = 3;

/// Minimum number of tool calls required before extraction can fire.
/// Prevents extraction on short Q&A exchanges without substantive work.
const MIN_TOOL_CALLS: u32 = 3;

/// Debounces memory extraction per session.
/// New messages reset the timer so extraction only runs when idle.
/// Also tracks a per-session turn counter so extraction only fires
/// every `EXTRACTION_TURN_INTERVAL` turns AND after at least
/// `MIN_TOOL_CALLS` tool calls have occurred.
pub struct MemoryDebouncer {
    pending: Arc<Mutex<HashMap<String, CancellationToken>>>,
    turn_counts: Arc<Mutex<HashMap<String, u32>>>,
    tool_call_counts: Arc<Mutex<HashMap<String, u32>>>,
    delay: Duration,
}

impl MemoryDebouncer {
    /// Create a debouncer with the given idle delay.
    pub fn new(delay: Duration) -> Self {
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
            turn_counts: Arc::new(Mutex::new(HashMap::new())),
            tool_call_counts: Arc::new(Mutex::new(HashMap::new())),
            delay,
        }
    }

    /// Record a tool call for the given session. Must be called each time
    /// a tool is invoked so the extraction threshold can be met.
    pub async fn record_tool_call(&self, session_id: &str) {
        let mut counts = self.tool_call_counts.lock().await;
        let count = counts.entry(session_id.to_string()).or_insert(0);
        *count += 1;
    }

    /// Schedule extraction for a session. Cancels any pending timer.
    /// Increments a per-session turn counter and only runs the extraction
    /// task every `EXTRACTION_TURN_INTERVAL` turns.
    pub async fn schedule<F, Fut>(&self, session_id: &str, task: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send,
    {
        // Increment turn counter and check if this turn should run extraction.
        // Extraction requires BOTH the turn threshold AND the tool call threshold.
        let should_extract = {
            let mut counts = self.turn_counts.lock().await;
            let count = counts.entry(session_id.to_string()).or_insert(0);
            *count += 1;
            if *count >= EXTRACTION_TURN_INTERVAL {
                let mut tc_counts = self.tool_call_counts.lock().await;
                let tc_count = tc_counts.entry(session_id.to_string()).or_insert(0);
                if *tc_count >= MIN_TOOL_CALLS {
                    *count = 0;
                    *tc_count = 0;
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };

        if !should_extract {
            return;
        }

        let mut pending = self.pending.lock().await;

        // Cancel existing timer gracefully
        if let Some(token) = pending.remove(session_id) {
            token.cancel();
        }

        let delay = self.delay;
        let pending_ref = self.pending.clone();
        let session_key = session_id.to_string();
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn(async move {
            tokio::select! {
                _ = tokio::time::sleep(delay) => {
                    // Remove ourselves from pending before running
                    pending_ref.lock().await.remove(&session_key);
                    task().await;
                }
                _ = cancel_clone.cancelled() => {
                    // Timer cancelled by a newer schedule call
                }
            }
        });
        crate::memory_flush::track_extraction(handle).await;

        pending.insert(session_id.to_string(), cancel);
    }
}

impl Default for MemoryDebouncer {
    fn default() -> Self {
        Self::new(Duration::from_secs(5))
    }
}

/// Returns the global singleton `MemoryDebouncer` instance.
pub fn global() -> &'static MemoryDebouncer {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<MemoryDebouncer> = OnceLock::new();
    INSTANCE.get_or_init(MemoryDebouncer::default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn test_debounce_fires_on_third_turn_with_tool_calls() {
        let counter = Arc::new(AtomicU32::new(0));
        let debouncer = MemoryDebouncer::new(Duration::from_millis(50));

        // Record enough tool calls to meet the threshold
        for _ in 0..3 {
            debouncer.record_tool_call("session1").await;
        }

        // Turns 1 and 2 should NOT fire (throttled)
        for _ in 0..2 {
            let c = counter.clone();
            debouncer
                .schedule("session1", move || async move {
                    c.fetch_add(1, Ordering::SeqCst);
                })
                .await;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 0, "should not fire before 3rd turn");

        // Turn 3 should fire (turn threshold AND tool call threshold met)
        let c = counter.clone();
        debouncer
            .schedule("session1", move || async move {
                c.fetch_add(1, Ordering::SeqCst);
            })
            .await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 1, "should fire on 3rd turn with enough tool calls");
    }

    #[tokio::test]
    async fn test_debounce_blocked_without_tool_calls() {
        let counter = Arc::new(AtomicU32::new(0));
        let debouncer = MemoryDebouncer::new(Duration::from_millis(50));

        // Schedule 3 turns without any tool calls — should NOT fire
        for _ in 0..3 {
            let c = counter.clone();
            debouncer
                .schedule("session1", move || async move {
                    c.fetch_add(1, Ordering::SeqCst);
                })
                .await;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 0, "should not fire without enough tool calls");
    }

    #[tokio::test]
    async fn test_debounce_different_sessions() {
        let counter = Arc::new(AtomicU32::new(0));
        let debouncer = MemoryDebouncer::new(Duration::from_millis(50));

        // Record enough tool calls for both sessions
        for _ in 0..3 {
            debouncer.record_tool_call("session1").await;
            debouncer.record_tool_call("session2").await;
        }

        // Each session needs 3 turns to fire — schedule 3 for each
        for _ in 0..3 {
            let c1 = counter.clone();
            debouncer
                .schedule("session1", move || async move {
                    c1.fetch_add(1, Ordering::SeqCst);
                })
                .await;

            let c2 = counter.clone();
            debouncer
                .schedule("session2", move || async move {
                    c2.fetch_add(1, Ordering::SeqCst);
                })
                .await;
        }

        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 2, "both sessions should fire on 3rd turn");
    }
}
