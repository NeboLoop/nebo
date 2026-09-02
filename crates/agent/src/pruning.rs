use ai::{ChatRequest, Message, Provider, StreamEventType};
use db::models::ChatMessage;
use tracing::debug;

/// Chars estimate for a base64 image.
const IMAGE_CHAR_ESTIMATE: usize = 8000;
/// Minimum token savings to bother micro-compacting.
const MICRO_COMPACT_MIN_SAVINGS: usize = 1000;
/// Protect the N most recent tool results from micro-compaction. Keeping only 3
/// stripped content the model was still actively working with, so mid-run reads
/// it had just done looked "empty" once compacted. 5 leaves enough live context
/// to reason over.
const MICRO_COMPACT_KEEP_RECENT: usize = 5;
/// When compactable tool results exceed this count, strip aggressively
/// regardless of age (keep only MICRO_COMPACT_KEEP_RECENT most recent).
const MICRO_COMPACT_COUNT_TRIGGER: usize = 4;

/// Inactivity gap (seconds) before time-based micro-compaction fires.
/// Matches typical provider cache TTL — if cache is cold, no point re-processing
/// stale tool results at full input cost.
pub const TIME_BASED_GAP_THRESHOLD_SECS: i64 = 300; // 5 minutes
/// How many recent tool results to keep during time-based clearing.
/// Keep the single most recent so the model retains immediate context.
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

/// Micro-compact: trim old tool results to reduce context size.
/// Returns modified messages and tokens saved.

/// The text a tool-result row actually carries. In production tool rows keep
/// `content` EMPTY and the payload in `tool_results[].content`; rendering
/// from `msg.content` produced empty "bounded slices" and `[os] 0 lines`
/// summaries, rewriting every earlier file read in the model's history as
/// "nothing came back" — which the model then believed about fresh reads too
/// (2026-09-01: "the file appears empty" on files that read fine).
fn tool_result_text(msg: &ChatMessage) -> String {
    if let Some(tr) = msg.tool_results.as_deref() {
        if let Ok(results) = serde_json::from_str::<Vec<serde_json::Value>>(tr) {
            let joined: Vec<&str> = results
                .iter()
                .filter_map(|r| r.get("content").and_then(|c| c.as_str()))
                .filter(|c| !c.is_empty())
                .collect();
            if !joined.is_empty() {
                return joined.join("\n");
            }
        }
    }
    msg.content.clone()
}

/// First tool_call_id of a result message — the freeze key. Results without
/// an id are replaced but never frozen (no stable identity to freeze on).
fn first_tool_call_id(msg: &ChatMessage) -> Option<String> {
    let tr = msg.tool_results.as_ref()?;
    let parsed: Vec<serde_json::Value> = serde_json::from_str(tr).ok()?;
    let id = parsed.first()?.get("tool_call_id")?.as_str()?;
    (!id.is_empty()).then(|| id.to_string())
}

