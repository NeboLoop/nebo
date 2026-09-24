//! Trimming old tool results to stubs, with each rendering frozen the first
//! time it is chosen so the prompt prefix stays stable. Runs every step:
//! `time_based_micro_compact` clears stale results after an idle gap,
//! `micro_compact` stubs, dedupes and drops old screenshots once the
//! conversation nears its window. Moved as-is from `pruning.rs` (WP2.6).

use db::models::ChatMessage;
use tracing::debug;

use crate::pruning::{IMAGE_CHAR_ESTIMATE, estimate_message_tokens, estimate_total_tokens};

/// Frozen renderings, keyed on the tool call id: the first rendering chosen
/// for a result is the rendering forever (persisted per chat).
pub type Frozen = std::collections::HashMap<String, String>;

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


/// The text a tool-result row actually carries. In production tool rows keep
/// `content` EMPTY and the payload in `tool_results[].content`; rendering
/// from `msg.content` produced empty "bounded slices" and `[os] 0 lines`
/// summaries, rewriting every earlier file read in the model's history as
/// "nothing came back" — which the model then believed about fresh reads too
/// (2026-09-01: "the file appears empty" on files that read fine).
/// The note the runner appends to a result it already flagged as a repeat
/// (runner.rs, redundancy guard). Stripped before comparing, so the flagged
/// copy still matches the original it duplicates.
const REDUNDANT_RESULT_NOTE: &str = "\n\n(Note: this is identical to a result you already received earlier in this session.";

/// What a duplicate becomes: the event (a repeat), the original it repeats
/// (by tool_call_id), and the recovery (use that one). The original stays
/// whole; every later copy is one line, so a run that re-fetched the same
/// page or re-ran the same search N times costs one result, not N.
fn duplicate_result_stub(original_call_id: &str) -> String {
    if original_call_id.is_empty() {
        DUPLICATE_RESULT_STUB.to_string()
    } else {
        format!("(identical to the result of tool call {original_call_id} earlier in this conversation — you already have this content there; do not fetch it again)")
    }
}
const DUPLICATE_RESULT_STUB: &str = "(identical to an earlier result in this conversation — you already have this content; do not fetch it again)";
const DUPLICATE_RESULT_STUB_PREFIX: &str = "(identical to ";

/// How many image-bearing tool results keep their image. A UI drive returns
/// a picture per step (~1.5K tokens each); the model needs the latest one or
/// two to act, never the twenty before them.
const KEEP_RECENT_IMAGES: usize = 2;
/// Appended to a result whose image was dropped, so the text says what is
/// missing and what to do about it instead of silently reading as text-only.
const IMAGE_REMOVED_NOTE: &str = "(screenshot removed from context — this was the view at that step; take a fresh capture if you need it)";

/// Drop `image_url` from every tool-result row of `msg`, noting it in the
/// row text and the message text. Returns how many images were dropped.
fn strip_result_images(msg: &mut ChatMessage) -> usize {
    let Some(tr) = msg.tool_results.as_deref() else { return 0 };
    let Ok(mut rows) = serde_json::from_str::<Vec<serde_json::Value>>(tr) else { return 0 };
    let mut dropped = 0;
    for row in rows.iter_mut() {
        let Some(obj) = row.as_object_mut() else { continue };
        if obj.remove("image_url").and_then(|v| v.as_str().map(|s| !s.is_empty())).unwrap_or(false) {
            dropped += 1;
            let text = obj.get("content").and_then(|c| c.as_str()).unwrap_or("");
            let noted = if text.is_empty() { IMAGE_REMOVED_NOTE.to_string() } else { format!("{text}\n{IMAGE_REMOVED_NOTE}") };
            obj.insert("content".into(), serde_json::Value::String(noted));
        }
    }
    if dropped > 0 {
        msg.tool_results = serde_json::to_string(&rows).ok();
        if !msg.content.contains(IMAGE_REMOVED_NOTE) {
            if !msg.content.is_empty() {
                msg.content.push('\n');
            }
            msg.content.push_str(IMAGE_REMOVED_NOTE);
        }
    }
    dropped
}

/// True when any tool-result row of `msg` still carries an image.
fn result_has_image(msg: &ChatMessage) -> bool {
    msg.tool_results
        .as_deref()
        .and_then(|tr| serde_json::from_str::<Vec<serde_json::Value>>(tr).ok())
        .map(|rows| rows.iter().any(|r| r.get("image_url").and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty())))
        .unwrap_or(false)
}

