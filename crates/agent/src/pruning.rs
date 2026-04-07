use ai::{ChatRequest, Message, Provider, StreamEventType};
use db::models::ChatMessage;
use tracing::debug;

/// Approximate chars per token.
const CHARS_PER_TOKEN: usize = 4;
/// Chars estimate for a base64 image.
const IMAGE_CHAR_ESTIMATE: usize = 8000;
/// Minimum token savings to bother micro-compacting.
const MICRO_COMPACT_MIN_SAVINGS: usize = 1000;
/// Protect the N most recent tool results from micro-compaction.
const MICRO_COMPACT_KEEP_RECENT: usize = 3;
/// When compactable tool results exceed this count, strip aggressively
/// regardless of age (keep only MICRO_COMPACT_KEEP_RECENT most recent).
const MICRO_COMPACT_COUNT_TRIGGER: usize = 4;

/// Inactivity gap (seconds) before time-based micro-compaction fires.
/// Matches typical provider cache TTL — if cache is cold, no point re-processing
/// stale tool results at full input cost.
pub const TIME_BASED_GAP_THRESHOLD_SECS: i64 = 300; // 5 minutes
/// How many recent tool results to keep during time-based clearing.
/// Claude Code keeps 1 — we match that.
pub const TIME_BASED_KEEP_RECENT: usize = 1;

/// Default sliding window token limit (used when caller doesn't supply one).
pub const DEFAULT_WINDOW_MAX_TOKENS: usize = 40_000;

/// Hard cap on message count regardless of token budget.
/// Even short messages add serialization/attention overhead at the provider.
/// 80 messages × ~120 tokens/msg ≈ 9,600 tokens — well within budget.
const MAX_MESSAGE_COUNT: usize = 80;

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
/// `max_tokens` controls the token budget for the window — caller typically
/// passes `ContextThresholds::auto_compact` so eviction only fires when
/// approaching the context limit (like Claude Code's ~83% threshold).
pub fn apply_sliding_window(
    messages: &[ChatMessage],
    run_start_time: i64,
    max_tokens: usize,
) -> (Vec<ChatMessage>, Vec<ChatMessage>) {
    // Early-return: if total tokens fit within budget AND message count is under
    // the cap, no eviction needed. This short-circuits the vast majority of turns.
    let total = estimate_total_tokens(messages);
    if total <= max_tokens && messages.len() <= MAX_MESSAGE_COUNT {
        return (messages.to_vec(), vec![]);
    }

    // Walk backwards from end, accumulating tokens and counting messages
    let mut window_start = messages.len();
    let mut accumulated_tokens = 0usize;
    let mut kept_count = 0usize;

    for i in (0..messages.len()).rev() {
        let msg = &messages[i];

        // Never evict current-run messages
        if msg.created_at >= run_start_time {
            let tokens = estimate_message_tokens(msg);
            accumulated_tokens += tokens;
            kept_count += 1;
            window_start = i;
            continue;
        }

        let tokens = estimate_message_tokens(msg);
        if accumulated_tokens + tokens > max_tokens || kept_count >= MAX_MESSAGE_COUNT {
            break;
        }

        accumulated_tokens += tokens;
        kept_count += 1;
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

    // Find tool result indices eligible for compaction.
    // ALL tool results are compactable — the keep-recent protection prevents
    // stripping results the model still needs.
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

            let tool_name = find_tool_name_for_result(messages, i);
            let age = messages.len().saturating_sub(i);
            tool_result_indices.push((i, age, tool_name));
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

    // Count-based trigger: when compactable results exceed threshold,
    // strip aggressively regardless of age (Claude Code style).
    let count_triggered = tool_result_indices.len() > MICRO_COMPACT_COUNT_TRIGGER;
    // Age-based floor for the non-triggered path (backward compat).
    let min_age = if count_triggered { 0 } else if total_tokens < warning_threshold { 6 } else { 3 };

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

/// Time-based micro-compaction: clear stale tool results when user returns
/// after inactivity. If the gap since the last assistant message exceeds
/// `gap_threshold_secs`, replace all but the `keep_recent` most recent tool
/// results with `[cleared]`. Preserves tool_call_ids for orphan filtering.
///
/// Rationale: provider prompt caches expire after ~5 minutes. If the user
/// has been away longer than that, the entire context will be re-processed
/// at full input cost. Clearing stale tool results prevents paying to
/// re-tokenize results the model already processed in a prior turn.
pub fn time_based_micro_compact(
    messages: &[ChatMessage],
    keep_recent: usize,
    gap_threshold_secs: i64,
) -> (Vec<ChatMessage>, usize) {
    // Find the last assistant message timestamp
    let last_assistant_ts = messages
        .iter()
        .rev()
        .find(|m| m.role == "assistant")
        .map(|m| m.created_at)
        .unwrap_or(0);

    if last_assistant_ts == 0 {
        return (messages.to_vec(), 0); // no assistant messages → nothing to clear
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let gap = now - last_assistant_ts;
    if gap < gap_threshold_secs {
        return (messages.to_vec(), 0); // active session — don't touch
    }

    // Collect indices of tool result messages (walking backwards for recency)
    let mut tool_indices: Vec<usize> = Vec::new();
    for (i, msg) in messages.iter().enumerate().rev() {
        if msg.role == "tool" || (msg.tool_results.is_some()
            && msg.tool_results.as_ref().is_some_and(|tr| !tr.is_empty() && tr != "[]" && tr != "null"))
        {
            tool_indices.push(i);
        }
    }

    if tool_indices.len() <= keep_recent {
        return (messages.to_vec(), 0); // not enough to clear
    }

    let mut result = messages.to_vec();
    let mut tokens_saved = 0usize;

    // tool_indices is newest-first; skip the first `keep_recent` entries
    for &idx in &tool_indices[keep_recent..] {
        let msg = &result[idx];
        let old_tokens = estimate_message_tokens(msg);
        if old_tokens < 10 {
            continue; // already small
        }

        let cleared = "[cleared]".to_string();

        // Preserve tool_call_ids in tool_results JSON
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
                            "content": cleared,
                            "is_error": false
                        })
                    })
                    .collect();
                serde_json::to_string(&preserved).ok()
            } else {
                Some(serde_json::json!([{
                    "tool_call_id": "",
                    "content": cleared,
                    "is_error": false
                }]).to_string())
            }
        } else {
            None
        };

        result[idx] = ChatMessage {
            id: msg.id.clone(),
            chat_id: msg.chat_id.clone(),
            role: msg.role.clone(),
            content: cleared.clone(),
            metadata: msg.metadata.clone(),
            created_at: msg.created_at,
            day_marker: msg.day_marker.clone(),
            tool_calls: msg.tool_calls.clone(),
            tool_results: compacted_results,
            token_estimate: Some(2),
        };
        tokens_saved += old_tokens.saturating_sub(2);
    }

    debug!(
        gap_secs = gap,
        tool_results_cleared = tool_indices.len().saturating_sub(keep_recent),
        tokens_saved = tokens_saved,
        "Time-based micro-compact fired (stale session)"
    );

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
                format!("{}...", crate::runner::truncate_str(&msg.content, 200))
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

