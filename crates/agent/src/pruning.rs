use db::models::ChatMessage;

/// Approximate chars per token.
const CHARS_PER_TOKEN: usize = 4;
/// Chars estimate for a base64 image.
const IMAGE_CHAR_ESTIMATE: usize = 8000;
/// Minimum token savings to bother micro-compacting.
const MICRO_COMPACT_MIN_SAVINGS: usize = 3000;
/// Protect the N most recent tool results from micro-compaction.
const MICRO_COMPACT_KEEP_RECENT: usize = 5;

/// Sliding window parameters.
pub const WINDOW_MAX_MESSAGES: usize = 20;
pub const WINDOW_MAX_TOKENS: usize = 40000;

/// Graduated context thresholds.
pub struct ContextThresholds {
    /// Micro-compact activates above this.
    pub warning: usize,
    /// Log warning about context size.
    pub error: usize,
    /// Trigger full compaction.
    pub auto_compact: usize,
}

impl ContextThresholds {
    /// Compute from model context window minus overhead.
    pub fn from_context_window(context_window: usize, prompt_overhead: usize) -> Self {
        let effective = context_window.saturating_sub(prompt_overhead);
        let auto_compact = std::cmp::min(effective, 500_000);
        let error = auto_compact.saturating_sub(10_000);
        let warning = auto_compact.saturating_sub(20_000);

        // Apply minimums
        Self {
            warning: std::cmp::max(warning, 40_000),
            error: std::cmp::max(error, 50_000),
            auto_compact,
        }
    }
}

/// Estimate tokens for a message.
pub fn estimate_message_tokens(msg: &ChatMessage) -> usize {
    let mut chars = msg.content.len();
    if let Some(ref tc) = msg.tool_calls {
        chars += tc.len();
    }
    if let Some(ref tr) = msg.tool_results {
        chars += tr.len();
    }
    // Check for image content
    if msg.content.contains("data:image/") {
        chars += IMAGE_CHAR_ESTIMATE;
    }
    chars / CHARS_PER_TOKEN
}

/// Estimate total tokens for all messages.
pub fn estimate_total_tokens(messages: &[ChatMessage]) -> usize {
    messages.iter().map(estimate_message_tokens).sum()
}

/// Apply sliding window: returns (window_messages, evicted_messages).
/// Never evicts messages with created_at >= run_start_time.
pub fn apply_sliding_window(
    messages: &[ChatMessage],
    run_start_time: i64,
) -> (Vec<ChatMessage>, Vec<ChatMessage>) {
    if messages.len() <= WINDOW_MAX_MESSAGES {
        let total = estimate_total_tokens(messages);
        if total <= WINDOW_MAX_TOKENS {
            return (messages.to_vec(), vec![]);
        }
    }

    // Walk backwards from end, accumulating tokens
    let mut window_start = messages.len();
    let mut accumulated_tokens = 0usize;
    let mut message_count = 0usize;

    for i in (0..messages.len()).rev() {
        let msg = &messages[i];

        // Never evict current-run messages
        if msg.created_at >= run_start_time {
            let tokens = estimate_message_tokens(msg);
            accumulated_tokens += tokens;
            message_count += 1;
            window_start = i;
            continue;
        }

        let tokens = estimate_message_tokens(msg);
        if accumulated_tokens + tokens > WINDOW_MAX_TOKENS || message_count >= WINDOW_MAX_MESSAGES {
            break;
        }

        accumulated_tokens += tokens;
        message_count += 1;
        window_start = i;
    }

    // Fix tool-pair boundaries: don't split tool_use from tool_result
    while window_start > 0 {
        let msg = &messages[window_start];
        // If first message is a tool result, include preceding assistant message
        if msg.role == "tool"
            || (msg.tool_results.is_some()
                && msg.tool_results.as_ref().is_some_and(|tr| !tr.is_empty() && tr != "[]"))
        {
            window_start -= 1;
        } else {
            break;
        }
    }

    let evicted = messages[..window_start].to_vec();
    let window = messages[window_start..].to_vec();

    (window, evicted)
}