pub fn micro_compact(
    messages: &[ChatMessage],
    warning_threshold: usize,
    frozen: &mut std::collections::HashMap<String, String>,
) -> (Vec<ChatMessage>, usize) {
    let total_tokens = estimate_total_tokens(messages);
    // Below the warning threshold the context fits comfortably — touch nothing.
    // Stripping results the model is actively working with mid-run makes it
    // "start over": it re-announces, re-reads, and spawns agents to recover
    // instructions it just loaded. Compaction is a pressure valve, not a
    // routine pass — it fires only when the context is actually near its limit.
    if total_tokens < warning_threshold {
        return (messages.to_vec(), 0);
    }
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
    // strip aggressively regardless of age.
    let count_triggered = tool_result_indices.len() > MICRO_COMPACT_COUNT_TRIGGER;
    // Age-based floor for the non-triggered path (backward compat).
    let min_age = if count_triggered {
        0
    } else if total_tokens < warning_threshold {
        6
    } else {
        3
    };

    for (idx, age, tool_name) in candidates {
        if *age < min_age {
            continue;
        }

        let msg = &result[*idx];
        let old_tokens = estimate_message_tokens(msg);
        if old_tokens < 100 {
            continue; // Not worth compacting small results
        }

        // Build informative summary instead of generic "[trimmed: X result]".
        // FROZEN DECISION: the first rendering ever chosen for a tool_use_id
        // is the rendering forever (per run). Re-deciding each iteration is
        // how a result rendered fine on pass N became "[os] 0 lines" on pass
        // N+1 — the model must never watch its own history mutate.
        let (_call_name, call_input) = find_tool_call_for_result(messages, *idx);
        let freeze_key = first_tool_call_id(msg);
        let trimmed_content = freeze_key
            .as_ref()
            .and_then(|k| frozen.get(k).cloned())
            .unwrap_or_else(|| build_tool_summary(tool_name, call_input.as_ref(), &tool_result_text(msg)));
        if let Some(k) = freeze_key {
            frozen.entry(k).or_insert_with(|| trimmed_content.clone());
        }

        // Preserve original tool_call_ids so the orphan filter in build_messages
        // can still match compacted results with their corresponding tool_calls.
        let compacted_results = if let Some(ref tr_json) = msg.tool_results {
            if let Ok(results) = serde_json::from_str::<Vec<serde_json::Value>>(tr_json) {
                let preserved: Vec<serde_json::Value> = results
                    .iter()
                    .map(|r| {
                        let original_id =
                            r.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or("");
                        // A compacted failure must still read as a failure —
                        // hardcoding `false` here made every stale error look
                        // like a success to the model and to
                        // `compaction::collect_tool_failures`.
                        let was_error =
                            r.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
                        serde_json::json!({
                            "tool_call_id": original_id,
                            "content": trimmed_content,
                            "is_error": was_error
                        })
                    })
                    .collect();
                serde_json::to_string(&preserved).ok()
            } else {
                Some(
                    serde_json::json!([{
                        "tool_call_id": "",
                        "content": trimmed_content,
                        "is_error": false
                    }])
                    .to_string(),
                )
            }
        } else {
            None
        };

        // Read-type results keep a bounded slice of real content, so their
        // new size varies; estimate from the trimmed length rather than a flat 10.
        let new_tokens = (trimmed_content.len() / crate::CHARS_PER_TOKEN).max(10);
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
            token_estimate: Some(new_tokens as i64),
            html: None,
        };
        tokens_saved += old_tokens.saturating_sub(new_tokens);
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
    warning_threshold: usize,
    frozen: &mut std::collections::HashMap<String, String>,
) -> (Vec<ChatMessage>, usize) {
    // Small contexts re-tokenize for pennies — clearing them saves nothing and
    // deletes working knowledge (loaded skill instructions, fetched data) right
    // as the user resumes. Only clear when the stale context is actually large.
    if estimate_total_tokens(messages) < warning_threshold {
        return (messages.to_vec(), 0);
    }
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
        if msg.role == "tool"
            || (msg.tool_results.is_some()
                && msg
                    .tool_results
                    .as_ref()
                    .is_some_and(|tr| !tr.is_empty() && tr != "[]" && tr != "null"))
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

        // Read-type results are deliverables (calendar/mail/file/search). Even
        // when stale, keep a bounded slice of the real content rather than
        // wiping it to "[cleared]" — the model must still be able to report
        // what was fetched. Side-effecting results clear as before.
        let (call_name, call_input) = find_tool_call_for_result(messages, idx);
        let input = call_input.unwrap_or(serde_json::Value::Null);
        // Same ONE inference the executor and `build_tool_summary` use. Reading
        // the raw field here left this path with the exact `[os] 0 lines`
        // defect after the summarizer was fixed: a bare `os {action:"read"}`
        // carries no `resource`, so it was judged side-effecting and wiped to
        // `[cleared]`.
        let resource = tools::OsTool::resolved_resource(&input);
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
        // FROZEN DECISION — same contract as micro_compact: one rendering
        // per tool_use_id per run, shared across both compaction paths.
        let freeze_key = first_tool_call_id(msg);
        let cleared = freeze_key
            .as_ref()
            .and_then(|k| frozen.get(k).cloned())
            .unwrap_or_else(|| {
                if is_read_type(call_name.as_str(), resource, action) {
                    bounded_content(&tool_result_text(msg))
                } else {
                    "[cleared]".to_string()
                }
            });
        if let Some(k) = freeze_key {
            frozen.entry(k).or_insert_with(|| cleared.clone());
        }

        // Preserve tool_call_ids in tool_results JSON
        let compacted_results = if let Some(ref tr_json) = msg.tool_results {
            if let Ok(results) = serde_json::from_str::<Vec<serde_json::Value>>(tr_json) {
                let preserved: Vec<serde_json::Value> = results
                    .iter()
                    .map(|r| {
                        let original_id =
                            r.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or("");
                        // A compacted failure must still read as a failure —
                        // hardcoding `false` here made every stale error look
                        // like a success to the model and to
                        // `compaction::collect_tool_failures`.
                        let was_error =
                            r.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
                        serde_json::json!({
                            "tool_call_id": original_id,
                            "content": cleared,
                            "is_error": was_error
                        })
                    })
                    .collect();
                serde_json::to_string(&preserved).ok()
            } else {
                Some(
                    serde_json::json!([{
                        "tool_call_id": "",
                        "content": cleared,
                        "is_error": false
                    }])
                    .to_string(),
                )
            }
        } else {
            None
        };

        let new_tokens = (cleared.len() / crate::CHARS_PER_TOKEN).max(2);
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
            token_estimate: Some(new_tokens as i64),
            html: None,
        };
        tokens_saved += old_tokens.saturating_sub(new_tokens);
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
    // `file`/`shell` are OsTool's private sub-tools and never appear as a
    // registered tool name — those arms were dead. `os` covers both.
    match tool_name {
        "web" => 0, // Stale fastest
        "os" => 2,  // Shell/file output is often large
        _ => 3,
    }
}

/// Find the tool name and input for a tool result message.
///
/// The result row carries the `tool_call_id` it answers; match on it. The
/// runner issues tool calls in parallel and stores each result as its own
/// message, so "walk back and take the first call" attributed results 2..N of a
/// batch to call 1 — wrong tool, wrong resource, wrong `is_read_type` verdict,
/// and therefore the wrong decision about whether to keep the content. Same
/// family as the `[os] 0 lines` outage. Falls back to the first call only for
/// legacy rows with no id.
fn find_tool_call_for_result(
    messages: &[ChatMessage],
    result_idx: usize,
) -> (String, Option<serde_json::Value>) {
    let wanted_id: Option<String> = messages[result_idx]
        .tool_results
        .as_deref()
        .and_then(|tr| serde_json::from_str::<Vec<serde_json::Value>>(tr).ok())
        .and_then(|rs| {
            rs.first()
                .and_then(|r| r.get("tool_call_id").and_then(|v| v.as_str()))
                .map(str::to_string)
        })
        .filter(|id| !id.is_empty());

    // Look backwards for the assistant message that issued the batch.
    for i in (0..result_idx).rev() {
        let msg = &messages[i];
        if msg.role == "assistant" {
            if let Some(ref tc_json) = msg.tool_calls {
                if let Ok(calls) = serde_json::from_str::<Vec<serde_json::Value>>(tc_json) {
                    let pick = wanted_id
                        .as_deref()
                        .and_then(|id| {
                            calls
                                .iter()
                                .find(|c| c.get("id").and_then(|v| v.as_str()) == Some(id))
                        })
                        .or_else(|| calls.first());
                    if let Some(call) = pick {
                        let name = call
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        let input = call.get("input").cloned();
                        return (name, input);
                    }
                }
            }
            break; // Stop at first assistant message
        }
    }
    ("unknown".to_string(), None)
}

