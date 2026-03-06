use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Debounces memory extraction per session.
/// New messages reset the timer so extraction only runs when idle.
pub struct MemoryDebouncer {
    pending: Arc<Mutex<HashMap<String, CancellationToken>>>,
    delay: Duration,
}

impl MemoryDebouncer {
    /// Create a debouncer with the given idle delay.
    pub fn new(delay: Duration) -> Self {
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
            delay,
        }
    }

    /// Schedule extraction for a session. Cancels any pending timer.
    pub async fn schedule<F, Fut>(&self, session_id: &str, task: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send,
    {
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

        tokio::spawn(async move {
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

        pending.insert(session_id.to_string(), cancel);
    }
}

impl Default for MemoryDebouncer {
    fn default() -> Self {
        Self::new(Duration::from_secs(5))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn test_debounce_fires_once() {
        let counter = Arc::new(AtomicU32::new(0));
        let debouncer = MemoryDebouncer::new(Duration::from_millis(50));

        // Schedule 3 times rapidly — only the last should fire
        for _ in 0..3 {
            let c = counter.clone();
            debouncer.schedule("session1", move || async move {
                c.fetch_add(1, Ordering::SeqCst);
            }).await;
        }

        // Wait for debounce to fire
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_debounce_different_sessions() {
        let counter = Arc::new(AtomicU32::new(0));
        let debouncer = MemoryDebouncer::new(Duration::from_millis(50));

        // Different sessions should fire independently
        let c1 = counter.clone();
        debouncer.schedule("session1", move || async move {
            c1.fetch_add(1, Ordering::SeqCst);
        }).await;

        let c2 = counter.clone();
        debouncer.schedule("session2", move || async move {
            c2.fetch_add(1, Ordering::SeqCst);
        }).await;

        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }
}
