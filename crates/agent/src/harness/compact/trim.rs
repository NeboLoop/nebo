//! The per-step trim, Claude Code's time-based microcompact: once the
//! conversation has sat idle long enough that the provider's prompt cache
//! has gone cold (an hour since the last reply), the results of calls whose
//! output can be got again (file reads and changes, searches, commands, web
//! searches and fetches: each tool says so, `DynTool::cleared_when_stale`)
//! are cleared, all but the five most recent. Everything else stays whole
//! until a checkpoint.
//!
//! Every rendering is frozen the first time it is chosen, keyed on the tool
//! call id and persisted per chat, and applied on every later step, so the
//! prompt prefix never changes under the cache once it has been sent.
//!
//! Screenshots are the one Nebo addition: a UI drive returns a picture on
//! every step (about 1.5k tokens each) and only the newest ones say anything
//! about the screen as it is now, so all but the two newest lose their
//! image. The text of the result stays.

use std::collections::{HashMap, HashSet};

use db::models::ChatMessage;

use crate::pruning::{IMAGE_CHAR_ESTIMATE, estimate_message_tokens};

/// Frozen renderings, keyed on the tool call id: the first rendering chosen
/// for a result is the rendering forever (persisted per chat).
pub type Frozen = HashMap<String, String>;

/// The tool call ids whose results may be cleared once stale.
pub type Clearable = HashSet<String>;

/// Idle time after which stale results are cleared.
pub const STALE_AFTER_SECS: i64 = 60 * 60;
/// Clearable results kept whole, newest first.
pub const KEEP_RECENT: usize = 5;
/// What a cleared result reads.
pub const CLEARED: &str = "[Old tool result content cleared]";

/// Image-bearing results that keep their image, newest first.
const KEEP_RECENT_IMAGES: usize = 2;
/// What a result whose image was dropped says in its place.
const IMAGE_CLEARED: &str = "[Old screenshot cleared]";

/// Trim `messages` as of `now` (unix seconds). Returns the trimmed
/// conversation and the tokens saved; `frozen` gains every rendering chosen.
pub fn trim(messages: &[ChatMessage], now: i64, clearable: &Clearable, frozen: &mut Frozen) -> (Vec<ChatMessage>, usize) {
    let mut out = messages.to_vec();
    let mut saved = 0;

    let last_reply = messages.iter().rev().find(|m| m.role == "assistant").map(|m| m.created_at);
    let stale = last_reply.is_some_and(|at| at > 0 && now - at >= STALE_AFTER_SECS);
    if stale {
        let ids: Vec<String> = messages
            .iter()
            .rev()
            .filter_map(result_call_id)
            .filter(|id| clearable.contains(id))
            .skip(KEEP_RECENT)
            .collect();
        for id in ids {
            frozen.entry(id).or_insert_with(|| CLEARED.to_string());
        }
    }

    for msg in out.iter_mut() {
        let Some(id) = result_call_id(msg) else { continue };
        let Some(rendering) = frozen.get(&id) else { continue };
        let before = estimate_message_tokens(msg);
        replace_result(msg, rendering);
        saved += before.saturating_sub(estimate_message_tokens(msg));
    }

    let mut with_images: Vec<usize> = out.iter().enumerate().filter(|(_, m)| has_image(m)).map(|(i, _)| i).collect();
    with_images.truncate(with_images.len().saturating_sub(KEEP_RECENT_IMAGES));
    for i in with_images {
        saved += drop_images(&mut out[i]) * IMAGE_CHAR_ESTIMATE / crate::CHARS_PER_TOKEN;
    }

    (out, saved)
}

/// The call id a tool-result row answers (its first result's).
fn result_call_id(msg: &ChatMessage) -> Option<String> {
    let rows: Vec<serde_json::Value> = serde_json::from_str(msg.tool_results.as_deref()?).ok()?;
    let id = rows.first()?.get("tool_call_id")?.as_str()?;
    (!id.is_empty()).then(|| id.to_string())
}

fn result_rows(msg: &ChatMessage) -> Vec<serde_json::Value> {
    msg.tool_results
        .as_deref()
        .and_then(|tr| serde_json::from_str(tr).ok())
        .unwrap_or_default()
}

/// Replace every result's content with `text`, keeping the call ids and
/// error flags so the result still pairs with its call and a failure still
/// reads as one.
fn replace_result(msg: &mut ChatMessage, text: &str) {
    let mut rows = result_rows(msg);
    for row in rows.iter_mut().filter_map(|r| r.as_object_mut()) {
        row.insert("content".into(), text.into());
        row.remove("image_url");
    }
    msg.tool_results = serde_json::to_string(&rows).ok();
    msg.content = text.to_string();
    msg.token_estimate = Some((text.len() / crate::CHARS_PER_TOKEN) as i64);
}

fn has_image(msg: &ChatMessage) -> bool {
    result_rows(msg)
        .iter()
        .any(|r| r.get("image_url").and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty()))
}

