//! The per-step trim, and clearing old tool results under context
//! pressure, as Claude Code 2.1.280 does (`XBr`/`c5n`, m0460; its trigger is
//! the context hint, m1040): when the request is due for a checkpoint, the
//! results of calls whose output can be got again (a file read or change, a
//! command, a web search or fetch: each tool says so, `DynTool::clearable`)
//! are cleared first, all but the five most recent, and only when that saves
//! at least 20k tokens. Each cleared result is saved through the one spill
//! path and its place says where; a result with an image just reads as
//! cleared. Everything else stays whole until a checkpoint.
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

/// The tool call ids whose results may be cleared under pressure.
pub type Clearable = HashSet<String>;

/// Clearable results kept whole, newest first.
pub const KEEP_RECENT: usize = 5;
/// Clearing runs only when it saves at least this many tokens.
pub const MIN_TOKENS_SAVED: usize = 20_000;
/// What a cleared result reads when it isn't saved (it carried an image, or
/// saving it failed).
pub const CLEARED: &str = "[Old tool result content cleared]";

/// Image-bearing results that keep their image, newest first.
const KEEP_RECENT_IMAGES: usize = 2;
/// What a result whose image was dropped says in its place.
const IMAGE_CLEARED: &str = "[Old screenshot cleared]";

