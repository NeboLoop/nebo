use std::sync::Arc;

use ai::EmbeddingProvider;
use db::models::ChatMessage;
use db::Store;
use tracing::{debug, info, warn};

use crate::chunking;

/// Number of messages to group into a single indexing block.
const BLOCK_SIZE: usize = 5;

/// Index compacted messages from a session for later semantic search.
/// Loads messages after the last embedded message, groups them into blocks,
/// chunks each block, embeds, and stores in memory_chunks.
pub async fn index_compacted_messages(
    store: &Arc<Store>,
    embedding_provider: &dyn EmbeddingProvider,
    session_id: &str,
    user_id: &str,
) {
    // Get the high-water mark
    let last_embedded_id = match store.get_session(session_id) {
        Ok(Some(session)) => session.last_embedded_message_id.unwrap_or(0),
        _ => 0,
    };

    // Load messages after the high-water mark
    let all_messages = match store.get_chat_messages(session_id) {
        Ok(msgs) => msgs,
        Err(e) => {
            debug!(error = %e, "failed to load messages for transcript indexing");
            return;
        }
    };

    // Filter to messages after last_embedded_id
    // Messages are sorted by created_at; use numeric id comparison
    let new_messages: Vec<&ChatMessage> = all_messages
        .iter()
        .filter(|m| {
            // Parse ID as i64 for comparison (IDs are numeric strings)
            m.id.parse::<i64>().unwrap_or(0) > last_embedded_id
        })
        .filter(|m| m.role == "user" || m.role == "assistant")
        .filter(|m| !m.content.is_empty())
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
            .map(|m| format!("{}: {}", m.role, &m.content[..m.content.len().min(500)]))
            .collect::<Vec<_>>()
            .join("\n");

        if block_text.is_empty() {
            continue;
        }

        // Track highest message ID in this block
        for msg in block {
            if let Ok(id) = msg.id.parse::<i64>() {
                highest_id = highest_id.max(id);
            }
        }

        // Chunk the block text
        let chunks = chunking::chunk_text_default(&block_text);
        let chunk_texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();

        // Embed
        let embeddings = match embedding_provider.embed(&chunk_texts).await {
            Ok(e) => e,
            Err(e) => {
                warn!(error = %e, "transcript embedding failed");
                continue;
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
    }

    // Update high-water mark
    if highest_id > last_embedded_id {
        if let Err(e) = store.update_session_last_embedded_message_id(session_id, highest_id) {
            warn!(error = %e, "failed to update last_embedded_message_id");
        } else {
            info!(
                session_id,
                highest_id,
                "updated transcript index high-water mark"
            );
        }
    }
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