/// Micro-compact: trim old tool results to reduce context size.
/// Returns modified messages and tokens saved.
pub fn micro_compact(
    messages: &[ChatMessage],
    warning_threshold: usize,
) -> (Vec<ChatMessage>, usize) {
    let total_tokens = estimate_total_tokens(messages);
    let mut result = messages.to_vec();
    let mut tokens_saved = 0usize;

    // Find tool result indices eligible for compaction
    let compactable_tools = ["os", "system", "web", "file", "shell"];
    let mut tool_result_indices: Vec<(usize, usize, String)> = Vec::new(); // (index, age_from_end, tool_name)

    for (i, msg) in result.iter().enumerate() {
        if msg.role != "tool" && msg.role != "assistant" {
            continue;
        }

        // Check if this message has tool results
        if let Some(ref tr_json) = msg.tool_results {
            if tr_json.is_empty() || tr_json == "[]" || tr_json == "null" {
                continue;
            }

            // Find the tool name from tool_calls in the same or preceding assistant message
            let tool_name = find_tool_name_for_result(messages, i);
            if compactable_tools.contains(&tool_name.as_str()) {
                let age = messages.len().saturating_sub(i);
                tool_result_indices.push((i, age, tool_name));
            }
        }
    }

    // Sort by trim priority then age (oldest first)
    tool_result_indices.sort_by(|a, b| {
        let pa = trim_priority(&a.2);
        let pb = trim_priority(&b.2);
        pa.cmp(&pb).then(b.1.cmp(&a.1)) // higher priority first, then oldest first
    });

    // Protect most recent N results
    let protect_count = std::cmp::min(MICRO_COMPACT_KEEP_RECENT, tool_result_indices.len());
    let candidates = if tool_result_indices.len() > protect_count {
        &tool_result_indices[..tool_result_indices.len() - protect_count]
    } else {
        return (result, 0);
    };

    // Only compact if above threshold or proactively if old enough
    let min_age = if total_tokens < warning_threshold { 6 } else { 3 };

    for (idx, age, tool_name) in candidates {
        if *age < min_age {
            continue;
        }

        let msg = &result[*idx];
        let old_tokens = estimate_message_tokens(msg);
        if old_tokens < 100 {
            continue; // Not worth compacting small results
        }

        let trimmed_content = format!("[trimmed: {} result]", tool_name);

        // Preserve original tool_call_ids so the orphan filter in build_messages
        // can still match compacted results with their corresponding tool_calls.
        let compacted_results = if let Some(ref tr_json) = msg.tool_results {
            if let Ok(results) = serde_json::from_str::<Vec<serde_json::Value>>(tr_json) {
                let preserved: Vec<serde_json::Value> = results
                    .iter()
                    .map(|r| {
                        let original_id = r
                            .get("tool_call_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        serde_json::json!({
                            "tool_call_id": original_id,
                            "content": trimmed_content,
                            "is_error": false
                        })
                    })
                    .collect();
                serde_json::to_string(&preserved).ok()
            } else {
                Some(serde_json::json!([{
                    "tool_call_id": "",
                    "content": trimmed_content,
                    "is_error": false
                }]).to_string())
            }
        } else {
            None
        };

        result[*idx] = ChatMessage {
            id: msg.id.clone(),
            chat_id: msg.chat_id.clone(),
            role: msg.role.clone(),
            content: trimmed_content.clone(),
            metadata: msg.metadata.clone(),
            created_at: msg.created_at,
            day_marker: msg.day_marker.clone(),
            tool_calls: msg.tool_calls.clone(),
            tool_results: compacted_results,
            token_estimate: Some(10),
        };
        tokens_saved += old_tokens.saturating_sub(10);
    }

    if tokens_saved < MICRO_COMPACT_MIN_SAVINGS {
        return (messages.to_vec(), 0); // Not worth it
    }

    (result, tokens_saved)
}

/// Determine trimming order for tool types.
fn trim_priority(tool_name: &str) -> usize {
    match tool_name {
        "web" => 0,    // Stale fastest
        "file" => 1,   // Largest output
        "shell" => 2,  // Often large
        "os" | "system" => 2, // Same as shell
        _ => 3,
    }
}

/// Find the tool name for a tool result message by looking at preceding tool calls.
fn find_tool_name_for_result(messages: &[ChatMessage], result_idx: usize) -> String {
    // Look backwards for an assistant message with tool_calls
    for i in (0..result_idx).rev() {
        let msg = &messages[i];
        if msg.role == "assistant" {
            if let Some(ref tc_json) = msg.tool_calls {
                if let Ok(calls) = serde_json::from_str::<Vec<serde_json::Value>>(tc_json) {
                    if let Some(first) = calls.first() {
                        return first
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                    }
                }
            }
            break; // Stop at first assistant message
        }
    }
    "unknown".to_string()
}

/// Build a quick plaintext fallback summary for first eviction (no LLM call).
pub fn build_quick_fallback_summary(messages: &[ChatMessage], active_objective: &str) -> String {
    let mut parts = Vec::new();

    if !active_objective.is_empty() {
        parts.push(format!("Active objective: {}", active_objective));
    }

    // Extract user requests
    let mut user_requests = Vec::new();
    for msg in messages {
        if msg.role == "user" && !msg.content.is_empty() {
            let truncated = if msg.content.len() > 200 {
                format!("{}...", &msg.content[..200])
            } else {
                msg.content.clone()
            };
            user_requests.push(truncated);
        }
    }

    if !user_requests.is_empty() {
        parts.push(format!("User requests: {}", user_requests.join("; ")));
    }

    // Extract tool call names
    let mut tool_names = Vec::new();
    for msg in messages {
        if msg.role == "assistant" {
            if let Some(ref tc_json) = msg.tool_calls {
                if let Ok(calls) = serde_json::from_str::<Vec<serde_json::Value>>(tc_json) {
                    for call in &calls {
                        if let Some(name) = call.get("name").and_then(|v| v.as_str()) {
                            if !tool_names.contains(&name.to_string()) {
                                tool_names.push(name.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    if !tool_names.is_empty() {
        parts.push(format!("Tools used: {}", tool_names.join(", ")));
    }

    parts.join("\n")
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
        }
    }

    #[test]
    fn test_estimate_tokens() {
        let msg = make_msg("user", "hello world"); // 11 chars -> 2 tokens
        assert_eq!(estimate_message_tokens(&msg), 2);
    }

    #[test]
    fn test_sliding_window_small() {
        let messages = vec![make_msg("user", "hello"), make_msg("assistant", "hi")];
        let (window, evicted) = apply_sliding_window(&messages, 0);
        assert_eq!(window.len(), 2);
        assert!(evicted.is_empty());
    }

    #[test]
    fn test_context_thresholds() {
        let t = ContextThresholds::from_context_window(200_000, 10_000);
        assert!(t.warning < t.error);
        assert!(t.error < t.auto_compact);
    }
}