/// Apply every frozen rendering to `messages` and drop all but the newest
/// screenshots. Returns the trimmed conversation and the tokens saved.
pub fn trim(messages: &[ChatMessage], frozen: &Frozen) -> (Vec<ChatMessage>, usize) {
    let mut out = messages.to_vec();
    let mut saved = 0;

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

/// Clear old results under context pressure: every clearable result but
/// the [`KEEP_RECENT`] newest, when that saves at least
/// [`MIN_TOKENS_SAVED`]. A result already cleared or already saved to a
/// file is left as it is. `save` saves one result's text and returns what
/// the model sees in its place (`None`: it couldn't be saved). Each
/// rendering is frozen; returns the tokens saved, 0 when nothing was
/// cleared.
pub fn clear_old_results(
    messages: &[ChatMessage],
    clearable: &Clearable,
    frozen: &mut Frozen,
    mut save: impl FnMut(&str) -> Option<String>,
) -> usize {
    let (trimmed, _) = trim(messages, frozen);
    let results: Vec<(String, &ChatMessage)> = trimmed
        .iter()
        .filter_map(|m| result_call_id(m).map(|id| (id, m)))
        .filter(|(id, _)| clearable.contains(id))
        .collect();
    let keep = results.len().saturating_sub(KEEP_RECENT);
    let candidates: Vec<&(String, &ChatMessage)> = results[..keep]
        .iter()
        .filter(|(id, m)| !frozen.contains_key(id) && !already_saved(m))
        .collect();
    let saved: usize = candidates.iter().map(|(_, m)| estimate_message_tokens(m)).sum();
    if saved < MIN_TOKENS_SAVED {
        return 0;
    }
    for (id, msg) in candidates {
        let rendering = if has_image(msg) { None } else { save(&result_text(msg)) };
        frozen.insert(id.clone(), rendering.unwrap_or_else(|| CLEARED.to_string()));
    }
    saved
}

/// Whether a result already reads as cleared or saved to a file.
fn already_saved(msg: &ChatMessage) -> bool {
    let text = result_text(msg);
    text == CLEARED || text.starts_with("<persisted-output>")
}

/// The text of a result row's results.
fn result_text(msg: &ChatMessage) -> String {
    result_rows(msg)
        .iter()
        .filter_map(|r| r.get("content").and_then(|c| c.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
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

    /// `n` calls, each answered with a 4 KB result, stored at `at`.
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

    /// Saves every result it is handed as `saved:<n>`.
    fn saving() -> impl FnMut(&str) -> Option<String> {
        let mut n = 0;
        move |_text: &str| {
            n += 1;
            Some(format!("saved:{n}"))
        }
    }

    /// Below the pressure floor nothing is cleared: clearing that saves less
    /// than 20k tokens doesn't run, and the conversation is sent whole.
    #[test]
    fn clearing_that_saves_little_does_not_run() {
        let msgs = history(12, 1);
        let mut frozen = Frozen::new();
        assert_eq!(clear_old_results(&msgs, &all(12), &mut frozen, saving()), 0);
        assert!(frozen.is_empty());
        let (out, saved) = trim(&msgs, &frozen);
        assert_eq!(saved, 0);
        assert_eq!(serde_json::to_string(&out).unwrap(), serde_json::to_string(&msgs).unwrap());
    }

    /// Under pressure every clearable result but the five newest is saved
    /// and replaced by where it was saved; the call ids and the error flag
    /// stay, and results a tool keeps are never cleared.
    #[test]
    fn under_pressure_all_but_the_five_newest_clearable_results_clear() {
        let msgs = history(40, 1);
        let mut clearable = all(40);
        clearable.remove("c1");
        let mut frozen = Frozen::new();
        let saved = clear_old_results(&msgs, &clearable, &mut frozen, saving());
        assert!(saved >= MIN_TOKENS_SAVED, "{saved}");
        let (out, _) = trim(&msgs, &frozen);
        let tools: Vec<&ChatMessage> = out.iter().filter(|m| m.role == "tool").collect();
        assert_eq!(content(tools[0]), "saved:1");
        assert_eq!(result_rows(tools[0])[0]["is_error"], true, "a failure still reads as one");
        assert_eq!(result_rows(tools[0])[0]["tool_call_id"], "c0");
        assert_eq!(content(tools[1]).len(), 4000, "a result its tool keeps stays whole");
        assert_eq!(content(tools[2]), "saved:2");
        for t in &tools[35..] {
            assert_eq!(content(t).len(), 4000, "the five newest stay whole");
        }
        assert_eq!(frozen.len(), 34);
        // A result that couldn't be saved reads as cleared.
        let mut frozen = Frozen::new();
        clear_old_results(&msgs, &all(40), &mut frozen, |_: &str| None);
        assert_eq!(frozen["c0"], CLEARED);
    }

    /// A cleared result stays cleared on every later step, and clearing
    /// again touches only results that have since fallen out of the newest
    /// five: the prefix sent to the cache never changes.
    #[test]
    fn a_cleared_result_stays_cleared() {
        let mut msgs = history(40, 1);
        let mut frozen = Frozen::new();
        clear_old_results(&msgs, &all(40), &mut frozen, saving());
        let first = frozen.clone();
        msgs.extend(history(40, 2).into_iter().skip(1).map(|mut m| {
            m.tool_calls = m.tool_calls.map(|c| c.replace("\":\"c", "\":\"d"));
            m.tool_results = m.tool_results.map(|r| r.replace("\":\"c", "\":\"d"));
            m
        }));
        let ids: Clearable = all(40).into_iter().chain((0..40).map(|i| format!("d{i}"))).collect();
        clear_old_results(&msgs, &ids, &mut frozen, saving());
        for (id, rendering) in &first {
            assert_eq!(&frozen[id], rendering, "{id} changed after it was sent");
        }
        let (out, _) = trim(&msgs, &frozen);
        let tools: Vec<&ChatMessage> = out.iter().filter(|m| m.role == "tool").collect();
        assert_eq!(content(tools[35]), "saved:1", "a result that fell out of the newest five clears next time");
        assert_eq!(content(tools[79]).len(), 4000);
    }

    /// Only the two newest screenshots keep their image; the text stays.
    #[test]
    fn only_the_two_newest_screenshots_keep_their_image() {
        let mut msgs = vec![row("user", 1, None, None)];
        for i in 0..4 {
            let results = serde_json::json!([{ "tool_call_id": format!("s{i}"), "content": format!("step {i}"), "image_url": "/api/v1/files/s.png" }]).to_string();
            msgs.push(row("tool", 1, None, Some(results)));
        }
        let (out, saved) = trim(&msgs, &Frozen::new());
        let images: Vec<bool> = out.iter().skip(1).map(has_image).collect();
        assert_eq!(images, vec![false, false, true, true]);
        assert_eq!(content(&out[1]), format!("step 0\n{IMAGE_CLEARED}"));
        assert!(saved > 0);
    }
}