/// Name-only convenience over [`find_tool_call_for_result`] — same id-matched
/// attribution, used for trim ordering.
fn find_tool_name_for_result(messages: &[ChatMessage], result_idx: usize) -> String {
    find_tool_call_for_result(messages, result_idx).0
}

/// Max chars of real content preserved for a read-type tool result during
/// micro-compaction. Read-type results ARE the deliverable (calendar entries,
/// file contents, search hits) — collapsing them to a line count makes the
/// model report "empty"/"0 lines" for data it actually fetched. We keep a
/// bounded slice (token-budget intent preserved: a few KB, not unbounded).
const READ_RESULT_KEEP_CHARS: usize = 3500;

/// Whether a tool result's CONTENT is the deliverable (vs. a side-effect
/// confirmation). Read-type results must keep their actual content through
/// compaction, not be reduced to a line count.
///
/// Read-type:
///   - `os` PIM resources: calendar, mail, contacts, reminders
///   - `os` file reads: read, grep, glob, search
///   - `web` content fetches: search, fetch, sanitize, read_page, get
///   - `agent` memory reads: recall, search, list — the employee's own memory
///   - `skill` loads — instructions the model is following
///   - `plugin` — Gmail/Drive/CRM payloads are the deliverable
///   - every `mcp__*` tool — including Company Memory
///
/// The last four were missing (2026-08-28 audit): once a session passed the
/// warning threshold, a recalled memory, a loaded skill, an email body, or a
/// Company Memory hit was replaced with `[tool] N lines` — the identical
/// failure that produced `[os] 0 lines`, on four more surfaces. Over-including
/// costs a bounded slice (`READ_RESULT_KEEP_CHARS`); under-including hands the
/// model a lie. Err toward keeping.
///
/// Everything else (shell exec, file write/edit, browser click/type/navigate
/// mutations, etc.) is side-effecting and keeps a truthful trimmed stub.
fn is_read_type(tool_name: &str, resource: &str, action: &str) -> bool {
    if tool_name.starts_with("mcp__") {
        return true;
    }
    match tool_name {
        "os" => match resource {
            "calendar" | "mail" | "contacts" | "reminders" => true,
            "file" => matches!(action, "read" | "grep" | "glob" | "search"),
            _ => false,
        },
        "web" => matches!(
            action,
            "search" | "fetch" | "sanitize" | "read_page" | "get"
        ),
        "agent" => resource == "memory" && matches!(action, "recall" | "search" | "list"),
        "skill" => action == "load",
        "plugin" => true,
        _ => false,
    }
}

/// Keep a bounded slice of real content, truncated at a line boundary near
/// the cap, with an explicit truncation marker. Preserves the answer while
/// honoring the token budget.
fn bounded_content(content: &str) -> String {
    if content.len() <= READ_RESULT_KEEP_CHARS {
        return content.to_string();
    }
    // Truncate at a char boundary, then back up to the last newline so we
    // don't cut mid-line.
    let mut cut = READ_RESULT_KEEP_CHARS;
    while cut > 0 && !content.is_char_boundary(cut) {
        cut -= 1;
    }
    let slice = &content[..cut];
    let slice = match slice.rfind('\n') {
        Some(nl) if nl > READ_RESULT_KEEP_CHARS / 2 => &slice[..nl],
        _ => slice,
    };
    // Explicit about WHY it's short — a bare "truncated" (or worse, a blank) reads
    // as "the file is empty"; this tells the model the content existed and is
    // recoverable, so it re-reads instead of concluding the file was empty.
    format!(
        "{}\n…(truncated to save context — the full content was read successfully; re-read this path if you need the rest)",
        slice.trim_end()
    )
}

/// The stub that replaces a side-effecting tool result once it ages out of
/// the window. It states what HAPPENED to the content and how to get it back;
/// it never presents a measurement as if it were the answer. `[os] 0 lines`
/// read as "the tool returned nothing" — this reads as "the tool returned N
/// lines and we trimmed them", which the model can act on correctly.
fn trimmed_stub(label: &str, line_count: usize) -> String {
    format!(
        "{} — {} lines were returned and trimmed from context to save space; \
         re-run the call if you need that output again",
        label, line_count
    )
}

