use std::sync::Arc;

use ai::EmbeddingProvider;
use db::Store;
use db::models::ChatMessage;
use tracing::{debug, info, warn};

use crate::chunking;

/// Number of messages to group into a single indexing block.
const BLOCK_SIZE: usize = 5;

/// Index a session's conversation for later semantic search, called when
/// the conversation is checkpointed: every owner message and reply not yet
/// indexed (the session's high-water mark is the last row's rowid) is
/// grouped into blocks, chunked, embedded, and stored in memory_chunks
/// under `user_id`, so recall finds what the checkpoint summarized away.
/// Nebo's own rows (attachments, checkpoint boundaries, hidden prompts)
/// are not the conversation and are skipped.
pub async fn index_compacted_messages(
    store: &Arc<Store>,
    embedding_provider: &dyn EmbeddingProvider,
    session_id: &str,
    user_id: &str,
) {
    let last_embedded_id = match store.get_session(session_id) {
        Ok(Some(session)) => session.last_embedded_message_id.unwrap_or(0),
        _ => 0,
    };
    let rows = match store.get_chat_messages_after_rowid(&store.resolve_session_chat_id(session_id), last_embedded_id) {
        Ok(rows) => rows,
        Err(e) => {
            debug!(error = %e, "failed to load messages for transcript indexing");
            return;
        }
    };
    let new_messages: Vec<&(i64, ChatMessage)> = rows
        .iter()
        .filter(|(_, m)| (m.role == "user" || m.role == "assistant") && !m.content.trim().is_empty() && !nebos_own(m))
        .collect();

    if new_messages.is_empty() {
        return;
    }

    let model = embedding_provider.id().to_string();
    let dims = embedding_provider.dimensions() as i64;
    let mut highest_id: i64 = last_embedded_id;

    // Group into blocks of BLOCK_SIZE
    for block in new_messages.chunks(BLOCK_SIZE) {
        // Concatenate block messages
        let block_text: String = block
            .iter()
            .map(|(_, m)| format!("{}: {}", m.role, head(&m.content, 500)))
            .collect::<Vec<_>>()
            .join("\n");

        // Chunk the block text
        let chunks = chunking::chunk_text_default(&block_text);
        let chunk_texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();

        // Embed
        let embeddings = match embedding_provider.embed(&chunk_texts).await {
            Ok(e) => e,
            Err(e) => {
                // The mark stays before this block: the next checkpoint
                // tries it again.
                warn!(error = %e, "transcript embedding failed");
                break;
            }
        };

        // Store chunks and embeddings
        for (i, (chunk, embedding)) in chunks.iter().zip(embeddings.iter()).enumerate() {
            let chunk_id = match store.insert_memory_chunk(
                None, // no parent memory
                i as i64,
                &chunk.text,
                "session",
                session_id,
                chunk.start_char as i64,
                chunk.end_char as i64,
                &model,
                user_id,
            ) {
                Ok(id) => id,
                Err(e) => {
                    debug!(error = %e, "failed to insert session chunk");
                    continue;
                }
            };

            let blob = ai::f32_to_bytes(embedding);
            if let Err(e) = store.insert_memory_embedding(chunk_id, &model, dims, &blob) {
                debug!(error = %e, "failed to insert session embedding");
            }
        }
        for (rowid, _) in block {
            highest_id = highest_id.max(*rowid);
        }
    }

    // Update high-water mark
    if highest_id > last_embedded_id {
        if let Err(e) = store.update_session_last_embedded_message_id(session_id, highest_id) {
            warn!(error = %e, "failed to update last_embedded_message_id");
        } else {
            info!(
                session_id,
                highest_id, "updated transcript index high-water mark"
            );
        }
    }
}

/// A row Nebo wrote for itself rather than a message of the conversation:
/// an attachment, a checkpoint boundary, a hidden prompt.
fn nebos_own(msg: &ChatMessage) -> bool {
    let Some(meta) = msg.metadata.as_deref().and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok()) else {
        return false;
    };
    ["isMeta", "checkpoint", "hiddenPrompt"]
        .iter()
        .any(|k| meta.get(*k).and_then(|v| v.as_bool()) == Some(true))
}

/// At most `max` bytes of `text`, cut on a character boundary.
fn head(text: &str, max: usize) -> &str {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_block_size() {
        assert_eq!(BLOCK_SIZE, 5);
    }

    #[test]
    fn test_message_text_truncation() {
        let long_content = "x".repeat(1000);
        let truncated = &long_content[..long_content.len().min(500)];
        assert_eq!(truncated.len(), 500);
    }

    #[test]
    fn test_block_grouping() {
        let items: Vec<i32> = (0..12).collect();
        let blocks: Vec<&[i32]> = items.chunks(BLOCK_SIZE).collect();
        assert_eq!(blocks.len(), 3); // 5 + 5 + 2
        assert_eq!(blocks[0].len(), 5);
        assert_eq!(blocks[2].len(), 2);
    }
}
