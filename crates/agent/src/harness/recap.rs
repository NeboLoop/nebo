//! The owner recap: one or two plain sentences written after a chat turn,
//! for the owner coming back to the thread — never read back into a model
//! request. Turn-Controller Technical Design §2.7 / WP2.5.
//!
//! The turn's `Finish` spawns `write_recap` for turns the owner is in
//! (their own chat from the app, the phone or their loop) that weren't
//! cancelled, so it never delays the turn's own reply; a scheduled or other
//! unattended turn gets none. The recap shows under the turn and as the
//! employee's status line (the stored recap and the `turn_recap` event); it
//! is never an owner notification, so it adds no Inbox row and no unread
//! badge.

use std::sync::Arc;

use ai::{ChatRequest, Message, Provider, RequestTrace, StreamEventType};
use serde_json::json;

use crate::concurrency::ConcurrencyController;

/// The owner-facing instruction, in our own words (Turn-Controller
/// Technical Design §2.7). Never mentions internals or markdown. The call
/// offers the turn's tools only so its prefix stays the turn's; the reply
/// is its text.
pub const RECAP_INSTRUCTION: &str = "The owner is coming back to this thread. \
In one or two plain sentences under 40 words: the overall goal and where it \
stands, then the single next action. Reply in plain text only; don't call any tool.";

/// Hard character cap on the stored/emitted recap (§2.7).
pub const RECAP_CHAR_CAP: usize = 400;

/// What one recap call runs against: the finished turn's own last request,
/// forked for the prompt cache. Never mutated, and never itself stored into
/// the conversation.
pub struct RecapRequest {
    pub chat_id: String,
    pub turn_id: String,
    /// The turn's last request, extended by what was stored after it (its
    /// answer): system prompt, tools, conversation, model, cache breakpoints
    /// and trace (agent and run ids) as the turn sent them, so the call
    /// reads the turn's cached prefix. The aux route would miss it.
    pub fork_of: ChatRequest,
    /// The provider that answered the turn's last call.
    pub provider: Arc<dyn Provider>,
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

/// One forked call over the turn's own request: the same prefix with the
/// instruction appended, so the model and provider agree on the cached
/// prefix (§2.7, "reusing the turn's prompt cache"). One step; the reply's
/// text is the recap, nothing else reads or retries it.
async fn call_for_recap(req: &RecapRequest) -> Option<String> {
    let mut messages = req.fork_of.messages.clone();
    messages.push(Message {
        role: "user".to_string(),
        content: RECAP_INSTRUCTION.to_string(),
        ..Default::default()
    });
    let chat_req = ChatRequest {
        messages,
        trace: RequestTrace {
            purpose: "owner_recap",
            ..req.fork_of.trace.clone()
        },
        ..req.fork_of.clone()
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
            fork_of: ChatRequest {
                messages: vec![Message {
                    role: "user".to_string(),
                    content: "Find last quarter's top three clients by revenue.".to_string(),
                    ..Default::default()
                }],
                tools: vec![ai::ToolDefinition {
                    name: "search_web".to_string(),
                    description: "Search the web".to_string(),
                    input_schema: serde_json::json!({"type": "object"}),
                }],
                system: "You are a helpful employee.".to_string(),
                model: "anthropic/claude".to_string(),
                cache_breakpoints: vec![10],
                ..ChatRequest::new(RequestTrace {
                    agent_id: "agent-1".to_string(),
                    run_id: "turn-1".to_string(),
                    ..RequestTrace::new("agent_turn")
                })
            },
            provider,
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

    /// The recap call is the turn's own request with the ONE instruction
    /// line appended: the same system prompt, tools, model and cache
    /// breakpoints, the conversation as its prefix, and the turn's agent
    /// and run ids on the trace. Anything else would miss the turn's cache.
    #[tokio::test]
    async fn recap_call_is_the_turns_request_plus_the_instruction() {
        let (_dir, s) = store();
        let store = Arc::new(s);
        let concurrency = Arc::new(ConcurrencyController::new(Some(4)));
        let provider = Arc::new(ScriptedProvider::new("Working the client list; next, total the revenue."));
        let turn = req(provider.clone());
        let fork_of = turn.fork_of.clone();

        write_recap(store, concurrency, None, turn).await;

        let sent = provider.sent.lock().unwrap();
        assert_eq!(sent.len(), 1, "one forked, one-step call");
        let sent_req = &sent[0];
        assert_eq!(sent_req.system, fork_of.system);
        assert_eq!(sent_req.model, fork_of.model);
        assert_eq!(sent_req.cache_breakpoints, fork_of.cache_breakpoints);
        let names = |r: &ChatRequest| r.tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(sent_req), names(&fork_of), "the turn's tools, so the prefix is the turn's");
        assert_eq!(sent_req.messages.len(), fork_of.messages.len() + 1);
        for (a, b) in sent_req.messages.iter().zip(&fork_of.messages) {
            assert_eq!((&a.role, &a.content), (&b.role, &b.content), "the conversation is the prefix");
        }
        assert_eq!(
            sent_req.messages.last().unwrap().content,
            RECAP_INSTRUCTION,
            "the instruction is the final message, never folded into history"
        );
        assert_eq!(sent_req.trace.purpose, "owner_recap");
        assert_eq!(sent_req.trace.run_id, "turn-1", "the run it summarises");
        assert_eq!(sent_req.trace.agent_id, "agent-1");
        // The recap text itself never appears back in the request it was
        // produced from.
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