/// Build an informative one-line summary of a tool call + result.
/// Pure string ops — no LLM.
fn build_tool_summary(
    tool_name: &str,
    tool_input: Option<&serde_json::Value>,
    tool_result: &str,
) -> String {
    let line_count = tool_result.lines().count();

    let input = tool_input.unwrap_or(&serde_json::Value::Null);
    // ONE resource inference, shared with the executor (Rule 8) — see
    // `OsTool::resolved_resource`. Reading the raw field here is what let a
    // 651-line file be summarized as `[os] 0 lines`.
    let resource = tools::OsTool::resolved_resource(input);
    let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");

    // Read-type results: the content IS the deliverable. Keep a bounded slice
    // of the real content instead of discarding it for a line count.
    if is_read_type(tool_name, resource, action) {
        return bounded_content(tool_result);
    }

    match tool_name {
        "os" if resource == "shell" => {
            let cmd = input
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let cmd_short = if cmd.len() > 60 {
                format!("{}...", &cmd[..57])
            } else {
                cmd.to_string()
            };
            trimmed_stub(&format!("[{}:shell] {}", tool_name, cmd_short), line_count)
        }
        "os" if resource == "file" && action == "read" => {
            let path = input.get("path").and_then(|v| v.as_str()).unwrap_or("?");
            trimmed_stub(&format!("[{}:file:read] {}", tool_name, path), line_count)
        }
        "os" if resource == "file" => {
            let path = input.get("path").and_then(|v| v.as_str()).unwrap_or("?");
            trimmed_stub(&format!("[{}:file:{}] {}", tool_name, action, path), line_count)
        }
        "web" if action == "search" => {
            let query = input.get("query").and_then(|v| v.as_str()).unwrap_or("?");
            // Count results (rough: count "title" occurrences or similar)
            let result_count = tool_result.matches("\"title\"").count().max(1);
            format!("[web:search] '{}' ({} results)", query, result_count)
        }
        "web" if action == "navigate" => {
            let url = input.get("url").and_then(|v| v.as_str()).unwrap_or("?");
            let url_short = if url.len() > 60 { format!("{}...", &url[..57]) } else { url.to_string() };
            let visual = extract_visual_section(tool_result);
            if let Some(vis) = visual {
                format!("[web:navigate] {} — {}", url_short, vis)
            } else {
                format!("[web:navigate] {}", url_short)
            }
        }
        "web" if action == "read_page" || action == "snapshot" => {
            let visual = extract_visual_section(tool_result);
            if let Some(vis) = visual {
                format!("[web:read_page] {}", vis)
            } else {
                format!("[web:read_page] {} elements", tool_result.matches("ref_").count())
            }
        }
        "web" if matches!(action, "click" | "fill" | "type" | "scroll" | "hover" | "press") => {
            let first_line = tool_result.lines().next().unwrap_or("ok");
            let visual = extract_visual_section(tool_result);
            if let Some(vis) = visual {
                format!("[web:{}] {} — {}", action, first_line, vis)
            } else {
                format!("[web:{}] {}", action, first_line)
            }
        }
        "web" if action == "fetch" => {
            let url = input.get("url").and_then(|v| v.as_str()).unwrap_or("?");
            let url_short = if url.len() > 60 { format!("{}...", &url[..57]) } else { url.to_string() };
            trimmed_stub(&format!("[web:fetch] {}", url_short), line_count)
        }
        _ => {
            let label = if !resource.is_empty() {
                format!("[{}:{}]", tool_name, resource)
            } else {
                format!("[{}]", tool_name)
            };
            trimmed_stub(&label, line_count)
        }
    }
}

