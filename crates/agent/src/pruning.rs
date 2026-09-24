use ai::{ChatRequest, Message, Provider, StreamEventType};
use db::models::ChatMessage;
use tracing::debug;

/// Chars estimate for a base64 image.
pub(crate) const IMAGE_CHAR_ESTIMATE: usize = 8000;

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

    /// Tighten thresholds by the run's observed estimate undercount
    /// (API-reported usage vs local chars/4 estimate). Never loosens —
    /// an overcounting estimate just means compaction fires early.
    pub fn adjusted(&self, undercount: usize) -> Self {
        Self {
            warning: self.warning.saturating_sub(undercount),
            error: self.error.saturating_sub(undercount),
            auto_compact: self.auto_compact.saturating_sub(undercount),
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
    chars / crate::CHARS_PER_TOKEN
}

/// Estimate total tokens for all messages.
pub fn estimate_total_tokens(messages: &[ChatMessage]) -> usize {
    messages.iter().map(estimate_message_tokens).sum()
}

/// Apply sliding window: returns (window_messages, evicted_messages).
/// Never evicts messages with created_at >= run_start_time.
/// `max_tokens` controls the token budget for the window — caller typically
/// passes `ContextThresholds::auto_compact` so eviction only fires when
/// approaching the context limit (the standard ~83%-of-limit threshold).
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

    // Guard: if the loop never assigned window_start (e.g. budget was already
    // exceeded before any message was kept), clamp to last message so we
    // don't index out of bounds.
    if window_start >= messages.len() {
        window_start = messages.len().saturating_sub(1);
    }

    // Fix tool-pair boundaries: don't split tool_use from tool_result
    while window_start > 0 {
        let msg = &messages[window_start];
        // If first message is a tool result, include preceding assistant message
        if msg.role == "tool"
            || (msg.tool_results.is_some()
                && msg
                    .tool_results
                    .as_ref()
                    .is_some_and(|tr| !tr.is_empty() && tr != "[]"))
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

/// Message summarization: truncate old user/assistant messages to reduce context
/// without eviction. Keeps recent `keep_recent` messages intact; truncates older
/// user/assistant text to first sentence(s). No LLM — pure heuristic.
pub fn message_summarize(
    messages: &[ChatMessage],
    warning_threshold: usize,
    keep_recent: usize,
) -> (Vec<ChatMessage>, usize) {
    let total_tokens = estimate_total_tokens(messages);
    if total_tokens <= warning_threshold || messages.len() <= keep_recent {
        return (messages.to_vec(), 0);
    }

    let mut result = messages.to_vec();
    let mut tokens_saved = 0usize;
    let cutoff = messages.len().saturating_sub(keep_recent);

    for i in 0..cutoff {
        let msg = &result[i];

        // Only truncate user and assistant prose — skip tool/system messages
        if msg.role != "user" && msg.role != "assistant" {
            continue;
        }

        // Skip already-summarized messages
        if msg.content.starts_with("[summarized]") || msg.content.starts_with("[cleared]") {
            continue;
        }

        let (max_chars, max_sentences) = if msg.role == "user" {
            (200usize, 1usize)
        } else {
            (500, 2)
        };

        if msg.content.len() <= max_chars {
            continue;
        }

        let old_tokens = estimate_message_tokens(msg);
        let truncated = truncate_to_sentences(&msg.content, max_sentences, max_chars);
        let new_content = format!("[summarized] {}", truncated);

        result[i] = ChatMessage {
            id: msg.id.clone(),
            chat_id: msg.chat_id.clone(),
            role: msg.role.clone(),
            content: new_content,
            metadata: msg.metadata.clone(),
            created_at: msg.created_at,
            day_marker: msg.day_marker.clone(),
            tool_calls: msg.tool_calls.clone(),
            tool_results: msg.tool_results.clone(),
            token_estimate: None,
            html: None,
        };
        let new_tokens = estimate_message_tokens(&result[i]);
        tokens_saved += old_tokens.saturating_sub(new_tokens);
    }

    (result, tokens_saved)
}

/// Truncate text to at most `max_sentences` sentences, with a hard char cap.
fn truncate_to_sentences(text: &str, max_sentences: usize, max_chars: usize) -> String {
    let mut end = 0usize;
    let mut sentences = 0usize;

    // Walk through text finding sentence boundaries (. or \n after 20+ chars)
    for (i, ch) in text.char_indices() {
        if i >= max_chars {
            break;
        }
        if (ch == '.' || ch == '\n') && i >= 20 {
            end = i + 1;
            sentences += 1;
            if sentences >= max_sentences {
                break;
            }
        }
    }

    if end == 0 || end < 20 {
        // No sentence boundary found — hard truncate at max_chars
        let truncated: String = text.chars().take(max_chars).collect();
        format!("{}...", truncated)
    } else {
        format!("{}...", &text[..end].trim())
    }
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

/// Max tokens for compaction summary output. Generous so a structured summary
/// never cuts off mid-section (matches the provider non-thinking default cap).
const COMPACTION_MAX_TOKENS: i32 = 8192;
/// Max chars of evicted content to feed to the compaction model.
const COMPACTION_CONTENT_CAP: usize = 80_000;

/// Prefix marking a compaction checkpoint stored as a chat message (manual
/// compact replaces the conversation with one such message). Used to detect a
/// prior summary inside history being compacted so it is folded, never reset.
pub const COMPACTION_MESSAGE_MARKER: &str = "**Conversation Summary**";

/// Build a structured LLM summary of evicted messages.
///
/// Uses the sidecar pattern (isolated ChatRequest, no session/DB writes).
/// Falls back to `build_quick_fallback_summary()` on any error.
pub async fn build_llm_summary(
    trace: ai::RequestTrace,
    provider: &dyn Provider,
    evicted: &[ChatMessage],
    existing_summary: &str,
    active_task: &str,
    model: &str,
) -> Result<String, String> {
    // Prior checkpoints to fold into the new summary: the rolling session
    // summary plus any compaction checkpoint message found in the evicted
    // history (manual compact stores its output as a marked assistant message).
    let mut snapshots: Vec<String> = Vec::new();
    if !existing_summary.is_empty() {
        snapshots.push(existing_summary.to_string());
    }

    // Serialize evicted messages into a compact transcript
    let mut transcript = String::new();
    for msg in evicted {
        if msg.role == "assistant" && msg.content.starts_with(COMPACTION_MESSAGE_MARKER) {
            snapshots.push(msg.content.clone());
            continue;
        }
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
    if !snapshots.is_empty() {
        user_content.push_str(&format!(
            "## Previous Summary Snapshot\n{}\n\n",
            snapshots.join("\n\n")
        ));
    }
    if !active_task.is_empty() {
        user_content.push_str(&format!("## Active Objective\n{}\n\n", active_task));
    }
    user_content.push_str(&format!(
        "## Conversation Transcript to Summarize\n{}",
        transcript
    ));

    let system = "\
You are a conversation compaction engine. Produce a structured checkpoint of an ONGOING \
conversation so the next model can continue it mid-stream.

Compounding: if a \"## Previous Summary Snapshot\" is provided, FOLD it into your output — \
take the union of its facts and the new transcript, dedupe, and update anything the \
transcript supersedes — a goal the snapshot called IN PROGRESS that the transcript shows \
delivered becomes DONE. NEVER reset, drop, or restart the summary; the snapshot is earlier \
state of the same conversation.

Output ONLY the sections below, in this order, with these exact headings. SKIP any section \
that would be empty — never write \"None\".

## Goal
The user's most recent request, in the user's own words, with its STATUS on the first line: \
IN PROGRESS, DONE, or SUPERSEDED. The goal comes ONLY from user messages — the assistant's \
tool activity never defines it, and the same tool call repeated with the same arguments is a \
stall, not progress: never describe it as the current step or as work toward the goal. DONE \
when the transcript shows the request delivered or answered; SUPERSEDED when the user has \
since asked for something else (then that newer request is the goal). Only an IN PROGRESS \
goal is continued by the next model; never present a finished request as ongoing.

## Constraints & Preferences
Rules, limitations, and preferences the user stated.

## Completed Actions
Bullet list of actions taken and their outcomes (tools called, files modified, commands run).

## Active State
What is in progress RIGHT NOW: current step, partial results, and the immediate next action.

## Blocked
Items that cannot proceed and exactly why. Omit resolved errors and transient failures \
(timeouts, 404s, connection drops) — these are normal and must not influence future tool use.

## Key Decisions
Decisions made and their rationale. Critical for not re-deciding.

## Relevant Files/Artifacts
Full paths of files read, written, or modified; URLs, IDs, endpoints, versions, and other \
specific values needed to resume.

## Last Dropped Turns
One line per turn in the transcript being evicted, oldest to newest: role and gist. Only \
turns from this transcript — do not carry over dropped turns from the snapshot.";

    let req = ChatRequest {
        tool_credential: None,
        tool_choice: Default::default(),
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
        trace,
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
            html: None,
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
            html: None,
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

    #[test]
    fn test_message_summarize_truncates_old() {
        let long_user = "This is a long user message that goes on and on. ".repeat(20);
        let long_assistant = "Here is a detailed response with lots of information. ".repeat(30);
        let mut messages = Vec::new();

        // 20 old messages (10 user + 10 assistant)
        for i in 0..10 {
            let mut u = make_old_msg("user", &long_user);
            u.created_at = 1000 + i;
            messages.push(u);
            let mut a = make_old_msg("assistant", &long_assistant);
            a.created_at = 1000 + i;
            messages.push(a);
        }
        // 5 recent messages (within keep_recent=15)
        for i in 0..5 {
            let mut u = make_old_msg("user", &long_user);
            u.created_at = 2000 + i;
            messages.push(u);
        }

        // warning_threshold = 0 to force activation
        let (result, tokens_saved) = message_summarize(&messages, 0, 15);
        assert!(tokens_saved > 0, "should save tokens");

        // Check that old messages got summarized
        let summarized_count = result
            .iter()
            .filter(|m| m.content.starts_with("[summarized]"))
            .count();
        assert!(
            summarized_count > 0,
            "should have summarized some old messages"
        );

        // Check that recent messages (last 15) are untouched
        for i in (result.len() - 5)..result.len() {
            assert!(
                !result[i].content.starts_with("[summarized]"),
                "recent messages should not be summarized"
            );
        }
    }

    #[test]
    fn test_message_summarize_skips_short() {
        let messages = vec![
            make_old_msg("user", "hi"),
            make_old_msg("assistant", "hello"),
            make_old_msg("user", "how are you?"),
        ];
        // warning_threshold = 0 to force activation, keep_recent = 1
        let (_, tokens_saved) = message_summarize(&messages, 0, 1);
        assert_eq!(tokens_saved, 0, "short messages should not be summarized");
    }

    #[test]
    fn test_truncate_to_sentences() {
        // Sentences must be > 20 chars for the boundary to be recognized
        let text = "This is the first long sentence that matters. Here is the second sentence. And a third.";
        let result = truncate_to_sentences(text, 1, 200);
        assert!(result.contains("first long sentence"));
        assert!(result.ends_with("..."));
        assert!(!result.contains("second sentence"));
    }
}
