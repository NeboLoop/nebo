//! The owner recap: one or two plain sentences written after a chat turn,
//! for the owner coming back to the thread — never read back into a model
//! request. Turn-Controller Technical Design §2.7 / WP2.5.
//!
//! The turn's `Finish` spawns `write_recap` for chat turns that weren't
//! cancelled, so it never delays the turn's own reply. The recap shows
//! under the turn and as the employee's status line (the stored recap and
//! the `turn_recap` event); it is never an owner notification, so it adds
//! no Inbox row and no unread badge.

use std::sync::Arc;

use ai::{ChatRequest, Message, Provider, RequestTrace, StreamEventType};
use serde_json::json;

use crate::concurrency::ConcurrencyController;

/// The owner-facing instruction, in our own words (Turn-Controller
/// Technical Design §2.7). Never mentions tools, internals or markdown —
/// the model is given no tools and one step to answer in.
pub const RECAP_INSTRUCTION: &str = "The owner is coming back to this thread. \
In one or two plain sentences under 40 words: the overall goal and where it \
stands, then the single next action.";

/// Hard character cap on the stored/emitted recap (§2.7).
pub const RECAP_CHAR_CAP: usize = 400;

/// What one recap call runs against: the finished turn's own transcript and
/// model, forked and reused for the prompt cache — never mutated, and never
/// itself stored into the conversation.
pub struct RecapRequest {
    pub chat_id: String,
    pub turn_id: String,
    /// The turn's own system prompt, so the forked call shares its cached
    /// prefix instead of paying for a fresh one.
    pub system: String,
    pub cache_breakpoints: Vec<usize>,
    /// The turn's own transcript, ending with its answer.
    pub messages: Vec<Message>,
    /// The turn's own model and provider — reusing the aux/cheap route
    /// would miss the turn's prompt cache entirely.
    pub model: String,
    pub provider: Arc<dyn Provider>,
    pub agent_id: Option<String>,
}

/// Write the recap for the turn just finished. `None` when the call
/// produced nothing usable — best-effort: failures are logged and
/// swallowed, never surfaced to the owner as an error. Runs on the
/// background permit pool so it never competes with a live turn for an LLM
/// slot.
pub async fn write_recap(
    store: Arc<db::Store>,
    concurrency: Arc<ConcurrencyController>,
    broadcast: Option<crate::agent_worker::NotifyFn>,
    req: RecapRequest,
) -> Option<String> {
    let _permit = concurrency.acquire_background_permit().await;

    let text = call_for_recap(&req).await?;
    let capped = cap_chars(&text, RECAP_CHAR_CAP);

    if let Err(e) = store.write_chat_recap(&req.chat_id, &req.turn_id, &capped) {
        tracing::warn!(
            chat_id = %req.chat_id,
            turn_id = %req.turn_id,
            error = %e,
            "recap not persisted; emitting anyway"
        );
    }

    if let Some(f) = &broadcast {
        f(
            "turn_recap",
            json!({
                "chatId": req.chat_id,
                "turnId": req.turn_id,
                "text": capped,
            }),
        );
    }

    Some(capped)
}