/// Extract the `[Page Visual]` sidecar section from a tool result, if present.
/// Returns the structured visual assessment (PAGE/STATUS/BLOCKER/CONTENT/ACTION lines).
fn extract_visual_section(result: &str) -> Option<String> {
    let marker = "[Page Visual]\n";
    let start = result.find(marker)?;
    let visual = &result[start + marker.len()..];
    let trimmed = visual.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Keep only the structured lines (PAGE, STATUS, BLOCKER, CONTENT, ACTION, ELEMENTS)
    let compact: String = trimmed
        .lines()
        .filter(|l| {
            let l = l.trim();
            l.starts_with("PAGE:")
                || l.starts_with("STATUS:")
                || l.starts_with("BLOCKER:")
                || l.starts_with("CONTENT:")
                || l.starts_with("ACTION:")
                || l.starts_with("ELEMENTS:")
                || l.starts_with("- ")
        })
        .collect::<Vec<_>>()
        .join(" | ");
    if compact.is_empty() { None } else { Some(compact) }
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
transcript supersedes. NEVER reset, drop, or restart the summary; the snapshot is earlier \
state of the same ongoing work.

Output ONLY the sections below, in this order, with these exact headings. SKIP any section \
that would be empty — never write \"None\".

## Goal
The user's active task. This is an ONGOING task, not a finished one: the next model must \
NOT treat it as complete, wrap it up, or start it fresh — it must continue exactly where \
the conversation left off.

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
        trace: None,
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
            tool_results: Some(
                serde_json::json!([{
                    "tool_call_id": tool_call_id,
                    "content": content,
                    "is_error": false
                }])
                .to_string(),
            ),
            token_estimate: None,
            html: None,
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
            html: None,
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

        // gap_threshold of 1 second — all messages are old, so gap is huge.
        // warning_threshold 0 opens the pressure gate (this test exercises clearing).
        let (result, tokens_saved) = time_based_micro_compact(&messages, 1, 1, 0, &mut std::collections::HashMap::new());
        assert!(tokens_saved > 0, "should save tokens on stale session");

        // Only the most recent tool result (index 6) should keep its content
        // The older two (indices 2, 4) should be cleared
        let tool_results: Vec<&ChatMessage> = result.iter().filter(|m| m.role == "tool").collect();
        assert_eq!(tool_results.len(), 3);

        // Most recent keeps content
        assert!(
            !tool_results[2].content.contains("[cleared]"),
            "most recent tool result should keep content"
        );
        // Older ones cleared
        assert_eq!(tool_results[0].content, "[cleared]");
        assert_eq!(tool_results[1].content, "[cleared]");
    }

    /// Production shape: a tool row keeps `content` EMPTY and its text in
    /// `tool_results[].content`. Both compaction paths must render from the
    /// payload — rendering from `content` rewrote every stale file read as
    /// an empty result and taught the model that files "appear empty".
    #[test]
    fn compaction_renders_from_tool_results_when_content_is_empty() {
        let old_ts = 1000;
        let file_text: String = (1..=200).map(|i| format!("{i}\tline {i} of the pasted document\n")).collect();
        let mut read_call = make_assistant_msg("reading the upload", old_ts);
        read_call.tool_calls = Some(
            serde_json::json!([{ "name": "os", "id": "call_read",
                "input": { "action": "read", "path": "/uploads/pasted-text.md" } }]).to_string(),
        );
        let mut read_result = make_tool_result_msg("", old_ts); // content EMPTY, like prod
        read_result.tool_results = Some(
            serde_json::json!([{ "tool_call_id": "call_read", "content": file_text, "is_error": false }]).to_string(),
        );
        let convo = vec![
            make_msg("user", "here is the file"),
            read_call,
            read_result,
            make_assistant_msg("got it", old_ts),
            make_tool_result_msg("newest result keeps everything", old_ts),
        ];

        // Stage 1 (stale session): the read is older than keep_recent=1 → bounded, not emptied.
        let (tb, _) = time_based_micro_compact(&convo, 1, 1, 0, &mut std::collections::HashMap::new());
        let tb_read = &tb[2];
        assert!(tb_read.content.contains("line 1 of the pasted document"), "time-based kept real text: {:?}", &tb_read.content[..60.min(tb_read.content.len())]);
        assert!(!tb_read.content.is_empty() && tb_read.content != "[cleared]");
        let tr: Vec<serde_json::Value> = serde_json::from_str(tb_read.tool_results.as_deref().unwrap()).unwrap();
        assert!(tr[0]["content"].as_str().unwrap().contains("line 1 of"), "payload rendered into tool_results too");

        // Stage 2 (micro-compact): the summary must count the real lines, never "0 lines".
        let (mc, _) = micro_compact(&convo, 0, &mut std::collections::HashMap::new());
        let mc_read = &mc[2];
        assert!(!mc_read.content.contains("0 lines"), "summary saw the payload: {}", mc_read.content);
    }

    #[test]
    fn test_time_based_micro_compact_preserves_read_type_content() {
        // Stale session, but the older tool result is a read-type deliverable
        // (calendar). It must NOT be wiped to "[cleared]" — its content (bounded)
        // must survive so the model can still report what was fetched.
        let old_ts = 1000;
        let mut cal_call = make_assistant_msg("checking calendar", old_ts);
        cal_call.tool_calls = Some(
            serde_json::json!([{
                "name": "os",
                "id": "call_cal",
                "input": { "resource": "calendar", "action": "today" }
            }])
            .to_string(),
        );
        let cal_result = make_tool_result_msg("9:00 Standup\n13:00 Lunch with client", old_ts);

        let big = "x".repeat(4000);
        let messages = vec![
            make_msg("user", "what's on my calendar"),
            cal_call,
            cal_result,
            make_assistant_msg("now searching", old_ts),
            make_tool_result_msg(&big, old_ts), // most recent (kept anyway)
        ];

        let (result, _) = time_based_micro_compact(&messages, 1, 1, 0, &mut std::collections::HashMap::new());
        let tool_results: Vec<&ChatMessage> = result.iter().filter(|m| m.role == "tool").collect();
        // Older calendar result kept content despite being stale + not most-recent
        assert!(
            tool_results[0].content.contains("Lunch with client"),
            "stale read-type result must keep content, got: {}",
            tool_results[0].content
        );
        assert_ne!(tool_results[0].content, "[cleared]");
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
        let (_, tokens_saved) = time_based_micro_compact(&messages, 1, 300, 0, &mut std::collections::HashMap::new());
        assert_eq!(tokens_saved, 0, "active session should not be compacted");
    }

    #[test]
    fn test_micro_compact_universal_tools() {
        // Tool results from non-standard tools (e.g. "search_emails") should
        // now be compactable since we removed the category filter.
        let big = "x".repeat(4000);
        let mut messages = Vec::new();
        // Create 8 tool results with a custom tool name — exceeds count trigger (4)
        // and leaves ≥2 compactable after the keep-recent-5 protection.
        for i in 0..8 {
            let mut assistant = make_old_msg("assistant", "calling tool");
            assistant.tool_calls = Some(
                serde_json::json!([{
                    "name": "search_emails",
                    "id": format!("call_{}", i),
                    "input": {}
                }])
                .to_string(),
            );
            messages.push(assistant);
            messages.push(make_tool_result_msg(&big, 1000));
        }

        // Threshold below the ~16K estimated total so the pressure gate opens.
        let (result, tokens_saved) = micro_compact(&messages, 1_000, &mut std::collections::HashMap::new());
        assert!(
            tokens_saved > 0,
            "non-standard tool results should be compactable (universal filter)"
        );

        // Should keep 5 most recent, compact the older 3.
        // Tool summaries now use informative format like "[search_emails] N lines"
        let compacted_count = result
            .iter()
            .filter(|m| m.content.contains("[search_emails]"))
            .count();
        assert!(
            compacted_count >= 2,
            "should compact at least 2 old results, got {}",
            compacted_count
        );
    }

    #[test]
    fn test_compaction_pressure_gate() {
        // Below the warning threshold neither compaction stage touches anything.
        // Regression: stripping results mid-run (count trigger) or on resume
        // (stale-session clear) deleted skill instructions the model had just
        // loaded, making it "start over."
        let big = "x".repeat(4000);
        let mut messages = Vec::new();
        for i in 0..8 {
            let mut assistant = make_old_msg("assistant", "calling tool");
            assistant.tool_calls = Some(
                serde_json::json!([{
                    "name": "search_emails",
                    "id": format!("call_{}", i),
                    "input": {}
                }])
                .to_string(),
            );
            messages.push(assistant);
            messages.push(make_tool_result_msg(&big, 1000));
        }

        let (result, saved) = micro_compact(&messages, 100_000, &mut std::collections::HashMap::new());
        assert_eq!(saved, 0, "micro_compact must not fire under the threshold");
        assert!(
            result.iter().all(|m| !m.content.contains("[search_emails]")),
            "no result may be summarized under the threshold"
        );

        let (_, tb_saved) = time_based_micro_compact(&messages, 1, 1, 100_000, &mut std::collections::HashMap::new());
        assert_eq!(
            tb_saved, 0,
            "stale-session clear must not fire under the threshold"
        );
    }

    /// Once a rendering is chosen for a tool_use_id it NEVER changes within
    /// the run — even if the underlying message would render differently on a
    /// later pass. Re-deciding per iteration is how the model watched its own
    /// history mutate mid-run (the outage's delivery mechanism).
    #[test]
    fn frozen_renderings_never_change_within_a_run() {
        let calls = r#"[{"id":"c1","name":"os","input":{"action":"exec","command":"cargo build"}}]"#;
        let big = "line\n".repeat(1200);
        let mut convo = vec![
            tmsg("user", "go", None, None),
            tmsg("assistant", "", Some(calls), None),
            tmsg("tool", &big, None, Some(r#"[{"tool_call_id":"c1","content":"..."}]"#)),
        ];
        pad_past_compaction(&mut convo);
        let mut frozen = std::collections::HashMap::new();
        let (out1, saved) = micro_compact(&convo, 1_000, &mut frozen);
        assert!(saved > 0, "the test must actually compact something");
        let first_rendering = out1[2].content.clone();

        // Mutate the underlying content — a fresh decision would now differ.
        convo[2].content = "totally different\n".repeat(1500);
        let (out2, _) = micro_compact(&convo, 1_000, &mut frozen);
        assert_eq!(
            out2[2].content, first_rendering,
            "the rendering for c1 must be frozen, not re-decided"
        );
    }

    /// The `system` -> `os` rename must stay finished.
    ///
    /// It was left half-done once: some match arms were updated, others were
    /// not, the missed ones went permanently dead, and a customer's 651-line
    /// file read was collapsed to `[os] 0 lines` and handed to the model as the
    /// tool's answer. It then told its owner for 15+ turns that it could not
    /// read a file it had already read.
    ///
    /// The instinct on finding that wreckage is to add an alias or a normalizer
    /// so both names keep working. Both are the same mistake: they make the
    /// half-done rename permanent and hand the next matcher the same chance to
    /// remember one name and miss the other. Zero stored messages ever carried
    /// the old name — the compatibility was never needed. Finish renames.
    #[test]
    fn the_old_tool_name_never_comes_back() {
        let source = include_str!("pruning.rs");
        let offenders: Vec<&str> = source
            .lines()
            .filter(|l| l.contains("\"system\""))
            // this test names it on purpose
            .filter(|l| !l.contains("the_old_tool_name_never_comes_back"))
            .collect();
        assert!(
            offenders.is_empty(),
            "`system` is a dead tool name — normalize at the boundary or migrate \
             the data, never match on it here:\n{}",
            offenders.join("\n")
        );
    }

    #[test]
    fn test_build_tool_summary_shell() {
        let input = serde_json::json!({
            "resource": "shell",
            "command": "ls -la /tmp"
        });
        let result = "file1.txt\nfile2.txt\nfile3.txt\n";
        let summary = build_tool_summary("os", Some(&input), result);
        assert!(summary.starts_with("[os:shell]"));
        assert!(summary.contains("ls -la /tmp"));
        assert!(summary.contains("3 lines"));
    }

    #[test]
    fn test_build_tool_summary_file_read_preserves_content() {
        // File reads are read-type: the content IS the deliverable, so the
        // summary keeps the actual content rather than a line count.
        let input = serde_json::json!({
            "resource": "file",
            "action": "read",
            "path": "/home/user/code.rs"
        });
        let result = "line1\nline2\n";
        let summary = build_tool_summary("os", Some(&input), result);
        assert_eq!(summary, result, "read-type result content must survive");
        assert!(!summary.contains("lines"));
    }

    #[test]
    /// END-TO-END guard for the 2026-08-28 outage class.
    ///
    /// `build_tool_summary` unit tests were not enough — the defect only shows
    /// when a real conversation goes through `micro_compact`, which is the path
    /// that rewrote a customer's 651-line file read into `[os] 0 lines` and
    /// handed it to the model as the tool's answer. He then watched his agent
    /// insist, for 15+ turns, that it could not read a file it had already read.
    ///
    /// The invariant this locks: **compaction may shorten a tool result, but it
    /// must never replace the answer with a bare count.** If this test ever goes
    /// red, an agent somewhere is about to be told its tools are broken.
    #[test]
    fn compaction_never_turns_a_file_read_into_a_line_count() {
        fn msg(role: &str, content: &str, calls: Option<&str>, results: Option<&str>) -> ChatMessage {
            ChatMessage {
                id: String::new(),
                chat_id: String::new(),
                role: role.to_string(),
                content: content.to_string(),
                metadata: None,
                created_at: 0,
                day_marker: None,
                tool_calls: calls.map(|c| c.to_string()),
                tool_results: results.map(|r| r.to_string()),
                token_estimate: None,
                html: None,
            }
        }

        // The exact call shape the model emits: no `resource` field.
        let call = r#"[{"id":"c1","name":"os","input":{"action":"read","path":"/home/j/grabber.py"}}]"#;
        let body: String = (0..400)
            .map(|i| format!("{:6}\tline_{i} = 'payload'\n", i + 1))
            .collect();

        let mut convo = vec![
            msg("user", "read grabber.py and fix the bugs", None, None),
            msg("assistant", "", Some(call), None),
            // Production shape: the payload rides tool_results; `content` is empty.
            msg("tool", "", None, Some(&serde_json::json!([{"tool_call_id":"c1","content":body}]).to_string())),
        ];
        // Bulk it past the compaction threshold AND past the keep-recent
        // protection, so the read under test is a real candidate. The first
        // version of this test had no other tool results, so nothing was ever
        // compacted and it passed vacuously.
        pad_past_compaction(&mut convo);

        let (compacted, saved) = micro_compact(&convo, 1_000, &mut std::collections::HashMap::new());
        assert!(saved > 0, "the test must actually compact something");
        let read_result = &compacted[2].content;
        assert_ne!(read_result, &body, "the read must have gone through the summarizer");

        assert!(
            !read_result.contains("0 lines"),
            "a file read must never compact to a line count — this is the bug: {read_result}"
        );
        assert!(
            read_result.contains("line_1 ") || read_result.contains("payload"),
            "the file's content must survive compaction: {}",
            crate::runner::truncate_str(read_result, 200)
        );
    }

    fn tmsg(role: &str, content: &str, calls: Option<&str>, results: Option<&str>) -> ChatMessage {
        ChatMessage {
            id: String::new(),
            chat_id: String::new(),
            role: role.to_string(),
            content: content.to_string(),
            metadata: None,
            created_at: 0,
            day_marker: None,
            tool_calls: calls.map(|c| c.to_string()),
            tool_results: results.map(|r| r.to_string()),
            token_estimate: None,
            html: None,
        }
    }

    /// `micro_compact` protects the 5 most recent tool results and only compacts
    /// candidates beyond them. A test whose only tool results are the ones under
    /// test therefore compacts NOTHING and passes vacuously — which is exactly
    /// what happened to the first version of these tests. This pads a
    /// conversation so the results under test are real candidates.
    fn pad_past_compaction(convo: &mut Vec<ChatMessage>) {
        for i in 0..5 {
            convo.push(tmsg(
                "assistant", "",
                Some(&format!(r#"[{{"id":"pad{i}","name":"os","input":{{"action":"exec","command":"true"}}}}]"#)),
                None,
            ));
            convo.push(tmsg(
                "tool", "ok\n", None,
                Some(&format!(r#"[{{"tool_call_id":"pad{i}","content":"ok"}}]"#)),
            ));
        }
        for i in 0..40 {
            convo.push(tmsg("user", &format!("f{i} {}", "x".repeat(400)), None, None));
            convo.push(tmsg("assistant", &format!("r{i} {}", "y".repeat(400)), None, None));
        }
    }

    /// The runner issues tool calls in parallel and stores one message per
    /// result. Each result must be summarized against ITS OWN call — the old
    /// `calls.first()` lookup summarized a web fetch as if it were an os read.
    #[test]
    fn parallel_batch_results_keep_their_own_call() {
        let calls = r#"[
            {"id":"c1","name":"os","input":{"action":"read","path":"/a.py"}},
            {"id":"c2","name":"os","input":{"action":"exec","command":"cargo build"}},
            {"id":"c3","name":"web","input":{"action":"fetch","url":"https://example.com/x"}}
        ]"#;
        let big = "line\n".repeat(1200);
        let mut convo = vec![
            tmsg("user", "go", None, None),
            tmsg("assistant", "", Some(calls), None),
            // Production shape: the payload rides tool_results; `content` is empty.
            tmsg("tool", "", None, Some(&serde_json::json!([{"tool_call_id":"c1","content":big}]).to_string())),
            tmsg("tool", "", None, Some(&serde_json::json!([{"tool_call_id":"c2","content":big}]).to_string())),
            tmsg("tool", "", None, Some(&serde_json::json!([{"tool_call_id":"c3","content":big}]).to_string())),
        ];
        pad_past_compaction(&mut convo);
        let (out, saved) = micro_compact(&convo, 1_000, &mut std::collections::HashMap::new());
        assert!(saved > 0, "the test must actually compact something");

        // c1 is a read → content kept. c2 is a shell exec → truthful stub
        // naming the COMMAND. c3 is a web fetch → read-type, content kept.
        assert!(out[2].content.contains("line"), "read keeps content: {}", out[2].content);
        assert!(
            out[3].content.starts_with("[os:shell] cargo build"),
            "exec result must be attributed to its own call: {}",
            out[3].content
        );
        assert!(out[4].content.contains("line"), "fetch keeps content: {}", out[4].content);
    }

    /// A compacted failure must still be a failure.
    #[test]
    fn compaction_preserves_is_error() {
        let calls = r#"[{"id":"c1","name":"os","input":{"action":"exec","command":"cargo test"}}]"#;
        let big = "error[E0308]: mismatched types\n".repeat(300);
        let mut convo = vec![
            tmsg("user", "go", None, None),
            tmsg("assistant", "", Some(calls), None),
            tmsg("tool", &big, None,
                 Some(r#"[{"tool_call_id":"c1","content":"...","is_error":true}]"#)),
        ];
        pad_past_compaction(&mut convo);
        let (out, saved) = micro_compact(&convo, 1_000, &mut std::collections::HashMap::new());
        assert!(saved > 0, "the test must actually compact something");
        assert_ne!(out[2].content, big, "the failure result must have been compacted");
        let tr: Vec<serde_json::Value> =
            serde_json::from_str(out[2].tool_results.as_deref().unwrap()).unwrap();
        assert_eq!(
            tr[0].get("is_error").and_then(|v| v.as_bool()),
            Some(true),
            "a compacted failure read as a success: {:?}",
            tr[0]
        );
    }

    /// The time-based path had the same missing-`resource` defect as the
    /// summarizer after the summarizer was fixed. A bare `os read` must keep
    /// its content here too, not become `[cleared]`.
    #[test]
    fn time_based_compact_infers_resource_like_the_executor() {
        let calls = r#"[{"id":"c1","name":"os","input":{"action":"read","path":"/a.py"}}]"#;
        let big = "def f():\n    pass\n".repeat(200);
        let convo = vec![
            tmsg("user", "go", None, None),
            tmsg("assistant", "", Some(calls), None),   // created_at 0 → stale
            tmsg("tool", &big, None, Some(r#"[{"tool_call_id":"c1","content":"..."}]"#)),
            tmsg("assistant", "", Some(r#"[{"id":"c2","name":"os","input":{"action":"exec","command":"ls"}}]"#), None),
            tmsg("tool", "a\nb\n", None, Some(r#"[{"tool_call_id":"c2","content":"..."}]"#)),
        ];
        let (out, _) = time_based_micro_compact(&convo, 1, 1, 0, &mut std::collections::HashMap::new());
        assert_ne!(out[2].content, "[cleared]", "a read must not be wiped");
        assert!(out[2].content.contains("def f()"), "read keeps content: {}", out[2].content);
    }

    /// Names that are not registered tools get no special treatment, so a
    /// half-finished rename can never leave a silently dead arm again.
    #[test]
    fn dead_tool_names_get_no_special_treatment() {
        let baseline = trim_priority("definitely-not-a-tool");
        // (the retired os name is covered by the source-grep guard above; naming it here
        // would trip that guard.)
        for dead in ["file", "shell", "bot"] {
            assert_eq!(trim_priority(dead), baseline, "{dead} must not have its own priority");
            let s = build_tool_summary(dead, Some(&serde_json::json!({"action":"x"})), "a\nb\n");
            assert!(!s.starts_with(&format!("[{dead}:")), "{dead} must fall to the catch-all: {s}");
        }
    }

    /// The four families the audit found still being collapsed.
    #[test]
    fn is_read_type_covers_memory_skill_plugin_mcp() {
        assert!(is_read_type("agent", "memory", "recall"));
        assert!(is_read_type("agent", "memory", "search"));
        assert!(!is_read_type("agent", "task", "create"), "a spawn/create is side-effecting");
        assert!(is_read_type("skill", "", "load"));
        assert!(is_read_type("plugin", "gws", ""));
        assert!(is_read_type("mcp__memory__memory_search", "", ""));
        assert!(!is_read_type("os", "shell", "exec"));
    }

    /// A stub states what happened and how to recover; it never reads as the
    /// tool's answer. `[os] 0 lines` read as "the tool returned nothing".
    #[test]
    fn trimmed_stub_states_what_happened() {
        let s = build_tool_summary("custom_tool", Some(&serde_json::json!({})), "a\nb\nc\n");
        assert!(s.contains("3 lines were returned"), "{s}");
        assert!(s.contains("trimmed from context"), "{s}");
        assert!(s.contains("re-run"), "{s}");
    }

    /// The 2026-08-28 production defect, exactly as it arrived.
    ///
    /// A plain `os` read carries no `resource` field — the tool infers it. The
    /// summary did not, so a 651-line file was replaced with `[os] 0 lines` and
    /// handed to the model as the tool's answer. It then reported, for 15+
    /// turns, that every method of reading the file returned empty. The file
    /// was fine; the history was forged.
    #[test]
    fn read_without_explicit_resource_preserves_content() {
        let input = serde_json::json!({
            "action": "read",
            "path": "/home/jorgen/Nebo/x96-archive/stream-grabber/grabber.py"
        });
        let result = "#!/usr/bin/env python3\nimport asyncio\nimport json\n";
        let summary = build_tool_summary("os", Some(&input), result);

        assert!(
            !summary.contains("0 lines"),
            "a read must never collapse to a line count: {summary}"
        );
        assert!(
            summary.contains("import asyncio"),
            "the file content IS the deliverable: {summary}"
        );
    }

    /// The shell half of the same inference — a bare `exec` has no `resource`.
    #[test]
    fn exec_without_explicit_resource_is_identified_as_shell() {
        let input = serde_json::json!({"action": "exec", "command": "ls -la /tmp"});
        let summary = build_tool_summary("os", Some(&input), "a\nb\nc\n");
        assert!(
            !summary.starts_with("[os] "),
            "an exec must not fall to the unidentified catch-all: {summary}"
        );
    }

    fn test_build_tool_summary_calendar_preserves_content() {
        // Calendar reads were collapsing to "[os:calendar] 0 lines" — the bug.
        // Now the real content must be preserved.
        let input = serde_json::json!({
            "resource": "calendar",
            "action": "today"
        });
        let result = "9:00 Standup\n13:00 Lunch with client\n15:30 Design review";
        let summary = build_tool_summary("os", Some(&input), result);
        assert_eq!(summary, result);
        assert!(summary.contains("Lunch with client"));
    }

    #[test]
    fn test_build_tool_summary_read_type_bounded() {
        // Large read-type content is bounded with a truncation marker.
        let input = serde_json::json!({ "resource": "mail", "action": "unread" });
        let result = "x".repeat(10_000);
        let summary = build_tool_summary("os", Some(&input), &result);
        assert!(summary.len() < result.len(), "should be bounded");
        assert!(summary.len() <= READ_RESULT_KEEP_CHARS + 160);
        assert!(summary.contains("truncated to save context"));
    }

    #[test]
    fn test_build_tool_summary_web_search_preserves_content() {
        // web search is read-type — keep the result payload, not a count.
        let input = serde_json::json!({
            "resource": "search",
            "action": "search",
            "query": "rust async tutorial"
        });
        let result = r#"{"title": "Async Rust", "url": "..."}, {"title": "Tokio Guide", "url": "..."}"#;
        let summary = build_tool_summary("web", Some(&input), result);
        assert_eq!(summary, result);
        assert!(summary.contains("Tokio Guide"));
    }

    #[test]
    fn test_build_tool_summary_fallback() {
        let input = serde_json::json!({});
        let result = "some output\n";
        let summary = build_tool_summary("custom_tool", Some(&input), result);
        assert!(summary.starts_with("[custom_tool]"));
        assert!(summary.contains("lines"));
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
