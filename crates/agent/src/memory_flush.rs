use ai::Provider;
use db::Store;
use tracing::{debug, info, warn};

use crate::memory;

/// Estimated token-to-character ratio.
const CHARS_PER_TOKEN: usize = 4;

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
pub async fn run_memory_flush(
    provider: &dyn Provider,
    store: &Store,
    session_id: &str,
    user_id: &str,
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
    if let Some(facts) = memory::extract_facts(provider, &messages).await {
        memory::store_facts(store, &facts, user_id);
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
