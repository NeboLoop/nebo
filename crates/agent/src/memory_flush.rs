use std::sync::Arc;
use std::time::Duration;

use ai::Provider;
use db::Store;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::memory;

/// Estimated token-to-character ratio.
const CHARS_PER_TOKEN: usize = 4;

/// How long `drain_extractions` waits for in-flight tasks before giving up.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

/// Global tracker for in-flight background extraction tasks (memory extraction,
/// LLM summary, indexing, personality synthesis). On shutdown the server calls
/// `drain_extractions()` to await these instead of dropping them silently.
static EXTRACTION_HANDLES: std::sync::OnceLock<Mutex<Vec<JoinHandle<()>>>> =
    std::sync::OnceLock::new();

fn extraction_handles() -> &'static Mutex<Vec<JoinHandle<()>>> {
    EXTRACTION_HANDLES.get_or_init(|| Mutex::new(Vec::new()))
}

/// Register a background extraction task handle for graceful shutdown tracking.
/// Call this whenever a `tokio::spawn` is used for memory extraction, LLM
/// summary, indexing, or personality synthesis.
pub async fn track_extraction(handle: JoinHandle<()>) {
    let mut handles = extraction_handles().lock().await;
    // Prune already-finished handles to keep the vec bounded.
    handles.retain(|h| !h.is_finished());
    handles.push(handle);
}

/// Await all in-flight extraction tasks with a timeout.
/// Called from the server shutdown path to avoid dropping work silently.
pub async fn drain_extractions() {
    let handles: Vec<JoinHandle<()>> = {
        let mut guard = extraction_handles().lock().await;
        guard.drain(..).filter(|h| !h.is_finished()).collect()
    };

    if handles.is_empty() {
        return;
    }

    info!(
        count = handles.len(),
        timeout_secs = DRAIN_TIMEOUT.as_secs(),
        "draining in-flight memory extractions..."
    );

    let drain_all = async {
        for handle in handles {
            let _ = handle.await;
        }
    };

    match tokio::time::timeout(DRAIN_TIMEOUT, drain_all).await {
        Ok(()) => info!("all in-flight extractions drained"),
        Err(_) => warn!(
            "extraction drain timed out after {}s — some tasks may be lost",
            DRAIN_TIMEOUT.as_secs()
        ),
    }
}

/// Pending flush context stashed when an extraction is already in progress.
struct PendingFlush {
    session_id: String,
    user_id: String,
    topics: Vec<napp::agent::MemoryTopic>,
}

/// Overlap guard: prevents concurrent memory extractions.
/// If a flush is already running, the new context is stashed as pending.
/// When the in-progress flush finishes, it checks for and runs the pending one.
static FLUSH_LOCK: std::sync::OnceLock<Mutex<Option<PendingFlush>>> = std::sync::OnceLock::new();

fn flush_state() -> &'static Mutex<Option<PendingFlush>> {
    FLUSH_LOCK.get_or_init(|| Mutex::new(None))
}

/// In-progress flag — separate from the pending-context mutex so we can
/// check "is running?" without holding the pending-context lock during
/// the (potentially long) extraction.
static FLUSH_IN_PROGRESS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Check whether a pre-compaction memory flush should run.
/// Returns true if the session has had new compactions and the message
/// window is large enough to warrant extraction from all messages.
pub fn should_run_memory_flush(
    store: &Store,
    session_id: &str,
    auto_compact_tokens: usize,
) -> bool {
    let session = match store.get_session(session_id) {
        Ok(Some(s)) => s,
        _ => return false,
    };

    let compaction_count = session.compaction_count.unwrap_or(0);
    let flush_compaction_count = session.memory_flush_compaction_count.unwrap_or(0);

    // Must have had new compactions since last flush
    if compaction_count <= flush_compaction_count {
        return false;
    }

    // Estimate token usage from messages
    let messages = match store.get_chat_messages(session_id) {
        Ok(msgs) => msgs,
        Err(_) => return false,
    };

    let total_chars: usize = messages.iter().map(|m| m.content.len()).sum();
    let estimated_tokens = total_chars / CHARS_PER_TOKEN;
    let threshold = (auto_compact_tokens as f64 * 0.75) as usize;

    estimated_tokens >= threshold
}

