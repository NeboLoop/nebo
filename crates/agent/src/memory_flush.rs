use std::sync::Arc;
use std::time::Duration;

use ai::{EmbeddingProvider, Provider};
use db::Store;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::memory;

/// Estimated token-to-character ratio.

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
    let estimated_tokens = total_chars / crate::CHARS_PER_TOKEN;
    let threshold = (auto_compact_tokens as f64 * 0.75) as usize;

    estimated_tokens >= threshold
}

/// The pre-checkpoint memory flush, for session `session_id` only: the
/// conversation the checkpoint is about to summarise (the session's active
/// chat from its last boundary on) is read now, before the boundary is
/// written, and its facts are extracted in the background, so the turn never
/// waits on it (Claude Code runs memory extraction as a background fork).
/// Tracked for shutdown like every background extraction.
pub async fn spawn_memory_flush(
    provider: Arc<dyn Provider>,
    store: Arc<Store>,
    session_id: String,
    user_id: String,
    topics: Vec<napp::agent::MemoryTopic>,
    embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    provenance: Vec<types::provenance::ProvenanceClass>,
    window_tokens: usize,
) {
    let messages = match store.get_chat_messages_since_checkpoint(&store.resolve_session_chat_id(&session_id)) {
        Ok(msgs) => msgs,
        Err(e) => {
            warn!(session_id, error = %e, "memory flush: failed to load messages");
            return;
        }
    };
    if messages.is_empty() {
        return;
    }
    let handle = tokio::spawn(async move {
        info!(session_id, message_count = messages.len(), "running pre-checkpoint memory flush");
        if let Some(facts) = memory::extract_facts(
            ai::RequestTrace::new("memory_flush"),
            provider.as_ref(),
            &messages,
            Some((store.as_ref(), &user_id)),
            &topics,
            "",
            None,
            window_tokens,
        )
        .await
        {
            memory::store_facts(&store, &facts, &user_id, embedding_provider, &topics, &provenance);
            debug!(session_id, "memory flush extraction complete");
        }
        let compaction_count = match store.get_session(&session_id) {
            Ok(Some(s)) => s.compaction_count.unwrap_or(0),
            _ => return,
        };
        if let Err(e) = store.update_session_memory_flush(&session_id, compaction_count) {
            warn!(error = %e, "failed to update memory flush tracking");
        }
    });
    track_extraction(handle).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Answers `{}` and keeps what each call read; holds every call while
    /// `hold` is set.
    #[derive(Default)]
    struct Reader {
        hold: std::sync::atomic::AtomicBool,
        read: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl Provider for Reader {
        fn id(&self) -> &str {
            "reader"
        }
        async fn stream(&self, req: &ai::ChatRequest) -> Result<ai::EventReceiver, ai::ProviderError> {
            self.read.lock().unwrap().push(req.messages[0].content.clone());
            while self.hold.load(std::sync::atomic::Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let (tx, rx) = tokio::sync::mpsc::channel(2);
            let _ = tx.send(ai::StreamEvent::text("{}")).await;
            let _ = tx.send(ai::StreamEvent::done()).await;
            Ok(rx)
        }
    }

    /// B15: a flush is its own session's: while another session's flush is
    /// still running, this one reads and extracts its own conversation at
    /// once, and nobody runs a flush for another session.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_flush_runs_for_its_own_session_beside_another() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(&dir.path().join("f.db").to_string_lossy()).unwrap());
        let sessions = crate::session::SessionManager::new(store.clone());
        let a = sessions.get_or_create("agent:a:web", "").unwrap().id;
        let b = sessions.get_or_create("agent:b:web", "").unwrap().id;
        sessions.append_message(&a, "user", "ALPHA: the Rivera deposit is 5%.", None, None, None).unwrap();
        sessions.append_message(&b, "user", "BRAVO: the Chen lease ends in March.", None, None, None).unwrap();
        let slow = Arc::new(Reader::default());
        slow.hold.store(true, std::sync::atomic::Ordering::SeqCst);
        let quick = Arc::new(Reader::default());
        let flush = |p: &Arc<Reader>, sid: &str| {
            spawn_memory_flush(p.clone(), store.clone(), sid.to_string(), "u".into(), vec![], None, vec![], 200_000)
        };
        flush(&slow, &a).await;
        flush(&quick, &b).await;
        for _ in 0..200 {
            if !quick.read.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let read = quick.read.lock().unwrap().clone();
        assert_eq!(read.len(), 1, "B's flush ran while A's was still running");
        assert!(read[0].contains("BRAVO") && !read[0].contains("ALPHA"), "only its own conversation: {}", read[0]);
        slow.hold.store(false, std::sync::atomic::Ordering::SeqCst);
        drain_extractions().await;
        let slow_read = slow.read.lock().unwrap().clone();
        assert_eq!(slow_read.len(), 1, "A's flush ran once, for A");
        assert!(slow_read[0].contains("ALPHA") && !slow_read[0].contains("BRAVO"));
    }

    #[test]
    fn test_chars_per_token() {
        assert_eq!(crate::CHARS_PER_TOKEN, 4);
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
        let estimated_tokens = text.len() / crate::CHARS_PER_TOKEN;
        // 58 chars / 4 = 14 tokens
        assert_eq!(estimated_tokens, 14);
    }
}
