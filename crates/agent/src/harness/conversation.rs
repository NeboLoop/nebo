//! The conversation a step sends: loaded since the last checkpoint,
//! sanitized, converted, with mid-turn input framed.

use std::collections::{HashMap, HashSet};

use ai::Message;
use db::models::ChatMessage;
use tracing::{debug, info, warn};

use crate::harness::tool_round::ToolResultRow;
use crate::session::SessionManager;

/// Stand-in for a tool_use whose result is missing from history (strict
/// providers reject an unmatched tool_use).
///
/// NOT wrapped as a `<system-reminder>`: that tag is the ephemeral message-stream
/// channel (`steering::wrap_system_reminder`, never persisted — CHAT_SYSTEM §4.2)
/// and this is a persisted tool-role message. It only has to be honest and
/// unmistakable: the old `[Tool result unavailable]` read like the TOOL reporting
/// failure, and a model that concludes its tools are failing stops trusting the
/// ones that work — 11 of these landed in the 2026-08-28 loop.
const ORPHANED_TOOL_RESULT: &str = "(this call's result is missing from the \
conversation history — it was trimmed to fit. This is NOT a tool failure and \
says nothing about whether the call succeeded. Make the call again if you still \
need the result.)";

/// What an interrupted tool call's result says: the call did not finish, and
/// the model must not retry it on its own initiative. Mirrors Claude Code's
/// "[Request interrupted by user for tool use]".
pub const INTERRUPTED_TOOL_RESULT: &str = "[Request interrupted by user for tool use]";

/// The line the thread carries after a stop. The model reads it (the next
/// turn starts from the owner's words, not from the interrupted step); the
/// owner does not (isMeta — the chat already shows the stop).
pub const INTERRUPT_MESSAGE: &str = "[Request interrupted by user] The owner stopped this work. \
Do not resume the interrupted step on your own; wait for their next message and act on that.";