/// Max tokens for compaction summary output.
const COMPACTION_MAX_TOKENS: i32 = 2000;
/// Max chars of evicted content to feed to the compaction model.
const COMPACTION_CONTENT_CAP: usize = 80_000;

/// Build a structured LLM summary of evicted messages.
///
/// Uses the sidecar pattern (isolated ChatRequest, no session/DB writes).
/// Falls back to `build_quick_fallback_summary()` on any error.
pub async fn build_llm_summary(
    provider: &dyn Provider,
    evicted: &[ChatMessage],
    existing_summary: &str,
    active_task: &str,
    model: &str,
) -> Result<String, String> {
    // Serialize evicted messages into a compact transcript
    let mut transcript = String::new();
    for msg in evicted {
        let role = msg.role.as_str();
        if !msg.content.is_empty() {
            transcript.push_str(&format!("[{}]: {}\n", role, msg.content));
        }
        if let Some(ref tc) = msg.tool_calls {
            if !tc.is_empty() && tc != "[]" && tc != "null" {
                transcript.push_str(&format!("[{} tool_calls]: {}\n", role, tc));
            }
        }
        if let Some(ref tr) = msg.tool_results {
            if !tr.is_empty() && tr != "[]" && tr != "null" {
                // Truncate individual tool results in the transcript
                let tr_display = if tr.len() > 500 {
                    format!("{}...(truncated)", crate::runner::truncate_str(tr, 500))
                } else {
                    tr.clone()
                };
                transcript.push_str(&format!("[{} tool_result]: {}\n", role, tr_display));
            }
        }
    }

    // Cap total transcript fed to model
    if transcript.len() > COMPACTION_CONTENT_CAP {
        transcript.truncate(COMPACTION_CONTENT_CAP);
    }

    let mut user_content = String::new();
    if !existing_summary.is_empty() {
        user_content.push_str(&format!(
            "## Existing Summary (merge with new context)\n{}\n\n",
            existing_summary
        ));
    }
    if !active_task.is_empty() {
        user_content.push_str(&format!("## Active Objective\n{}\n\n", active_task));
    }
    user_content.push_str(&format!("## Conversation Transcript to Summarize\n{}", transcript));

    let system = "\
You are a conversation compaction engine. Produce a structured summary of the conversation transcript below. \
If an existing summary is provided, merge it with the new information into one coherent summary. \
Be concise but preserve critical context.

Output format (use these exact headings):

## Goal
One sentence: what is the user trying to accomplish?

## Key Decisions
Bullet list of decisions made and approaches chosen.

## Files & Resources
Full paths of files read, written, or referenced.

## Errors & Resolutions
Any errors encountered and how they were resolved (or if still open).

## Current State
What has been completed and what remains.

## User Requests
Verbatim key requests from the user (quoted, max 3).

## Pending Items
Anything incomplete or blocked.";

    let req = ChatRequest {
        messages: vec![Message {
            role: "user".to_string(),
            content: user_content,
            ..Default::default()
        }],
        tools: vec![],
        max_tokens: COMPACTION_MAX_TOKENS,
        temperature: 0.0,
        system: system.to_string(),
        static_system: String::new(),
        model: model.to_string(),
        enable_thinking: false,
        metadata: None,
        cache_breakpoints: vec![],
        cancel_token: None,
    };

    let mut rx = provider
        .stream(&req)
        .await
        .map_err(|e| format!("compaction stream: {e}"))?;

    let mut text = String::new();
    while let Some(event) = rx.recv().await {
        match event.event_type {
            StreamEventType::Text => text.push_str(&event.text),
            StreamEventType::Done | StreamEventType::Error => break,
            _ => {}
        }
    }

    if text.is_empty() {
        Err("compaction: empty response from provider".into())
    } else {
        debug!(summary_len = text.len(), "LLM compaction summary generated");
        Ok(text)
    }
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
        let (window, evicted) = apply_sliding_window(&messages, 0, DEFAULT_WINDOW_MAX_TOKENS);
        assert_eq!(window.len(), 2);
        assert!(evicted.is_empty());
    }

    #[test]
    fn test_sliding_window_token_eviction() {
        // Each message ~2500 chars = ~625 tokens. 5 messages = ~3125 tokens.
        let big = "x".repeat(2500);
        let messages: Vec<ChatMessage> = (0..5)
            .map(|i| {
                let role = if i % 2 == 0 { "user" } else { "assistant" };
                make_old_msg(role, &big)
            })
            .collect();
        // With a 2000-token budget, should evict some messages
        // run_start_time in the future so none are protected as "current run"
        let (window, evicted) = apply_sliding_window(&messages, 999_999, 2000);
        assert!(!evicted.is_empty(), "should evict when over token budget");
        assert!(window.len() < messages.len());
    }

    #[test]
    fn test_sliding_window_high_threshold_no_eviction() {
        // Same messages but with a high threshold — should keep everything
        let big = "x".repeat(2500);
        let messages: Vec<ChatMessage> = (0..5)
            .map(|i| {
                let role = if i % 2 == 0 { "user" } else { "assistant" };
                make_msg(role, &big)
            })
            .collect();
        let (window, evicted) = apply_sliding_window(&messages, 0, 100_000);
        assert!(evicted.is_empty(), "high threshold should keep everything");
        assert_eq!(window.len(), 5);
    }

    fn make_old_msg(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            id: uuid::Uuid::new_v4().to_string(),
            chat_id: "test".to_string(),
            role: role.to_string(),
            content: content.to_string(),
            metadata: None,
            created_at: 1000, // in the past
            day_marker: None,
            tool_calls: None,
            tool_results: None,
            token_estimate: None,
        }
    }

    #[test]
    fn test_sliding_window_message_count_cap() {
        // 200 short messages (~1 token each) — well within token budget but exceeds count cap
        let messages: Vec<ChatMessage> = (0..200)
            .map(|i| {
                let role = if i % 2 == 0 { "user" } else { "assistant" };
                make_old_msg(role, "ok")
            })
            .collect();
        // run_start_time far in the future so none are "current run" protected
        let (window, evicted) = apply_sliding_window(&messages, 999_999, 100_000);
        assert!(
            window.len() <= MAX_MESSAGE_COUNT,
            "window should be capped at {} messages, got {}",
            MAX_MESSAGE_COUNT,
            window.len()
        );
        assert!(!evicted.is_empty(), "should evict excess messages");
    }

    #[test]
    fn test_context_thresholds() {
        let t = ContextThresholds::from_context_window(200_000, 10_000);
        assert!(t.warning < t.error);
        assert!(t.error < t.auto_compact);
    }

    fn make_tool_result_msg(content: &str, created_at: i64) -> ChatMessage {
        let tool_call_id = uuid::Uuid::new_v4().to_string();
        ChatMessage {
            id: uuid::Uuid::new_v4().to_string(),
            chat_id: "test".to_string(),
            role: "tool".to_string(),
            content: content.to_string(),
            metadata: None,
            created_at,
            day_marker: None,
            tool_calls: None,
            tool_results: Some(serde_json::json!([{
                "tool_call_id": tool_call_id,
                "content": content,
                "is_error": false
            }]).to_string()),
            token_estimate: None,
        }
    }

    fn make_assistant_msg(content: &str, created_at: i64) -> ChatMessage {
        ChatMessage {
            id: uuid::Uuid::new_v4().to_string(),
            chat_id: "test".to_string(),
            role: "assistant".to_string(),
            content: content.to_string(),
            metadata: None,
            created_at,
            day_marker: None,
            tool_calls: None,
            tool_results: None,
            token_estimate: None,
        }
    }

    #[test]
    fn test_time_based_micro_compact_stale_session() {
        let old_ts = 1000; // way in the past
        let big_result = "x".repeat(4000); // ~1000 tokens
        let messages = vec![
            make_msg("user", "hello"),
            make_assistant_msg("let me search", old_ts),
            make_tool_result_msg(&big_result, old_ts),
            make_assistant_msg("found something", old_ts),
            make_tool_result_msg(&big_result, old_ts),
            make_assistant_msg("here's the answer", old_ts),
            make_tool_result_msg(&big_result, old_ts), // most recent tool result
        ];

        // gap_threshold of 1 second — all messages are old, so gap is huge
        let (result, tokens_saved) = time_based_micro_compact(&messages, 1, 1);
        assert!(tokens_saved > 0, "should save tokens on stale session");

        // Only the most recent tool result (index 6) should keep its content
        // The older two (indices 2, 4) should be cleared
        let tool_results: Vec<&ChatMessage> = result.iter()
            .filter(|m| m.role == "tool")
            .collect();
        assert_eq!(tool_results.len(), 3);

        // Most recent keeps content
        assert!(!tool_results[2].content.contains("[cleared]"),
            "most recent tool result should keep content");
        // Older ones cleared
        assert_eq!(tool_results[0].content, "[cleared]");
        assert_eq!(tool_results[1].content, "[cleared]");
    }

    #[test]
    fn test_time_based_micro_compact_active_session() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let big_result = "x".repeat(4000);
        let messages = vec![
            make_msg("user", "hello"),
            make_assistant_msg("response", now - 10), // 10 seconds ago
            make_tool_result_msg(&big_result, now - 10),
        ];

        // gap_threshold of 300 seconds — session is active (10s ago)
        let (_, tokens_saved) = time_based_micro_compact(&messages, 1, 300);
        assert_eq!(tokens_saved, 0, "active session should not be compacted");
    }

    #[test]
    fn test_micro_compact_universal_tools() {
        // Tool results from non-standard tools (e.g. "search_emails") should
        // now be compactable since we removed the category filter.
        let big = "x".repeat(4000);
        let mut messages = Vec::new();
        // Create 6 tool results with a custom tool name — exceeds count trigger (4)
        for i in 0..6 {
            let mut assistant = make_old_msg("assistant", "calling tool");
            assistant.tool_calls = Some(serde_json::json!([{
                "name": "search_emails",
                "id": format!("call_{}", i),
                "input": {}
            }]).to_string());
            messages.push(assistant);
            messages.push(make_tool_result_msg(&big, 1000));
        }

        let (result, tokens_saved) = micro_compact(&messages, 100_000);
        assert!(tokens_saved > 0,
            "non-standard tool results should be compactable (universal filter)");

        // Should keep 3 most recent, compact older 3
        let compacted_count = result.iter()
            .filter(|m| m.content.contains("[trimmed:"))
            .count();
        assert!(compacted_count >= 2, "should compact at least 2 old results, got {}", compacted_count);
    }
}