fn dedup_key(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let body = match text.find(REDUNDANT_RESULT_NOTE) {
        Some(i) => &text[..i],
        None => text,
    };
    let mut h = std::collections::hash_map::DefaultHasher::new();
    body.trim_end().hash(&mut h);
    h.finish()
}

/// A result message with its content replaced, tool_call_ids and error flags
/// intact so the orphan filter still pairs it with its call.
fn stub_result(msg: &ChatMessage, text: &str) -> ChatMessage {
    let tool_results = msg.tool_results.as_deref().map(|tr| {
        match serde_json::from_str::<Vec<serde_json::Value>>(tr) {
            Ok(results) => serde_json::to_string(
                &results
                    .iter()
                    .map(|r| {
                        serde_json::json!({
                            "tool_call_id": r.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or(""),
                            "content": text,
                            "is_error": r.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false)
                        })
                    })
                    .collect::<Vec<_>>(),
            )
            .unwrap_or_default(),
            Err(_) => serde_json::json!([{"tool_call_id": "", "content": text, "is_error": false}]).to_string(),
        }
    });
    ChatMessage {
        id: msg.id.clone(),
        chat_id: msg.chat_id.clone(),
        role: msg.role.clone(),
        content: text.to_string(),
        metadata: msg.metadata.clone(),
        created_at: msg.created_at,
        day_marker: msg.day_marker.clone(),
        tool_calls: msg.tool_calls.clone(),
        tool_results,
        token_estimate: Some(((text.len() / crate::CHARS_PER_TOKEN).max(10)) as i64),
        html: None,
    }
}