/// Stop means stop, and the record must say so. A cancel can land after the
/// assistant's tool calls were persisted and before their results were; left
/// alone, the next turn's history sanitizer fills each gap with the
/// trimmed-history note, which tells the model to make the call again — and
/// it did, resuming the very search the owner had just stopped, three times
/// in a row (2026-09-18). Each open call gets an interrupt result and the
/// thread gets one interrupt line.
pub(crate) fn record_interrupt(sessions: &SessionManager, session_id: &str) {
    let messages = match sessions.get_messages(session_id) {
        Ok(m) => m,
        Err(e) => {
            warn!(session_id, error = %e, "could not load the thread to record the interrupt");
            return;
        }
    };
    let mut open: Vec<String> = Vec::new();
    if let Some(last) = messages.iter().rposition(|m| m.role == "assistant") {
        let issued: Vec<String> = messages[last]
            .tool_calls
            .as_deref()
            .and_then(|tc| serde_json::from_str::<Vec<serde_json::Value>>(tc).ok())
            .map(|calls| {
                calls
                    .iter()
                    .filter_map(|c| c.get("id").and_then(|v| v.as_str()).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let answered: HashSet<String> = messages[last + 1..]
            .iter()
            .filter(|m| m.role == "tool")
            .filter_map(|m| m.tool_results.as_deref())
            .filter_map(|tr| serde_json::from_str::<Vec<serde_json::Value>>(tr).ok())
            .flatten()
            .filter_map(|r| r.get("tool_call_id").and_then(|v| v.as_str()).map(str::to_string))
            .collect();
        open = issued.into_iter().filter(|id| !answered.contains(id)).collect();
    }
    for id in &open {
        let row = ToolResultRow {
            tool_call_id: id.clone(),
            content: INTERRUPTED_TOOL_RESULT.to_string(),
            is_error: true,
            image_url: None,
            payload: None,
            outcome: Some("Interrupted".to_string()),
            duration_ms: None,
        };
        let tr_json = serde_json::json!([row]).to_string();
        if let Err(e) = sessions.append_message(session_id, "tool", "", None, Some(&tr_json), None) {
            warn!(session_id, error = %e, "could not record an interrupted tool call");
        }
    }
    let meta = serde_json::json!({ "isMeta": true }).to_string();
    if let Err(e) = sessions.append_message(session_id, "user", INTERRUPT_MESSAGE, None, None, Some(&meta)) {
        warn!(session_id, error = %e, "could not record the interrupt line");
    }
    info!(session_id, open_calls = open.len(), "interrupt recorded");
}

/// Who a message queued into a running turn came from. Both senders store
/// their words as typed with this mark (`metadata`); the loop hears the row at
/// its next step (`mid_turn_message_landed`), and the model reads it framed
/// for its sender (`frame_mid_turn_message`). One queue, two senders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MidTurnFrom {
    /// The owner typed it while the turn ran; `via` is the channel.
    Owner { via: String },
    /// The employee that handed this sub-agent its task sent it through
    /// `agent(task, send)`. The sender's session and the task are recorded so
    /// the thread shows where it came from; the sender's taint rides along
    /// into the run that hears it.
    Parent { session_key: String, task_id: String, taint: Vec<types::provenance::ProvenanceClass> },
}

impl MidTurnFrom {
    /// The row metadata that marks a message as queued into a running turn.
    pub fn metadata(&self) -> String {
        match self {
            Self::Owner { via } => serde_json::json!({ "arrivedMidTurn": true, "via": via }),
            Self::Parent { session_key, task_id, taint } => {
                let mut meta = serde_json::json!({
                    "arrivedMidTurn": true,
                    "from": "parent",
                    "parentSessionKey": session_key,
                    "taskId": task_id,
                });
                if !taint.is_empty() {
                    meta["provenance"] = serde_json::json!(taint);
                }
                meta
            }
        }
        .to_string()
    }
}

/// Who sent a message that arrived while a turn ran, if it is one (the mark
/// `MidTurnFrom::metadata` writes).
pub(crate) fn arrived_mid_turn(msg: &ChatMessage) -> Option<MidTurnFrom> {
    let meta: serde_json::Value = serde_json::from_str(msg.metadata.as_deref()?).ok()?;
    if meta.get("arrivedMidTurn").and_then(|v| v.as_bool()) != Some(true) {
        return None;
    }
    let text = |key: &str| meta.get(key).and_then(|v| v.as_str()).unwrap_or_default().to_string();
    if meta.get("from").and_then(|v| v.as_str()) == Some("parent") {
        return Some(MidTurnFrom::Parent {
            session_key: text("parentSessionKey"),
            task_id: text("taskId"),
            taint: meta
                .get("provenance")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default(),
        });
    }
    Some(MidTurnFrom::Owner { via: meta.get("via").and_then(|v| v.as_str()).unwrap_or("chat").to_string() })
}

/// True when a message queued into this turn landed after `seen`, the history
/// a step was built from: that step never had it.
pub(crate) fn mid_turn_message_landed(fresh: &[ChatMessage], seen: &[ChatMessage]) -> bool {
    let last_seen = seen.last().map(|m| m.id.as_str());
    fresh
        .iter()
        .rev()
        .take_while(|m| last_seen != Some(m.id.as_str()))
        .any(|m| m.role == "user" && arrived_mid_turn(m).is_some())
}

/// The taint the parent's messages in this thread carry: the run that reads
/// them has read the parent's content.
pub(crate) fn parent_taint(messages: &[ChatMessage]) -> Vec<types::provenance::ProvenanceClass> {
    messages
        .iter()
        .filter_map(|m| match arrived_mid_turn(m) {
            Some(MidTurnFrom::Parent { taint, .. }) => Some(taint),
            _ => None,
        })
        .flatten()
        .collect()
}

/// True when the parent's latest message to this sub-agent has no model step
/// after it. The loop hears a message that lands before it ends
/// (`mid_turn_message_landed`), so after a turn has ended a parent row with
/// no assistant row after it arrived too late for that turn.
pub(crate) fn parent_message_unheard(messages: &[ChatMessage]) -> bool {
    let Some(at) = messages
        .iter()
        .rposition(|m| m.role == "user" && matches!(arrived_mid_turn(m), Some(MidTurnFrom::Parent { .. })))
    else {
        return false;
    };
    !messages[at + 1..].iter().any(|m| m.role == "assistant")
}

/// True while the owner's latest mid-turn message has no worded reply after
/// it. An assistant row that only calls tools (narration or not) is not a
/// reply; the model is still on its old plan. A parent's message never makes
/// the next step a reply: the sub-agent's report is its answer.
pub(crate) fn unanswered_mid_turn_message(messages: &[ChatMessage]) -> bool {
    let Some(at) = messages
        .iter()
        .rposition(|m| m.role == "user" && matches!(arrived_mid_turn(m), Some(MidTurnFrom::Owner { .. })))
    else {
        return false;
    };
    !messages[at + 1..].iter().any(is_worded_reply)
}

/// An assistant row that answers in words. One that only calls tools
/// (narration or not) is not a reply; the model is still on its old plan.
fn is_worded_reply(m: &ChatMessage) -> bool {
    m.role == "assistant"
        && !m.content.trim().is_empty()
        && m.tool_calls.as_deref().is_none_or(|tc| tc.is_empty() || tc == "[]" || tc == "null")
}

/// How a message the owner typed mid-turn reads to the model. Claude Code's
/// framing, plus that the owner is waiting and the next step is the reply:
/// a changed instruction takes effect now, and the interrupted plan is not
/// continued past it.
///
/// A parent's message is framed as the parent's, not the owner's: it adds to
/// or changes the task, the sub-agent keeps working, and its report is the
/// answer — the parent is not waiting on a reply in between.
pub(crate) fn frame_mid_turn_message(words: &str, from: &MidTurnFrom) -> String {
    match from {
        MidTurnFrom::Owner { via } => format!(
            "The owner sent a new message while you were working (via {via}):\n{words}\n\n\
             IMPORTANT: reply to the owner now, in words, before any further tool use. If this \
             changes what they want, act on the new instruction and do not continue the interrupted \
             plan. If they asked you to continue or to add something, say so in one line; the work \
             resumes at your next step. They are waiting."
        ),
        MidTurnFrom::Parent { .. } => format!(
            "The employee who gave you this task sent you a message while you were working:\n\
             {words}\n\n\
             Take it into the task now. If it changes what they want, follow the new instruction and \
             drop the part of your plan it replaces; if it adds something, fold it in. Keep working \
             with your tools and do not delegate it; your final report goes back to them as usual."
        ),
    }
}

pub(crate) fn convert_messages(messages: &[ChatMessage]) -> Vec<Message> {
    // A message the owner typed mid-turn is framed until it is answered in
    // words; after that it is only their words (steering is per turn).
    let mut answered = vec![false; messages.len()];
    let mut reply_seen = false;
    for (i, m) in messages.iter().enumerate().rev() {
        answered[i] = reply_seen;
        reply_seen |= is_worded_reply(m);
    }
    messages
        .iter()
        .enumerate()
        .filter_map(|(i, msg)| {
            // Skip empty messages
            if msg.content.is_empty()
                && msg.tool_calls.as_ref().is_none_or(|tc| tc.is_empty())
                && msg.tool_results.as_ref().is_none_or(|tr| tr.is_empty())
            {
                return None;
            }

            let tool_calls = msg.tool_calls.as_ref().and_then(|tc| {
                if tc.is_empty() || tc == "[]" || tc == "null" {
                    None
                } else {
                    serde_json::from_str::<serde_json::Value>(tc).ok()
                }
            });

            let tool_results = msg.tool_results.as_ref().and_then(|tr| {
                if tr.is_empty() || tr == "[]" || tr == "null" {
                    None
                } else {
                    serde_json::from_str::<serde_json::Value>(tr).ok()
                }
            });

            let meta = msg
                .metadata
                .as_ref()
                .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok());
            // A picture the owner attached is stored once, as an attachment;
            // read it back from the upload store so the model sees it again on
            // every later turn. `images` is the older shape (rows written
            // before attachments carried an id) and rows that still have it.
            let from_attachments: Vec<ai::ImageContent> = meta
                .as_ref()
                .and_then(|v| v.get("attachments").cloned())
                .and_then(|v| serde_json::from_value::<Vec<comm::wire::Attachment>>(v).ok())
                .unwrap_or_default()
                .iter()
                .filter_map(crate::uploads::image)
                .collect();
            let images = if from_attachments.is_empty() {
                meta.as_ref()
                    .and_then(|v| v.get("images").cloned())
                    .and_then(|v| serde_json::from_value::<Vec<ai::ImageContent>>(v).ok())
            } else {
                Some(from_attachments)
            };
            // A message the owner sent while the turn was running is stored as
            // their words; until it is answered the model gets it framed: it
            // arrived mid-work and they are waiting on it.
            let content = match arrived_mid_turn(msg) {
                Some(from) if !answered[i] => frame_mid_turn_message(&msg.content, &from),
                _ => msg.content.clone(),
            };

            Some(Message {
                role: msg.role.clone(),
                content,
                tool_calls,
                tool_results,
                images,
            })
        })
        .collect()
}

/// Sanitize message ordering: ensure tool results immediately follow their
/// corresponding assistant message. Self-heals corrupted session data
/// (back-to-back assistants, out-of-order tool results) that strict providers
/// like GPT-5-mini reject. Also strips orphaned tool results that reference
/// tool_call_ids not found in any preceding assistant message (matches Go's
/// sanitizeAgentMessages).
pub(crate) fn sanitize_message_order(messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
    if messages.is_empty() {
        return messages;
    }

    // Phase 1: Collect all tool_call_ids issued by assistant messages
    let mut issued_call_ids = HashSet::new();
    for msg in &messages {
        if msg.role == "assistant"
            && let Some(ref tc_json) = msg.tool_calls
            && let Ok(calls) = serde_json::from_str::<Vec<serde_json::Value>>(tc_json)
        {
            for call in &calls {
                if let Some(id) = call.get("id").and_then(|v| v.as_str()) {
                    issued_call_ids.insert(id.to_string());
                }
            }
        }
    }

    // Phase 2: Map tool_call_id → tool result message for reordering.
    // Each tool message in DB has a single-element tool_results array.
    // Track which message indices are tool messages to skip in output.
    let mut tool_result_map: HashMap<String, ChatMessage> = HashMap::new();
    let mut tool_msg_indices = HashSet::new();
    let mut orphaned = 0u32;

    for (i, msg) in messages.iter().enumerate() {
        if msg.role != "tool" {
            continue;
        }
        if let Some(ref tr_json) = msg.tool_results
            && let Ok(results) = serde_json::from_str::<Vec<serde_json::Value>>(tr_json)
        {
            let mut valid_results = Vec::new();
            for r in &results {
                let tcid = r.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or("");
                if tcid.is_empty() || !issued_call_ids.contains(tcid) {
                    orphaned += 1;
                    continue;
                }
                valid_results.push((tcid.to_string(), r.clone()));
            }

            if !valid_results.is_empty() {
                tool_msg_indices.insert(i);
                for (tcid, result_val) in valid_results {
                    let single_tr = serde_json::json!([result_val]).to_string();
                    tool_result_map.insert(
                        tcid,
                        ChatMessage {
                            id: msg.id.clone(),
                            chat_id: msg.chat_id.clone(),
                            role: "tool".to_string(),
                            content: msg.content.clone(),
                            metadata: msg.metadata.clone(),
                            created_at: msg.created_at,
                            day_marker: msg.day_marker.clone(),
                            tool_calls: None,
                            tool_results: Some(single_tr),
                            token_estimate: msg.token_estimate,
                            html: None,
                        },
                    );
                }
            } else if orphaned > 0 {
                // All results in this message were orphaned — skip entire message
                tool_msg_indices.insert(i);
            }
        }
    }

    // Phase 3: Rebuild with tool results injected after their assistant
    let mut result = Vec::with_capacity(messages.len());
    let mut reordered = 0u32;
    let mut orphaned_uses = 0u32;

    for (i, msg) in messages.into_iter().enumerate() {
        if tool_msg_indices.contains(&i) {
            continue;
        }
        let has_tool_calls = msg.role == "assistant" && msg.tool_calls.is_some();
        let tc_json = msg.tool_calls.clone();
        result.push(msg);

        if has_tool_calls
            && let Some(ref tc) = tc_json
            && let Ok(calls) = serde_json::from_str::<Vec<serde_json::Value>>(tc)
        {
            for call in &calls {
                if let Some(id) = call.get("id").and_then(|v| v.as_str()) {
                    if let Some(tool_msg) = tool_result_map.remove(id) {
                        reordered += 1;
                        result.push(tool_msg);
                    } else {
                        // Orphaned tool_use: no matching tool_result exists.
                        // Inject a synthetic result so strict providers
                        // (Anthropic, GPT) don't reject the conversation.
                        orphaned_uses += 1;
                        let synthetic = serde_json::json!([{
                            "tool_call_id": id,
                            "content": ORPHANED_TOOL_RESULT
                        }]);
                        result.push(ChatMessage {
                            id: String::new(),
                            chat_id: String::new(),
                            role: "tool".to_string(),
                            content: ORPHANED_TOOL_RESULT.to_string(),
                            metadata: None,
                            created_at: chrono::Utc::now().timestamp(),
                            day_marker: None,
                            tool_calls: None,
                            tool_results: Some(synthetic.to_string()),
                            token_estimate: Some(0),
                            html: None,
                        });
                    }
                }
            }
        }
    }

    if reordered > 0 {
        debug!(
            reordered,
            "reordered tool results for correct message ordering"
        );
    }
    if orphaned > 0 {
        debug!(orphaned, "stripped orphaned tool results");
    }
    if orphaned_uses > 0 {
        debug!(
            orphaned_uses,
            "injected synthetic results for orphaned tool_use blocks"
        );
    }
    // Drop any remaining unmatched results — they're double orphans
    let unmatched = tool_result_map.len();
    if unmatched > 0 {
        debug!(unmatched, "dropped unmatched tool results");
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_msg(id: &str, role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            id: id.into(),
            chat_id: "c".into(),
            role: role.into(),
            content: content.into(),
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
    fn test_convert_messages() {
        let messages = vec![
            ChatMessage {
                id: "1".into(),
                chat_id: "c".into(),
                role: "user".into(),
                content: "hello".into(),
                metadata: None,
                created_at: 0,
                day_marker: None,
                tool_calls: None,
                tool_results: None,
                token_estimate: None,
                html: None,
            },
            ChatMessage {
                id: "2".into(),
                chat_id: "c".into(),
                role: "assistant".into(),
                content: "hi there".into(),
                metadata: None,
                created_at: 0,
                day_marker: None,
                tool_calls: None,
                tool_results: None,
                token_estimate: None,
                html: None,
            },
        ];

        let result = convert_messages(&messages);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].role, "user");
        assert_eq!(result[1].role, "assistant");
    }

    /// A mid-turn message is stored as the owner typed it and framed for the
    /// model only; an ordinary message is passed through untouched.
    #[test]
    fn mid_turn_message_is_framed_for_the_model_only() {
        let row = |content: &str, metadata: Option<&str>| ChatMessage {
            id: "m".into(),
            chat_id: "c".into(),
            role: "user".into(),
            content: content.into(),
            metadata: metadata.map(str::to_string),
            created_at: 0,
            day_marker: None,
            tool_calls: None,
            tool_results: None,
            token_estimate: None,
            html: None,
        };
        let plain = convert_messages(&[row("stop searching and tell me", None)]);
        assert_eq!(plain[0].content, "stop searching and tell me");
        let queued = convert_messages(&[row(
            "stop searching and tell me",
            Some(r#"{"arrivedMidTurn":true,"via":"web"}"#),
        )]);
        assert!(queued[0].content.starts_with("The owner sent a new message while you were working (via web):\nstop searching and tell me"), "{}", queued[0].content);
        assert!(queued[0].content.contains("They are waiting"));
        // Unanswered until a worded reply follows it; a tool-calling row is not one.
        let mid = row("stop reading", Some(r#"{"arrivedMidTurn":true,"via":"web"}"#));
        let mut narrating = row("Reading part 3.", None);
        narrating.role = "assistant".into();
        narrating.tool_calls = Some(r#"[{"id":"c1","name":"os","input":{}}]"#.into());
        let mut reply = row("So far: Northwind, March.", None);
        reply.role = "assistant".into();
        assert!(unanswered_mid_turn_message(&[mid.clone()]));
        assert!(unanswered_mid_turn_message(&[mid.clone(), narrating.clone()]));
        assert!(!unanswered_mid_turn_message(&[mid.clone(), narrating.clone(), reply.clone()]));
        assert!(!unanswered_mid_turn_message(&[row("hello", None)]));
        // The framing is steering: it rides only until the message is answered.
        // Every later turn reads the owner's words alone.
        let pending = convert_messages(&[mid.clone(), narrating.clone()]);
        assert!(pending[0].content.starts_with("The owner sent a new message"), "{}", pending[0].content);
        let answered = convert_messages(&[mid, narrating, reply]);
        assert_eq!(answered[0].content, "stop reading");
    }

    /// A parent employee's message to its running sub-agent is stored as
    /// sent, marked with where it came from and the parent's taint, and read
    /// by the model as the parent's — never as the owner waiting on a reply.
    #[test]
    fn a_parents_mid_turn_message_is_its_own_and_asks_for_no_reply() {
        let row = |id: &str, role: &str, content: &str, metadata: Option<String>| ChatMessage {
            id: id.into(),
            chat_id: "c".into(),
            role: role.into(),
            content: content.into(),
            metadata,
            created_at: 0,
            day_marker: None,
            tool_calls: None,
            tool_results: None,
            token_estimate: None,
            html: None,
        };
        let from = MidTurnFrom::Parent {
            session_key: "agent:ops:web".into(),
            task_id: "sa-1".into(),
            taint: vec![types::provenance::ProvenanceClass::Web],
        };
        let meta = from.metadata();
        let v: serde_json::Value = serde_json::from_str(&meta).unwrap();
        assert_eq!(v["from"], "parent");
        assert_eq!(v["parentSessionKey"], "agent:ops:web");
        assert_eq!(v["taskId"], "sa-1");
        assert_eq!(v["provenance"], serde_json::json!(["web"]));
        let owner = MidTurnFrom::Owner { via: "web".into() }.metadata();
        assert_eq!(owner, r#"{"arrivedMidTurn":true,"via":"web"}"#, "the owner's mark is unchanged");

        let msg = row("p", "user", "also cover pricing", Some(meta));
        assert_eq!(arrived_mid_turn(&msg), Some(from));
        let framed = &convert_messages(std::slice::from_ref(&msg))[0].content;
        assert!(framed.starts_with("The employee who gave you this task sent you a message"), "{framed}");
        assert!(framed.contains("also cover pricing"));
        assert!(!framed.contains("owner") && !framed.contains("They are waiting"), "{framed}");
        assert_eq!(parent_taint(std::slice::from_ref(&msg)), vec![types::provenance::ProvenanceClass::Web]);
        assert!(parent_taint(&[row("o", "user", "hi", Some(owner.clone()))]).is_empty());

        // The owner's rule (the next step is a reply in words) never fires
        // for a parent's message: the sub-agent's report is its answer.
        assert!(!unanswered_mid_turn_message(std::slice::from_ref(&msg)));
        assert!(unanswered_mid_turn_message(&[row("o", "user", "stop", Some(owner))]));

        // Unheard until a model step follows it.
        let step = row("a", "assistant", "done", None);
        assert!(parent_message_unheard(std::slice::from_ref(&msg)));
        assert!(!parent_message_unheard(&[msg.clone(), step.clone()]));
        assert!(parent_message_unheard(&[step.clone(), msg.clone()]));
        assert!(!parent_message_unheard(&[row("u", "user", "task", None), step.clone()]));

        // A queued message that landed after the history a step was built
        // from was not in that step; one it was built with was.
        let seen = vec![row("u", "user", "task", None), step.clone()];
        assert!(mid_turn_message_landed(&[seen[0].clone(), step.clone(), msg.clone()], &seen));
        assert!(!mid_turn_message_landed(&[seen[0].clone(), msg.clone(), step.clone()], &[seen[0].clone(), msg.clone(), step]));
        assert!(!mid_turn_message_landed(&seen, &seen));
    }

    #[test]
    fn test_sanitize_preserves_correct_order() {
        // Already correct: assistant → tool → assistant → tool
        let msg1 = make_msg("1", "user", "hello");
        let mut msg2 = make_msg("2", "assistant", "let me help");
        msg2.tool_calls = Some(r#"[{"id":"call_1","name":"web","input":{}}]"#.into());
        let mut msg3 = make_msg("3", "tool", "");
        msg3.tool_results =
            Some(r#"[{"tool_call_id":"call_1","content":"result","is_error":false}]"#.into());
        let msg4 = make_msg("4", "assistant", "done");

        let result = sanitize_message_order(vec![msg1, msg2, msg3, msg4]);
        assert_eq!(result.len(), 4);
        assert_eq!(result[0].role, "user");
        assert_eq!(result[1].role, "assistant");
        assert_eq!(result[2].role, "tool");
        assert_eq!(result[3].role, "assistant");
    }

    #[test]
    fn test_sanitize_reorders_back_to_back_assistants() {
        // Broken: assistant, assistant, tool(for #2), tool(for #1)
        let msg1 = make_msg("1", "user", "hello");
        let mut msg2 = make_msg("2", "assistant", "calling web");
        msg2.tool_calls = Some(r#"[{"id":"call_A","name":"web","input":{}}]"#.into());
        let mut msg3 = make_msg("3", "assistant", "calling system");
        msg3.tool_calls = Some(r#"[{"id":"call_B","name":"system","input":{}}]"#.into());
        let mut msg4 = make_msg("4", "tool", "");
        msg4.tool_results =
            Some(r#"[{"tool_call_id":"call_B","content":"sys result","is_error":false}]"#.into());
        let mut msg5 = make_msg("5", "tool", "");
        msg5.tool_results =
            Some(r#"[{"tool_call_id":"call_A","content":"web result","is_error":false}]"#.into());

        let result = sanitize_message_order(vec![msg1, msg2, msg3, msg4, msg5]);

        // Expected: user, assistant(A), tool(A), assistant(B), tool(B)
        assert_eq!(result.len(), 5);
        assert_eq!(result[0].role, "user");
        assert_eq!(result[1].role, "assistant"); // call_A
        assert_eq!(result[2].role, "tool"); // result for call_A
        assert!(result[2].tool_results.as_ref().unwrap().contains("call_A"));
        assert_eq!(result[3].role, "assistant"); // call_B
        assert_eq!(result[4].role, "tool"); // result for call_B
        assert!(result[4].tool_results.as_ref().unwrap().contains("call_B"));
    }

    #[test]
    fn test_sanitize_strips_orphaned_tool_results() {
        // Tool result references a call_id that no assistant ever issued
        let msg1 = make_msg("1", "user", "hello");
        let mut msg2 = make_msg("2", "tool", "");
        msg2.tool_results = Some(
            r#"[{"tool_call_id":"call_ORPHAN","content":"orphaned","is_error":false}]"#.into(),
        );
        let msg3 = make_msg("3", "assistant", "hi");

        let result = sanitize_message_order(vec![msg1, msg2, msg3]);
        // Orphaned tool message should be stripped
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].role, "user");
        assert_eq!(result[1].role, "assistant");
    }
}