/// Run memory extraction from ALL messages in the session (not just the last 6).
/// This captures any facts that might have been lost during compaction.
///
/// Overlap guard: if an extraction is already in progress, the context is
/// stashed as pending and will be run when the current extraction finishes.
pub async fn run_memory_flush(
    provider: &dyn Provider,
    store: &Arc<Store>,
    session_id: &str,
    user_id: &str,
    topics: &[napp::agent::MemoryTopic],
) {
    // Check if an extraction is already in progress.
    if FLUSH_IN_PROGRESS.load(std::sync::atomic::Ordering::Acquire) {
        // Stash as pending — the in-progress extraction will pick it up.
        let mut pending = flush_state().lock().await;
        *pending = Some(PendingFlush {
            session_id: session_id.to_string(),
            user_id: user_id.to_string(),
            topics: topics.to_vec(),
        });
        debug!(session_id, "memory flush already in progress — stashed as pending");
        return;
    }

    // Mark in-progress.
    FLUSH_IN_PROGRESS.store(true, std::sync::atomic::Ordering::Release);

    run_flush_inner(provider, store, session_id, user_id, topics).await;

    // Finished — check for pending context.
    FLUSH_IN_PROGRESS.store(false, std::sync::atomic::Ordering::Release);

    let pending = {
        let mut guard = flush_state().lock().await;
        guard.take()
    };

    if let Some(ctx) = pending {
        debug!(
            session_id = %ctx.session_id,
            "running trailing memory flush from pending context"
        );
        // Mark in-progress again for the trailing run.
        FLUSH_IN_PROGRESS.store(true, std::sync::atomic::Ordering::Release);
        run_flush_inner(provider, store, &ctx.session_id, &ctx.user_id, &ctx.topics).await;
        FLUSH_IN_PROGRESS.store(false, std::sync::atomic::Ordering::Release);
    }
}

/// Core extraction logic (no overlap guard).
async fn run_flush_inner(
    provider: &dyn Provider,
    store: &Arc<Store>,
    session_id: &str,
    user_id: &str,
    topics: &[napp::agent::MemoryTopic],
) {
    let messages = match store.get_chat_messages(session_id) {
        Ok(msgs) => msgs,
        Err(e) => {
            warn!(error = %e, "memory flush: failed to load messages");
            return;
        }
    };

    if messages.is_empty() {
        return;
    }

    info!(
        session_id,
        message_count = messages.len(),
        "running pre-compaction memory flush"
    );

    // Extract from all messages
    if let Some(facts) =
        memory::extract_facts(provider, &messages, Some(store), Some(user_id), topics, "").await
    {
        memory::store_facts(store, &facts, user_id, None, topics);
        debug!(session_id, "memory flush extraction complete");
    }

    // Update flush tracking
    let session = match store.get_session(session_id) {
        Ok(Some(s)) => s,
        _ => return,
    };

    let compaction_count = session.compaction_count.unwrap_or(0);
    if let Err(e) = store.update_session_memory_flush(session_id, compaction_count) {
        warn!(error = %e, "failed to update memory flush tracking");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chars_per_token() {
        assert_eq!(CHARS_PER_TOKEN, 4);
    }

    #[test]
    fn test_threshold_calculation() {
        let auto_compact_tokens = 80_000usize;
        let threshold = (auto_compact_tokens as f64 * 0.75) as usize;
        assert_eq!(threshold, 60_000);
    }

    #[test]
    fn test_token_estimation() {
        let text = "Hello world, this is a test message for token estimation.";
        let estimated_tokens = text.len() / CHARS_PER_TOKEN;
        // 58 chars / 4 = 14 tokens
        assert_eq!(estimated_tokens, 14);
    }
}