fn tool_result_text(msg: &ChatMessage) -> String {
    if let Some(tr) = msg.tool_results.as_deref()
        && let Ok(results) = serde_json::from_str::<Vec<serde_json::Value>>(tr)
    {
        let joined: Vec<&str> = results
            .iter()
            .filter_map(|r| r.get("content").and_then(|c| c.as_str()))
            .filter(|c| !c.is_empty())
            .collect();
        if !joined.is_empty() {
            return joined.join("\n");
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

/// Micro-compact: trim old tool results to reduce context size.
/// Returns modified messages and tokens saved.
pub fn micro_compact(
    messages: &[ChatMessage],
    warning_threshold: usize,
    frozen: &mut std::collections::HashMap<String, String>,
    spec: &TrimSpec,
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

    // Duplicates first, and outside the keep-recent protection: a result whose
    // text is byte-identical to an earlier one adds nothing however recent it
    // is. The 2026-09-19 search loop kept 5 copies of one ~10K-token search
    // under "protect the most recent" and never got under the threshold.
    // Frozen like every other rendering, keyed on the tool_call_id.
    {
        // content hash → tool_call_id of the first (kept) copy
        let mut seen: std::collections::HashMap<u64, String> = std::collections::HashMap::new();
        for slot in result.iter_mut() {
            let msg = &*slot;
            if msg.role != "tool" && msg.role != "assistant" {
                continue;
            }
            let Some(tr) = msg.tool_results.as_deref() else { continue };
            if tr.is_empty() || tr == "[]" || tr == "null" {
                continue;
            }
            let text = tool_result_text(msg);
            if text.len() < 200 || text.starts_with(DUPLICATE_RESULT_STUB_PREFIX) {
                continue;
            }
            let key = dedup_key(&text);
            let call_id = first_tool_call_id(msg).unwrap_or_default();
            match seen.get(&key) {
                None => {
                    seen.insert(key, call_id);
                }
                Some(original) => {
                    let old_tokens = estimate_message_tokens(msg);
                    let text = frozen
                        .get(&call_id)
                        .cloned()
                        .unwrap_or_else(|| duplicate_result_stub(original));
                    if !call_id.is_empty() {
                        frozen.entry(call_id).or_insert_with(|| text.clone());
                    }
                    let stub = stub_result(msg, &text);
                    tokens_saved += old_tokens.saturating_sub(estimate_message_tokens(&stub));
                    *slot = stub;
                }
            }
        }
    }

    // Old screenshots next, also outside the keep-recent protection: every
    // step of a UI drive returns a picture, and only the newest couple say
    // anything about the screen as it is now. Not frozen: the rule is
    // monotonic (a result that lost its image only gets older), so each pass
    // makes the same decision without a map.
    {
        let mut with_images: Vec<usize> = result.iter().enumerate().filter(|(_, m)| result_has_image(m)).map(|(i, _)| i).collect();
        with_images.truncate(with_images.len().saturating_sub(KEEP_RECENT_IMAGES));
        for i in with_images {
            let old_tokens = estimate_message_tokens(&result[i]);
            let dropped = strip_result_images(&mut result[i]);
            // A path `image_url` is a few bytes here but a whole image at the
            // provider; count what the provider would have sent.
            let provider_tokens = dropped * IMAGE_CHAR_ESTIMATE / crate::CHARS_PER_TOKEN;
            tokens_saved += old_tokens.saturating_sub(estimate_message_tokens(&result[i])).max(provider_tokens);
        }
    }

    // Find tool result indices eligible for compaction.
    // ALL tool results are compactable — the keep-recent protection prevents
    // stripping results the model still needs.
    let mut tool_result_indices: Vec<(usize, usize, String, Trim)> = Vec::new(); // (index, age_from_end, tool_name, spec)

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
            let trim = trim_of(spec, messages, i);
            tool_result_indices.push((i, age, tool_name, trim));
        }
    }

    // Sort by trim priority then age (oldest first)
    tool_result_indices.sort_by(|a, b| {
        let pa = a.3.priority;
        let pb = b.3.priority;
        pa.cmp(&pb).then(b.1.cmp(&a.1)) // higher priority first, then oldest first
    });

    // Protect most recent N results
    let protect_count = std::cmp::min(MICRO_COMPACT_KEEP_RECENT, tool_result_indices.len());
    let candidates = if tool_result_indices.len() > protect_count {
        &tool_result_indices[..tool_result_indices.len() - protect_count]
    } else {
        // Nothing beyond the protected tail — but the duplicate pass above
        // may already have paid for itself.
        return if tokens_saved >= MICRO_COMPACT_MIN_SAVINGS { (result, tokens_saved) } else { (messages.to_vec(), 0) };
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

    for (idx, age, tool_name, trim) in candidates {
        if *age < min_age {
            continue;
        }

        let msg = &result[*idx];
        let old_tokens = estimate_message_tokens(msg);
        if old_tokens < 100 || msg.content.starts_with(DUPLICATE_RESULT_STUB_PREFIX) {
            continue; // Not worth compacting small results
        }

        // Build informative summary instead of generic "[trimmed: X result]".
        // FROZEN DECISION: the first rendering ever chosen for a tool_use_id
        // is the rendering forever (per run). Re-deciding each iteration is
        // how a result rendered fine on pass N became "[os] 0 lines" on pass
        // N+1 — the model must never watch its own history mutate.
        let (_call_name, call_input, _) = find_tool_call_for_result(messages, *idx);
        let freeze_key = first_tool_call_id(msg);
        let trimmed_content = freeze_key
            .as_ref()
            .and_then(|k| frozen.get(k).cloned())
            .unwrap_or_else(|| build_tool_summary(tool_name, call_input.as_ref(), &tool_result_text(msg), trim.keeps_content));
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
    spec: &TrimSpec,
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
        let keeps_content = trim_of(spec, messages, idx).keeps_content;
        // FROZEN DECISION — same contract as micro_compact: one rendering
        // per tool_use_id per run, shared across both compaction paths.
        let freeze_key = first_tool_call_id(msg);
        let cleared = freeze_key
            .as_ref()
            .and_then(|k| frozen.get(k).cloned())
            .unwrap_or_else(|| {
                if keeps_content {
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

/// What trimming needs to know about one tool call, from its tool's spec
/// (`DynTool::trim_priority`, `DynTool::keeps_content_when_trimmed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trim {
    /// Lowest is trimmed first.
    pub priority: u8,
    /// The result is something read: an aged copy keeps a bounded slice of
    /// its content instead of a stub.
    pub keeps_content: bool,
}

impl Default for Trim {
    fn default() -> Self {
        Self {
            priority: tools::registry::TRIM_DEFAULT,
            keeps_content: false,
        }
    }
}

/// Each tool call's [`Trim`], by tool call id.
pub type TrimSpec = std::collections::HashMap<String, Trim>;

/// The trim facts for the result at `idx`, by the call it answers (found
/// the way every compaction path finds it).
fn trim_of(spec: &TrimSpec, messages: &[ChatMessage], idx: usize) -> Trim {
    find_tool_call_for_result(messages, idx)
        .2
        .and_then(|id| spec.get(&id).copied())
        .unwrap_or_default()
}

/// Find the tool name and input for a tool result message.
///
/// The result row carries the `tool_call_id` it answers; match on it. The
/// runner issues tool calls in parallel and stores each result as its own
/// message, so "walk back and take the first call" attributed results 2..N of a
/// batch to call 1 — wrong tool, wrong resource, wrong keep-content verdict,
/// and therefore the wrong decision about whether to keep the content. Same
/// family as the `[os] 0 lines` outage. Falls back to the first call only for
/// legacy rows with no id.
fn find_tool_call_for_result(
    messages: &[ChatMessage],
    result_idx: usize,
) -> (String, Option<serde_json::Value>, Option<String>) {
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
            if let Some(ref tc_json) = msg.tool_calls
                && let Ok(calls) = serde_json::from_str::<Vec<serde_json::Value>>(tc_json)
            {
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
                    let id = call.get("id").and_then(|v| v.as_str()).map(str::to_string);
                    return (name, input, id);
                }
            }
            break; // Stop at first assistant message
        }
    }
    ("unknown".to_string(), None, None)
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
    keeps_content: bool,
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
    if keeps_content {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Each call's trim facts, read from the real tools' specs (os and web
    /// here; any other tool gets the default).
    fn keeps(name: &str, input: Option<&serde_json::Value>) -> bool {
        trim(name, input.unwrap_or(&serde_json::Value::Null)).keeps_content
    }

    fn trim(name: &str, input: &serde_json::Value) -> Trim {
        use tools::registry::DynTool;
        let tool: Box<dyn DynTool> = match name {
            "os" => Box::new(tools::OsTool::new(
                tools::Policy::default(),
                std::sync::Arc::new(tools::ProcessRegistry::new()),
            )),
            "web" => Box::new(tools::WebTool::new()),
            _ => return Trim::default(),
        };
        Trim {
            priority: tool.trim_priority(),
            keeps_content: tool.keeps_content_when_trimmed(input),
        }
    }

    fn spec(messages: &[ChatMessage]) -> TrimSpec {
        let mut spec = TrimSpec::new();
        for m in messages {
            let calls: Vec<ai::ToolCall> = m
                .tool_calls
                .as_deref()
                .and_then(|j| serde_json::from_str(j).ok())
                .unwrap_or_default();
            for c in calls {
                spec.insert(c.id.clone(), trim(&c.name, &c.input));
            }
        }
        spec
    }

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
        let (result, tokens_saved) = time_based_micro_compact(&messages, 1, 1, 0, &mut std::collections::HashMap::new(), &spec(&messages));
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
        let (tb, _) = time_based_micro_compact(&convo, 1, 1, 0, &mut std::collections::HashMap::new(), &spec(&convo));
        let tb_read = &tb[2];
        assert!(tb_read.content.contains("line 1 of the pasted document"), "time-based kept real text: {:?}", &tb_read.content[..60.min(tb_read.content.len())]);
        assert!(!tb_read.content.is_empty() && tb_read.content != "[cleared]");
        let tr: Vec<serde_json::Value> = serde_json::from_str(tb_read.tool_results.as_deref().unwrap()).unwrap();
        assert!(tr[0]["content"].as_str().unwrap().contains("line 1 of"), "payload rendered into tool_results too");

        // Stage 2 (micro-compact): the summary must count the real lines, never "0 lines".
        let (mc, _) = micro_compact(&convo, 0, &mut std::collections::HashMap::new(), &spec(&convo));
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

        let (result, _) = time_based_micro_compact(&messages, 1, 1, 0, &mut std::collections::HashMap::new(), &spec(&messages));
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
        let (_, tokens_saved) = time_based_micro_compact(&messages, 1, 300, 0, &mut std::collections::HashMap::new(), &spec(&messages));
        assert_eq!(tokens_saved, 0, "active session should not be compacted");
    }

    /// Five identical search results (a re-run search loop) collapse to the
    /// first copy plus one-line stubs — including the "most recent" ones the
    /// keep-recent protection would otherwise hold whole.
    #[test]
    fn test_micro_compact_collapses_identical_results() {
        let big = "search result ".repeat(600); // ~8K chars
        let mut messages = Vec::new();
        for i in 0..5 {
            let mut assistant = make_old_msg("assistant", "searching");
            assistant.tool_calls = Some(
                serde_json::json!([{"name": "web", "id": format!("call_{i}"), "input": {"action": "search", "query": "same"}}]).to_string(),
            );
            messages.push(assistant);
            // The runner appends its redundancy note to repeats; the copy must still match.
            let text = if i == 0 { big.clone() } else { format!("{big}{REDUNDANT_RESULT_NOTE} You already have this content.)") };
            messages.push(make_tool_result_msg(&text, 1000));
        }
        let mut frozen = std::collections::HashMap::new();
        let (result, saved) = micro_compact(&messages, 1_000, &mut frozen, &spec(&messages));
        let stubs = result.iter().filter(|m| m.content.starts_with(DUPLICATE_RESULT_STUB_PREFIX)).count();
        assert_eq!(stubs, 4, "every copy after the first is a stub");
        let original_id = first_tool_call_id(&messages[1]).unwrap();
        assert!(result[3].content.contains(&original_id), "a stub names the call it repeats: {}", result[3].content);
        assert!(result[1].content.starts_with("search result"), "the first copy stays whole");
        assert!(saved > 4 * 1500, "saved {saved} tokens");
        assert_eq!(frozen.len(), 4, "each stub is frozen on its tool_call_id");
        // Second pass over the compacted history is a no-op for the stubs.
        let (again, _) = micro_compact(&result, 1_000, &mut frozen, &spec(&result));
        assert_eq!(again.iter().filter(|m| m.content.starts_with(DUPLICATE_RESULT_STUB_PREFIX)).count(), 4);
    }

    #[test]
    fn test_micro_compact_keeps_only_the_two_newest_screenshots() {
        let shot = |i: usize| {
            let mut m = make_tool_result_msg(&format!("Tapped Simulator at ({i},{i})"), 1000 + i as i64);
            m.tool_results = Some(
                serde_json::json!([{
                    "tool_call_id": format!("call_{i}"),
                    "content": format!("Tapped Simulator at ({i},{i})"),
                    "image_url": format!("data:image/jpeg;base64,{}", "A".repeat(16_000)),
                }])
                .to_string(),
            );
            m
        };
        let messages = vec![make_old_msg("user", "drive the app"), shot(0), shot(1), shot(2)];
        let mut frozen = std::collections::HashMap::new();
        let (result, saved) = micro_compact(&messages, 1_000, &mut frozen, &spec(&messages));
        assert!(!result_has_image(&result[1]), "the oldest screenshot is dropped");
        assert!(result[1].content.ends_with(IMAGE_REMOVED_NOTE), "and says so: {}", result[1].content);
        assert!(result[1].content.starts_with("Tapped Simulator at (0,0)"), "the step's text stays");
        assert!(result_has_image(&result[2]) && result_has_image(&result[3]), "the two newest keep theirs");
        // The image is ~4K tokens; the note it leaves behind costs a few dozen.
        assert!(saved >= 16_000 / crate::CHARS_PER_TOKEN - 100, "saved {saved} tokens");
        // Idempotent: a second pass over the compacted history changes nothing.
        let (again, _) = micro_compact(&result, 1_000, &mut frozen, &spec(&result));
        assert_eq!(again[1].content, result[1].content);
        assert_eq!(again[1].tool_results, result[1].tool_results);
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
            // Distinct per call: identical results are collapsed by the
            // duplicate pass and would never reach the summary path.
            messages.push(make_tool_result_msg(&format!("{big}{i}"), 1000));
        }

        // Threshold below the ~16K estimated total so the pressure gate opens.
        let (result, tokens_saved) = micro_compact(&messages, 1_000, &mut std::collections::HashMap::new(), &spec(&messages));
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

        let (result, saved) = micro_compact(&messages, 100_000, &mut std::collections::HashMap::new(), &spec(&messages));
        assert_eq!(saved, 0, "micro_compact must not fire under the threshold");
        assert!(
            result.iter().all(|m| !m.content.contains("[search_emails]")),
            "no result may be summarized under the threshold"
        );

        let (_, tb_saved) = time_based_micro_compact(&messages, 1, 1, 100_000, &mut std::collections::HashMap::new(), &spec(&messages));
        assert_eq!(
            tb_saved, 0,
            "stale-session clear must not fire under the threshold"
        );
    }

    /// Once a rendering is chosen for a tool_use_id it NEVER changes within
    /// the run — even if the underlying message would render differently on a
    /// later pass. Re-deciding per iteration is how the model watched its own
    /// history mutate mid-run (the outage's delivery mechanism).
    /// Eight large shell results past the keep-recent floor and the count
    /// trigger, so every stage has something to do (fixture floor: N+1).
    fn big_history() -> Vec<ChatMessage> {
        let big = "line\n".repeat(1200);
        let mut convo = vec![tmsg("user", "go", None, None)];
        for i in 0..8 {
            let calls = format!(
                r#"[{{"id":"c{i}","name":"os","input":{{"action":"exec","command":"cargo build {i}"}}}}]"#
            );
            let results = format!(r#"[{{"tool_call_id":"c{i}","content":"{}","is_error":false}}]"#, "x".repeat(600));
            convo.push(tmsg("assistant", "", Some(&calls), None));
            convo.push(tmsg("tool", &big, None, Some(&results)));
        }
        pad_past_compaction(&mut convo);
        convo
    }

    /// The freeze survives the run: a second run on a fresh map loaded from
    /// the store renders the same id the same way even though the underlying
    /// content (and therefore a fresh decision) changed. This is the
    /// reference's transcript-persisted `replacements` map.
    #[test]
    fn a_rendering_is_frozen_per_chat_across_runs() {
        let dir = tempfile::tempdir().unwrap();
        let store = db::Store::new(&dir.path().join("t.db").to_string_lossy()).unwrap();
        store.create_chat("chat-1", "t").unwrap();
        let mut convo = big_history();

        // Run 1: decide, then persist what was decided (the runner's step).
        let mut run1 = store.get_chat_renderings("chat-1").unwrap();
        let (out1, saved) = micro_compact(&convo, 0, &mut run1, &spec(&convo));
        assert!(saved > 0);
        let new: Vec<(String, String)> = run1.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        store.insert_chat_renderings("chat-1", &new).unwrap();

        // Between runs the bytes behind c3 change; a fresh decision would differ.
        for m in convo.iter_mut() {
            if m.tool_results.as_deref().is_some_and(|t| t.contains("\"c1\"")) {
                m.content = "totally different\n".repeat(1500);
                m.tool_results = Some(format!(
                    r#"[{{"tool_call_id":"c1","content":"{}","is_error":false}}]"#,
                    "changed\\n".repeat(50)
                ));
            }
        }
        // Run 2: a fresh runner loads the frozen map and must agree with run 1.
        let mut run2 = store.get_chat_renderings("chat-1").unwrap();
        let (out2, _) = micro_compact(&convo, 0, &mut run2, &spec(&convo));
        let idx = convo.iter().position(|m| m.tool_results.as_deref().is_some_and(|t| t.contains("\"c1\""))).unwrap();
        assert_eq!(out2[idx].tool_results, out1[idx].tool_results, "c1 renders as it first did");
        // And without the store, run 2 would have decided differently (the test can fail).
        let mut cold = std::collections::HashMap::new();
        let (out3, _) = micro_compact(&convo, 0, &mut cold, &spec(&convo));
        assert_ne!(out3[idx].tool_results, out1[idx].tool_results, "a cold decision differs");
    }

    /// A result that returned bytes is never rendered as "0 lines".
    #[test]
    fn a_compacted_view_never_shows_zero_lines_for_a_result_whose_bytes_exist() {
        let history = big_history();
        let mut frozen = std::collections::HashMap::new();
        let (view, saved) = micro_compact(&history, 0, &mut frozen, &spec(&history));
        assert!(saved > 0, "the pipeline must have acted for this test to mean anything");
        for (orig, rendered) in history.iter().zip(view.iter()) {
            if !tool_result_text(orig).trim().is_empty() {
                assert!(!tool_result_text(rendered).contains(" 0 lines"), "{}", tool_result_text(rendered));
            }
        }
    }

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
        let (out1, saved) = micro_compact(&convo, 1_000, &mut frozen, &spec(&convo));
        assert!(saved > 0, "the test must actually compact something");
        let first_rendering = out1[2].content.clone();

        // Mutate the underlying content — a fresh decision would now differ.
        convo[2].content = "totally different\n".repeat(1500);
        let (out2, _) = micro_compact(&convo, 1_000, &mut frozen, &spec(&convo));
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
        let source = include_str!("trim.rs");
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
        let summary = build_tool_summary("os", Some(&input), result, keeps("os", Some(&input)));
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
        let summary = build_tool_summary("os", Some(&input), result, keeps("os", Some(&input)));
        assert_eq!(summary, result, "read-type result content must survive");
        assert!(!summary.contains("lines"));
    }

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

        let (compacted, saved) = micro_compact(&convo, 1_000, &mut std::collections::HashMap::new(), &spec(&convo));
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
            // Distinct per call: byte-identical results collapse to one copy.
            tmsg("tool", "", None, Some(&serde_json::json!([{"tool_call_id":"c1","content":format!("{big}c1")}]).to_string())),
            tmsg("tool", "", None, Some(&serde_json::json!([{"tool_call_id":"c2","content":format!("{big}c2")}]).to_string())),
            tmsg("tool", "", None, Some(&serde_json::json!([{"tool_call_id":"c3","content":format!("{big}c3")}]).to_string())),
        ];
        pad_past_compaction(&mut convo);
        let (out, saved) = micro_compact(&convo, 1_000, &mut std::collections::HashMap::new(), &spec(&convo));
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
        let (out, saved) = micro_compact(&convo, 1_000, &mut std::collections::HashMap::new(), &spec(&convo));
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
        let (out, _) = time_based_micro_compact(&convo, 1, 1, 0, &mut std::collections::HashMap::new(), &spec(&convo));
        assert_ne!(out[2].content, "[cleared]", "a read must not be wiped");
        assert!(out[2].content.contains("def f()"), "read keeps content: {}", out[2].content);
    }

    /// Names that are not registered tools get no special treatment, so a
    /// half-finished rename can never leave a silently dead arm again.
    #[test]
    fn dead_tool_names_get_no_special_treatment() {
        // (the retired os name is covered by the source-grep guard above; naming it here
        // would trip that guard.)
        for dead in ["file", "shell", "bot"] {
            assert_eq!(trim(dead, &serde_json::json!({})), Trim::default(), "{dead} has no spec of its own");
            let s = build_tool_summary(dead, Some(&serde_json::json!({"action":"x"})), "a\nb\n", keeps(dead, Some(&serde_json::json!({"action":"x"}))));
            assert!(!s.starts_with(&format!("[{dead}:")), "{dead} must fall to the catch-all: {s}");
        }
    }

    /// A stub states what happened and how to recover; it never reads as the
    /// tool's answer. `[os] 0 lines` read as "the tool returned nothing".
    #[test]
    fn trimmed_stub_states_what_happened() {
        let s = build_tool_summary("custom_tool", Some(&serde_json::json!({})), "a\nb\nc\n", keeps("custom_tool", Some(&serde_json::json!({}))));
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
        let summary = build_tool_summary("os", Some(&input), result, keeps("os", Some(&input)));

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
        let summary = build_tool_summary("os", Some(&input), "a\nb\nc\n", keeps("os", Some(&input)));
        assert!(
            !summary.starts_with("[os] "),
            "an exec must not fall to the unidentified catch-all: {summary}"
        );
    }

    #[test]
    fn test_build_tool_summary_calendar_preserves_content() {
        // Calendar reads were collapsing to "[os:calendar] 0 lines" — the bug.
        // Now the real content must be preserved.
        let input = serde_json::json!({
            "resource": "calendar",
            "action": "today"
        });
        let result = "9:00 Standup\n13:00 Lunch with client\n15:30 Design review";
        let summary = build_tool_summary("os", Some(&input), result, keeps("os", Some(&input)));
        assert_eq!(summary, result);
        assert!(summary.contains("Lunch with client"));
    }

    #[test]
    fn test_build_tool_summary_read_type_bounded() {
        // Large read-type content is bounded with a truncation marker.
        let input = serde_json::json!({ "resource": "mail", "action": "unread" });
        let result = "x".repeat(10_000);
        let summary = build_tool_summary("os", Some(&input), &result, keeps("os", Some(&input)));
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
        let summary = build_tool_summary("web", Some(&input), result, keeps("web", Some(&input)));
        assert_eq!(summary, result);
        assert!(summary.contains("Tokio Guide"));
    }

    #[test]
    fn test_build_tool_summary_fallback() {
        let input = serde_json::json!({});
        let result = "some output\n";
        let summary = build_tool_summary("custom_tool", Some(&input), result, keeps("custom_tool", Some(&input)));
        assert!(summary.starts_with("[custom_tool]"));
        assert!(summary.contains("lines"));
    }
}
