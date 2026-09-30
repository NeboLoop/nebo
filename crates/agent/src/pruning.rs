//! Token estimates for stored messages: the checkpoint trigger and the
//! per-step trim read them.

use db::models::ChatMessage;

/// Chars estimate for a base64 image.
pub(crate) const IMAGE_CHAR_ESTIMATE: usize = 8000;

/// Estimate tokens for a message.
pub fn estimate_message_tokens(msg: &ChatMessage) -> usize {
    let mut chars = msg.content.len();
    if let Some(ref tc) = msg.tool_calls {
        chars += tc.len();
    }
    if let Some(ref tr) = msg.tool_results {
        chars += results_chars(tr);
    }
    // Check for image content
    if msg.content.contains("data:image/") {
        chars += IMAGE_CHAR_ESTIMATE;
    }
    chars / crate::CHARS_PER_TOKEN
}

/// The characters of stored tool results the model reads: an `image_url`
/// stays behind for the owner's app and is never sent (`sidecar` read the
/// picture into the result's text), so its bytes are not counted. A
/// multi-megabyte data URI counted as text once put one image read at a
/// million tokens and a checkpoint on every step (2026-09-30).
fn results_chars(results: &str) -> usize {
    if !results.contains("\"image_url\"") {
        return results.len();
    }
    let Ok(mut rows) = serde_json::from_str::<serde_json::Value>(results) else {
        return results.len();
    };
    for row in rows.as_array_mut().into_iter().flatten().filter_map(|r| r.as_object_mut()) {
        row.remove("image_url");
    }
    rows.to_string().len()
}

/// Estimate total tokens for all messages.
pub fn estimate_total_tokens(messages: &[ChatMessage]) -> usize {
    messages.iter().map(estimate_message_tokens).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_msg(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            id: uuid::Uuid::new_v4().to_string(),
            chat_id: "test".to_string(),
            role: role.to_string(),
            content: content.to_string(),
            metadata: None,
            created_at: 0,
            day_marker: None,
            tool_calls: None,
            tool_results: None,
            token_estimate: None,
            html: None,
        }
    }

    #[test]
    fn test_estimate_tokens() {
        let msg = make_msg("user", "hello world"); // 11 chars -> 2 tokens
        assert_eq!(estimate_message_tokens(&msg), 2);
    }

    /// A picture's bytes are never sent, so they never count: a read of a
    /// 4 MB image estimates as its text, not as a million tokens.
    #[test]
    fn an_image_on_a_result_is_not_counted_as_text() {
        let mut msg = make_msg("tool", "");
        let uri = format!("data:image/png;base64,{}", "A".repeat(4_000_000));
        let reading = "[Image: /plans/site.png, 6000×4000 px] TEXT: DECK 12'x16'";
        msg.tool_results = Some(serde_json::json!([{ "tool_call_id": "c1", "content": reading, "image_url": uri }]).to_string());
        assert!(estimate_message_tokens(&msg) < 100, "{}", estimate_message_tokens(&msg));
    }
}
