use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// How often (in turns) to run memory extraction. Extraction only fires
/// on every Nth turn to reduce LLM calls while still capturing facts.
const EXTRACTION_TURN_INTERVAL: u32 = 3;

/// Debounces memory extraction per session.
/// New messages reset the timer so extraction only runs when idle.
/// Also tracks a per-session turn counter so extraction only fires
/// every `EXTRACTION_TURN_INTERVAL` turns. Deliberately NOT gated on tool
/// activity: ordinary tool-less conversation is where preferences, entities,
/// and personality facts surface, and a tool-call prerequisite meant those
/// were never captured at all. The extraction prompt's own selectivity is the
/// content filter.
pub struct MemoryDebouncer {
    pending: Arc<Mutex<HashMap<String, CancellationToken>>>,
    turn_counts: Arc<Mutex<HashMap<String, u32>>>,
    delay: Duration,
}

impl MemoryDebouncer {
    /// Create a debouncer with the given idle delay.
    pub fn new(delay: Duration) -> Self {
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
            turn_counts: Arc::new(Mutex::new(HashMap::new())),
            delay,
        }
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
        let should_extract = {
            let mut counts = self.turn_counts.lock().await;
            let count = counts.entry(session_id.to_string()).or_insert(0);
            *count += 1;
            if *count >= EXTRACTION_TURN_INTERVAL {
                *count = 0;
                true
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
    async fn test_debounce_fires_on_third_turn() {
        let counter = Arc::new(AtomicU32::new(0));
        let debouncer = MemoryDebouncer::new(Duration::from_millis(50));

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

        // Turn 3 should fire — no tool activity required: tool-less chats are
        // where preferences/entities surface, and gating on tools meant those
        // were never captured.
        let c = counter.clone();
        debouncer
            .schedule("session1", move || async move {
                c.fetch_add(1, Ordering::SeqCst);
            })
            .await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 1, "should fire on 3rd turn without tool calls");
    }

    #[tokio::test]
    async fn test_debounce_different_sessions() {
        let counter = Arc::new(AtomicU32::new(0));
        let debouncer = MemoryDebouncer::new(Duration::from_millis(50));

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
