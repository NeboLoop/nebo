//! What an MCP App's view told the employee (`ui/update-model-context`):
//! the latest update from each view in a conversation, kept until the
//! conversation's next step and told then, once, when it changed.
//!
//! Each update replaces that view's earlier one (the spec's "each request
//! overwrites the previous context sent by the View"). Kept in memory: a
//! view tells it again the next time it is shown.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

/// Each conversation's views' latest words, by session key, then by the
/// call that showed the view.
#[derive(Default)]
pub struct ViewContexts {
    by_session: Mutex<HashMap<String, BTreeMap<String, String>>>,
}

impl ViewContexts {
    /// The view shown by call `view` in conversation `session_key` says
    /// `text` now; empty text withdraws what it said.
    pub fn set(&self, session_key: &str, view: &str, text: &str) {
        let mut all = self.by_session.lock().unwrap_or_else(|e| e.into_inner());
        let views = all.entry(session_key.to_string()).or_default();
        if text.trim().is_empty() {
            views.remove(view);
        } else {
            views.insert(view.to_string(), text.trim().to_string());
        }
        if views.is_empty() {
            all.remove(session_key);
        }
    }

    /// What the conversation's views say now, one paragraph per view, in
    /// the order their calls ran; empty when none says anything.
    pub fn text(&self, session_key: &str) -> String {
        let all = self.by_session.lock().unwrap_or_else(|e| e.into_inner());
        all.get(session_key).map(|views| views.values().cloned().collect::<Vec<_>>().join("\n\n")).unwrap_or_default()
    }
}

/// The words of one `ui/update-model-context` request: its text blocks,
/// then its structured content as JSON.
pub fn update_text(params: &serde_json::Value) -> String {
    let mut parts: Vec<String> = params
        .get("content")
        .and_then(|c| c.as_array())
        .into_iter()
        .flatten()
        .filter_map(|b| match b.get("type").and_then(|t| t.as_str()) {
            Some("text") => b.get("text").and_then(|t| t.as_str()).map(str::to_string),
            _ => Some(b.to_string()),
        })
        .collect();
    if let Some(structured) = params.get("structuredContent").filter(|v| !v.is_null()) {
        parts.push(serde_json::to_string_pretty(structured).unwrap_or_default());
    }
    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn each_views_update_replaces_its_last_and_empty_withdraws_it() {
        let c = ViewContexts::default();
        c.set("s", "call-1", "zoomed to Paris");
        c.set("s", "call-2", "form half filled");
        c.set("s", "call-1", "zoomed to Rome");
        assert_eq!(c.text("s"), "zoomed to Rome\n\nform half filled");
        c.set("s", "call-2", "");
        assert_eq!(c.text("s"), "zoomed to Rome");
        assert_eq!(c.text("other"), "");
    }

    #[test]
    fn an_update_is_its_text_then_its_structured_content() {
        let p = json!({"content": [{"type": "text", "text": "Viewing Paris"}], "structuredContent": {"zoom": 12}});
        assert_eq!(update_text(&p), "Viewing Paris\n{\n  \"zoom\": 12\n}");
    }
}