/// Drop every image of `msg`'s results, noting it in their text. Returns how
/// many were dropped.
fn drop_images(msg: &mut ChatMessage) -> usize {
    let mut rows = result_rows(msg);
    let mut dropped = 0;
    for row in rows.iter_mut().filter_map(|r| r.as_object_mut()) {
        if row.remove("image_url").and_then(|v| v.as_str().map(|s| !s.is_empty())).unwrap_or(false) {
            dropped += 1;
            let text = row.get("content").and_then(|c| c.as_str()).unwrap_or("");
            let noted = if text.is_empty() { IMAGE_CLEARED.to_string() } else { format!("{text}\n{IMAGE_CLEARED}") };
            row.insert("content".into(), noted.into());
        }
    }
    if dropped > 0 {
        msg.tool_results = serde_json::to_string(&rows).ok();
    }
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 60 * 60;

    fn row(role: &str, created_at: i64, tool_calls: Option<String>, tool_results: Option<String>) -> ChatMessage {
        ChatMessage {
            id: uuid::Uuid::new_v4().to_string(),
            chat_id: "c".into(),
            role: role.into(),
            content: String::new(),
            metadata: None,
            created_at,
            day_marker: None,
            tool_calls,
            tool_results,
            token_estimate: None,
            html: None,
        }
    }

    /// `n` calls, each answered with a 4 KB result, the last reply at `at`.
    fn history(n: usize, at: i64) -> Vec<ChatMessage> {
        let mut out = vec![row("user", at, None, None)];
        for i in 0..n {
            let calls = serde_json::json!([{ "id": format!("c{i}"), "name": "os", "input": {} }]).to_string();
            let results = serde_json::json!([{ "tool_call_id": format!("c{i}"), "content": "x".repeat(4000), "is_error": i == 0 }]).to_string();
            out.push(row("assistant", at, Some(calls), None));
            out.push(row("tool", at, None, Some(results)));
        }
        out
    }

    fn content(msg: &ChatMessage) -> String {
        result_rows(msg)[0]["content"].as_str().unwrap().to_string()
    }

    fn all(n: usize) -> Clearable {
        (0..n).map(|i| format!("c{i}")).collect()
    }

    /// Idle for less than an hour: nothing is touched, however large.
    #[test]
    fn a_live_conversation_is_left_whole() {
        let msgs = history(12, 1_000_000);
        let (out, saved) = trim(&msgs, 1_000_000 + HOUR - 1, &all(12), &mut Frozen::new());
        assert_eq!(saved, 0);
        assert_eq!(serde_json::to_string(&out).unwrap(), serde_json::to_string(&msgs).unwrap());
    }

    /// After an hour idle, clearable results beyond the five newest read
    /// "[Old tool result content cleared]"; the call ids and the error flag
    /// stay, and results a tool keeps are never cleared.
    #[test]
    fn after_an_hour_all_but_the_five_newest_clearable_results_clear() {
        let msgs = history(8, 1_000_000);
        let mut clearable = all(8);
        clearable.remove("c1");
        let mut frozen = Frozen::new();
        let (out, saved) = trim(&msgs, 1_000_000 + HOUR, &clearable, &mut frozen);
        let tools: Vec<&ChatMessage> = out.iter().filter(|m| m.role == "tool").collect();
        assert_eq!(content(tools[0]), CLEARED);
        assert_eq!(result_rows(tools[0])[0]["is_error"], true, "a failure still reads as one");
        assert_eq!(result_rows(tools[0])[0]["tool_call_id"], "c0");
        assert_eq!(content(tools[1]).len(), 4000, "a result its tool keeps stays whole");
        assert_eq!(content(tools[2]), CLEARED);
        for t in &tools[3..] {
            assert_eq!(content(t).len(), 4000, "the five newest stay whole");
        }
        assert!(saved > 0);
        assert_eq!(frozen.len(), 2);
    }

    /// A cleared result stays cleared on every later step, the owner back
    /// or not: the prefix sent to the cache never changes.
    #[test]
    fn a_cleared_result_stays_cleared() {
        let mut msgs = history(7, 1_000_000);
        let mut frozen = Frozen::new();
        trim(&msgs, 1_000_000 + HOUR, &all(7), &mut frozen);
        msgs.push(row("user", 1_000_000 + HOUR, None, None));
        msgs.push(row("assistant", 1_000_000 + HOUR + 5, None, None));
        let (out, _) = trim(&msgs, 1_000_000 + HOUR + 10, &all(7), &mut frozen);
        let tools: Vec<&ChatMessage> = out.iter().filter(|m| m.role == "tool").collect();
        assert_eq!(content(tools[0]), CLEARED);
        assert_eq!(content(tools[1]), CLEARED);
        assert_eq!(content(tools[2]).len(), 4000);
    }

    /// Only the two newest screenshots keep their image; the text stays.
    #[test]
    fn only_the_two_newest_screenshots_keep_their_image() {
        let mut msgs = vec![row("user", 1, None, None)];
        for i in 0..4 {
            let results = serde_json::json!([{ "tool_call_id": format!("s{i}"), "content": format!("step {i}"), "image_url": "/api/v1/files/s.png" }]).to_string();
            msgs.push(row("tool", 1, None, Some(results)));
        }
        let (out, saved) = trim(&msgs, 2, &Clearable::new(), &mut Frozen::new());
        let images: Vec<bool> = out.iter().skip(1).map(has_image).collect();
        assert_eq!(images, vec![false, false, true, true]);
        assert_eq!(content(&out[1]), format!("step 0\n{IMAGE_CLEARED}"));
        assert!(saved > 0);
    }
}