/// One forked, tool-less call over the turn's own conversation: reuses its
/// system prompt and cache breakpoints so the model and provider agree on
/// the cached prefix (§2.7 — "reusing the turn's prompt cache"). One step;
/// the reply is the recap, nothing else reads or retries it.
async fn call_for_recap(req: &RecapRequest) -> Option<String> {
    let mut messages = req.messages.clone();
    messages.push(Message {
        role: "user".to_string(),
        content: RECAP_INSTRUCTION.to_string(),
        tool_calls: None,
        tool_results: None,
        images: None,
    });

    let trace = RequestTrace {
        agent_id: req.agent_id.clone().unwrap_or_default(),
        ..RequestTrace::new("owner_recap")
    };
    let chat_req = ChatRequest {
        messages,
        max_tokens: 200,
        temperature: 0.3,
        system: req.system.clone(),
        model: req.model.clone(),
        cache_breakpoints: req.cache_breakpoints.clone(),
        // No tools offered — the recap call answers in plain text, one step.
        ..ChatRequest::new(trace)
    };

    let mut rx = match req.provider.stream(&chat_req).await {
        Ok(rx) => rx,
        Err(e) => {
            tracing::warn!(error = %e, "recap provider call failed");
            return None;
        }
    };

    let mut text = String::new();
    while let Some(event) = rx.recv().await {
        match event.event_type {
            StreamEventType::Text => text.push_str(&event.text),
            StreamEventType::Done | StreamEventType::Error => break,
            _ => {}
        }
    }

    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

/// Truncate to at most `cap` chars, on a char boundary — a byte-length cut
/// could split a multi-byte character apart.
fn cap_chars(s: &str, cap: usize) -> String {
    if s.chars().count() <= cap {
        s.to_string()
    } else {
        s.chars().take(cap).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai::{EventReceiver, ProviderError};
    use async_trait::async_trait;

    /// A provider that streams back one fixed reply and records every
    /// request it was sent — the scripted double `recap_never_enters_a_request`
    /// asserts over.
    struct ScriptedProvider {
        reply: String,
        sent: std::sync::Mutex<Vec<ChatRequest>>,
    }

    impl ScriptedProvider {
        fn new(reply: &str) -> Self {
            Self { reply: reply.to_string(), sent: std::sync::Mutex::new(Vec::new()) }
        }
    }

    #[async_trait]
    impl Provider for ScriptedProvider {
        fn id(&self) -> &str {
            "scripted"
        }

        async fn stream(&self, req: &ChatRequest) -> Result<EventReceiver, ProviderError> {
            self.sent.lock().unwrap().push(req.clone());
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            let text = self.reply.clone();
            tokio::spawn(async move {
                let _ = tx.send(ai::StreamEvent::text(text)).await;
                let _ = tx.send(ai::StreamEvent::done()).await;
            });
            Ok(rx)
        }
    }

    fn store() -> (tempfile::TempDir, db::Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nebo-recap-test.db");
        let s = db::Store::new(&path.to_string_lossy()).expect("store");
        s.create_chat("chat-1", "Test chat").expect("create chat");
        (dir, s)
    }

    fn req(provider: Arc<ScriptedProvider>) -> RecapRequest {
        RecapRequest {
            chat_id: "chat-1".to_string(),
            turn_id: "turn-1".to_string(),
            system: "You are a helpful employee.".to_string(),
            cache_breakpoints: vec![10],
            messages: vec![Message {
                role: "user".to_string(),
                content: "Find last quarter's top three clients by revenue.".to_string(),
                tool_calls: None,
                tool_results: None,
                images: None,
            }],
            model: "anthropic/claude".to_string(),
            provider,
            agent_id: Some("agent-1".to_string()),
        }
    }

    #[tokio::test]
    async fn recap_is_stored_and_emitted() {
        let (_dir, s) = store();
        let store = Arc::new(s);
        let concurrency = Arc::new(ConcurrencyController::new(Some(4)));
        let provider = Arc::new(ScriptedProvider::new(
            "Finding last quarter's top clients; next, pull the revenue totals.",
        ));

        let emitted = Arc::new(std::sync::Mutex::new(Vec::new()));
        let emitted_clone = emitted.clone();
        let broadcast: crate::agent_worker::NotifyFn = Arc::new(move |ev, payload| {
            emitted_clone.lock().unwrap().push((ev.to_string(), payload));
        });

        let result = write_recap(store.clone(), concurrency, Some(broadcast), req(provider)).await;

        let text = result.expect("a recap was written");
        assert!(!text.is_empty());

        let stored = store.get_chat_recap("chat-1", "turn-1").unwrap().expect("row persisted");
        assert_eq!(stored.text, text);

        let events = emitted.lock().unwrap();
        assert!(
            events.iter().any(|(ev, payload)| ev == "turn_recap"
                && payload["chatId"] == "chat-1"
                && payload["turnId"] == "turn-1"
                && payload["text"] == text),
            "turn_recap was broadcast with the stored text: {events:?}"
        );
        // Never an owner notification: no Inbox row, no unread badge.
        assert!(
            !events.iter().any(|(ev, _)| ev == "notification_created"),
            "a recap is not a notification: {events:?}"
        );
        let owner = store.ensure_local_user_id().unwrap_or_default();
        assert!(store.get_notification("recap:turn-1", &owner).unwrap().is_none(), "no Inbox row");
        assert_eq!(store.count_unread_notifications(&owner).unwrap(), 0, "the badge doesn't climb");
    }

    #[tokio::test]
    async fn recap_over_cap_is_truncated() {
        let (_dir, s) = store();
        let store = Arc::new(s);
        let concurrency = Arc::new(ConcurrencyController::new(Some(4)));
        let long_reply = "a".repeat(RECAP_CHAR_CAP + 250);
        let provider = Arc::new(ScriptedProvider::new(&long_reply));

        let result = write_recap(store, concurrency, None, req(provider)).await;
        let text = result.expect("a recap was written");
        assert_eq!(text.chars().count(), RECAP_CHAR_CAP);
    }

    #[tokio::test]
    async fn empty_reply_writes_nothing() {
        let (_dir, s) = store();
        let store = Arc::new(s);
        let concurrency = Arc::new(ConcurrencyController::new(Some(4)));
        let provider = Arc::new(ScriptedProvider::new("   "));

        let result = write_recap(store.clone(), concurrency, None, req(provider)).await;
        assert!(result.is_none());
        assert!(store.get_chat_recap("chat-1", "turn-1").unwrap().is_none());
    }

    /// The recap call's request is the turn's own transcript plus the ONE
    /// instruction line — never widened with recap machinery — and offers
    /// no tools. This is the module-level half of `recap_never_enters_a_request`;
    /// the turn-level half (the *next* turn's request never carries a past
    /// recap) is asserted by WP2.3's scripted-provider suite once
    /// `drive_turn` exists, over the fixture `recap-never-in-context`.
    #[tokio::test]
    async fn recap_call_offers_no_tools_and_the_recap_never_reenters_it() {
        let (_dir, s) = store();
        let store = Arc::new(s);
        let concurrency = Arc::new(ConcurrencyController::new(Some(4)));
        let provider = Arc::new(ScriptedProvider::new("Working the client list; next, total the revenue."));

        write_recap(store, concurrency, None, req(provider.clone())).await;

        let sent = provider.sent.lock().unwrap();
        assert_eq!(sent.len(), 1, "one forked, one-step call");
        let sent_req = &sent[0];
        assert!(sent_req.tools.is_empty(), "no tools offered to the recap call");
        assert!(
            sent_req.messages.iter().all(|m| !m.content.contains(RECAP_INSTRUCTION)
                || m.role == "user" && m.content == RECAP_INSTRUCTION),
            "the instruction rides once, as the last user turn"
        );
        assert_eq!(
            sent_req.messages.last().unwrap().content,
            RECAP_INSTRUCTION,
            "the instruction is the final message, never folded into history"
        );
        // The recap text itself never appears back in the request it was
        // produced from, or in any request this call sends.
        for m in &sent_req.messages {
            assert!(!m.content.contains("Working the client list"));
        }
    }

    #[test]
    fn cap_chars_cuts_on_a_char_boundary() {
        let s = "a".repeat(10) + "€"; // multi-byte char at the boundary
        let capped = cap_chars(&s, 10);
        assert_eq!(capped.chars().count(), 10);
        assert_eq!(capped, "a".repeat(10));
    }

    #[test]
    fn cap_chars_leaves_short_text_alone() {
        assert_eq!(cap_chars("short", 400), "short");
    }
}
