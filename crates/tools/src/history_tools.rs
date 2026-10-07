//! Past conversations: `search_history` finds messages, `read_session`
//! reads one conversation, `list_sessions` lists them. A Confidential
//! conversation (its memory scope says so) reaches only itself: nothing said
//! in one matter is found from another.

use std::sync::Arc;

use db::Store;
use serde_json::{Value, json};

use crate::memory_tools::MemoryScopeKind;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// Most messages `read_session` returns: the conversation's last ones, or
/// one page from `from`.
const READ_LAST: usize = 50;

/// Most characters of message text on one `read_session` page. A message
/// longer than what is left continues on the next page, so nothing is cut.
// ponytail: one fixed budget sized for a small model's context; per-model if pages crowd one out.
const PAGE_CHARS: usize = 20_000;

pub struct History {
    store: Arc<Store>,
}

impl History {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }

    pub fn tools(self) -> Vec<Box<dyn DynTool>> {
        let history = Arc::new(self);
        [HistoryOp::Search, HistoryOp::Read, HistoryOp::List]
            .into_iter()
            .map(|op| {
                Box::new(HistoryTool {
                    op,
                    history: history.clone(),
                }) as Box<dyn DynTool>
            })
            .collect()
    }

    /// The one conversation a Confidential run may read — its own — or
    /// `None` for every conversation.
    fn confined_to(&self, ctx: &ToolContext) -> Option<String> {
        (MemoryScopeKind::of(&ctx.user_id) == MemoryScopeKind::Confidential)
            .then(|| self.store.resolve_session_chat_id(&ctx.session_id))
    }

    fn search(&self, input: &Value, ctx: &ToolContext) -> ToolResult {
        let query = input["query"].as_str().unwrap_or("");
        let limit = input["limit"].as_i64().unwrap_or(20);
        match self.store.search_chats(query, limit, self.confined_to(ctx).as_deref()) {
            Ok(hits) if hits.is_empty() => {
                ToolResult::ok(format!("No messages found matching: {query}"))
            }
            Ok(hits) => {
                let lines: Vec<String> = hits
                    .iter()
                    .map(|h| {
                        let when = chrono::DateTime::from_timestamp(h.created_at, 0)
                            .map(|t| t.format("%Y-%m-%d").to_string())
                            .unwrap_or_default();
                        let title = if h.chat_title.is_empty() {
                            h.chat_id.clone()
                        } else {
                            h.chat_title.clone()
                        };
                        format!(
                            "- {when} in \"{title}\" (chat {}, message {}), {}: {}",
                            h.chat_id, h.message_id, h.role, h.snippet
                        )
                    })
                    .collect();
                ToolResult::ok(format!(
                    "Found {} messages (best match first):\n{}",
                    lines.len(),
                    lines.join("\n")
                ))
            }
            Err(e) => ToolResult::error(format!("Chat search failed: {e}")),
        }
    }

    /// The chat `id` names: a chat id as `search_history` gives it, or a
    /// session id (messages live under the session's active chat, resolved
    /// by the one derivation in the db crate).
    fn chat_of(&self, id: &str) -> String {
        match self.store.get_chat(id) {
            Ok(Some(chat)) => chat.id,
            _ => self.store.resolve_session_chat_id(id),
        }
    }

    fn read(&self, input: &Value, ctx: &ToolContext) -> ToolResult {
        let own = self.store.resolve_session_chat_id(&ctx.session_id);
        let chat_id = input["session_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map_or_else(|| own.clone(), |id| self.chat_of(id));
        let confined = self.confined_to(ctx);
        if confined.is_some() && chat_id != own {
            return ToolResult::error(
                "This conversation is confidential: only its own messages can be read here. Leave \
                 session_id out to read this one.",
            );
        }
        let from = input["from"].as_str().filter(|s| !s.is_empty());
        let to_file = input["to_file"].as_str().filter(|s| !s.trim().is_empty());
        if to_file.is_some() && confined.is_some() {
            return ToolResult::error(
                "This conversation is confidential: nothing said in it is written out to a file.",
            );
        }
        if from.is_none() && to_file.is_none() {
            return self.read_last(&chat_id);
        }
        // `from`: "start", a message id, or a page cursor `<id>+<offset>`
        // (where a long message continues).
        let (from_id, offset) = match from {
            None | Some("start") => (None, 0),
            Some(cursor) => match cursor.split_once('+') {
                Some((id, at)) => (Some(id), at.parse().unwrap_or(0)),
                None => (Some(cursor), 0),
            },
        };
        if let Some(id) = from_id {
            match self.store.get_chat_message(id) {
                Ok(Some(m)) if m.chat_id == chat_id => {}
                _ => {
                    return ToolResult::error(format!(
                        "No message {id} in this conversation. `from` takes \"start\", a message id \
                         from read_session or search_history, or the cursor a page ends with."
                    ));
                }
            }
        }
        let msgs = match self.store.get_chat_messages_from(&chat_id, from_id) {
            Ok(m) => m,
            Err(e) => return ToolResult::error(format!("Failed to read the session: {e}")),
        };
        let said: Vec<&db::models::ChatMessage> = msgs.iter().filter(|m| said(m)).collect();
        match to_file {
            Some(name) => write_transcript(name, &said, offset),
            None => {
                let limit = input["limit"].as_u64().map_or(READ_LAST, |n| n.max(1) as usize);
                ToolResult::ok(page(&said, offset, limit))
            }
        }
    }

    /// The conversation's last messages, each shortened: an overview whose
    /// ids are where a full read starts.
    fn read_last(&self, chat_id: &str) -> ToolResult {
        match self.store.get_chat_messages(chat_id) {
            Ok(msgs) if msgs.is_empty() => {
                ToolResult::ok(format!("No messages in conversation {chat_id}."))
            }
            Ok(msgs) => {
                let recent: Vec<&db::models::ChatMessage> =
                    msgs.iter().rev().take(READ_LAST).collect();
                let lines: Vec<String> = recent
                    .iter()
                    .rev()
                    .map(|m| {
                        let preview = if m.content.len() > 200 {
                            format!("{}...", crate::truncate_str(&m.content, 200))
                        } else {
                            m.content.clone()
                        };
                        format!("[{}] {}: {preview}", m.id, m.role)
                    })
                    .collect();
                ToolResult::ok(format!(
                    "{} messages in session (showing the last {}, shortened):\n{}\n\nFull text: \
                     read_session with from=\"start\" or from=<message id>.",
                    msgs.len(),
                    recent.len(),
                    lines.join("\n")
                ))
            }
            Err(e) => ToolResult::error(format!("Failed to read the session: {e}")),
        }
    }

    fn list(&self, ctx: &ToolContext) -> ToolResult {
        if self.confined_to(ctx).is_some() {
            return ToolResult::ok(
                "This conversation is confidential: other conversations are not listed here. \
                 read_session with no session_id reads this one.",
            );
        }
        match self.store.list_sessions(50, 0) {
            Ok(sessions) if sessions.is_empty() => ToolResult::ok("No sessions."),
            Ok(sessions) => {
                let lines: Vec<String> = sessions
                    .iter()
                    .map(|s| {
                        format!(
                            "- {} ({}): {} messages",
                            s.id,
                            s.name.as_deref().unwrap_or("-"),
                            s.message_count.unwrap_or(0)
                        )
                    })
                    .collect();
                ToolResult::ok(format!(
                    "{} sessions:\n{}",
                    sessions.len(),
                    lines.join("\n")
                ))
            }
            Err(e) => ToolResult::error(format!("Failed to list sessions: {e}")),
        }
    }
}

/// A message someone said: tool rows and text-less tool-call turns are
/// the work, not the conversation.
fn said(m: &db::models::ChatMessage) -> bool {
    m.role != "tool" && !m.content.trim().is_empty()
}

fn heading(m: &db::models::ChatMessage) -> String {
    let when = chrono::DateTime::from_timestamp(m.created_at, 0)
        .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default();
    format!("{} ({when})", m.role)
}

/// One page of full messages from `offset` (characters into the first),
/// ending with the cursor for the next page or the end.
fn page(msgs: &[&db::models::ChatMessage], offset: usize, limit: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    let mut start = offset;
    for (n, m) in msgs.iter().enumerate() {
        let rest: String = m.content.chars().skip(start).collect();
        let len = rest.chars().count();
        let room = PAGE_CHARS.saturating_sub(used);
        if n == limit || (len > room && n > 0) {
            return format!("{out}Next page: from={}", if start > 0 { format!("{}+{start}", m.id) } else { m.id.clone() });
        }
        let cont = if start > 0 { ", continued" } else { "" };
        if len > room {
            // Longer than a whole page: this part now, the rest next.
            let part: String = rest.chars().take(room).collect();
            out.push_str(&format!("[{}] {}{cont}:\n{part}\n\n", m.id, heading(m)));
            return format!("{out}Next page: from={}+{}", m.id, start + room);
        }
        out.push_str(&format!("[{}] {}{cont}:\n{rest}\n\n", m.id, heading(m)));
        used += len;
        start = 0;
    }
    if out.is_empty() {
        "No messages from there: that is the end of the conversation.".to_string()
    } else {
        format!("{out}End of conversation.")
    }
}

/// The messages, word for word, in a file in the owner's files: exact text
/// no one retypes.
fn write_transcript(name: &str, msgs: &[&db::models::ChatMessage], offset: usize) -> ToolResult {
    let Some(file) = std::path::Path::new(name.trim()).file_name() else {
        return ToolResult::error("to_file is a file name, like transcript.md.");
    };
    let dir = match config::workspace_dir() {
        Ok(d) => d,
        Err(e) => return ToolResult::error(format!("Cannot find the files folder: {e}")),
    };
    let text: String = msgs
        .iter()
        .enumerate()
        .map(|(n, m)| {
            let body: String = m.content.chars().skip(if n == 0 { offset } else { 0 }).collect();
            format!("{}:\n{body}\n\n", heading(m))
        })
        .collect();
    let path = dir.join(file);
    match std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, text.trim_end())) {
        Ok(()) => ToolResult::ok(format!(
            "Wrote {} messages, word for word, to {}. Each is headed by who said it (as stored: user \
             is the owner) and when. Rename those headings with an edit, not by rewriting the text.",
            msgs.len(),
            path.display()
        )),
        Err(e) => ToolResult::error(format!("Couldn't write {}: {e}", path.display())),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HistoryOp {
    Search,
    Read,
    List,
}

struct HistoryTool {
    op: HistoryOp,
    history: Arc<History>,
}

impl DynTool for HistoryTool {
    fn name(&self) -> &str {
        match self.op {
            HistoryOp::Search => "search_history",
            HistoryOp::Read => "read_session",
            HistoryOp::List => "list_sessions",
        }
    }

    fn description(&self) -> String {
        match self.op {
            HistoryOp::Search => "Searches past conversations for messages matching the query, best match first, with the chat each came from.".to_string(),
            HistoryOp::Read => "Reads one conversation (leave `session_id` out for this one). Without `from`: its last messages, shortened. With `from`: full messages, oldest first, a page at a time; each page ends with the `from` for the next one.\n\
- To work through a long conversation, read it page by page, or give helpers page ranges and have them report back what they found.\n\
- When the exact words matter (a transcript, quotes), set `to_file`: the messages are written to a file unchanged, so no one retypes them; work from the file.\n\
- A search_history hit's message id is a `from`.".to_string(),
            HistoryOp::List => "Lists conversations with their message counts.".to_string(),
        }
    }

    fn schema(&self) -> Value {
        match self.op {
            HistoryOp::Search => json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Words to find." },
                    "limit": { "type": "integer", "description": "Most messages to return (default 20)." }
                },
                "required": ["query"]
            }),
            HistoryOp::Read => json!({
                "type": "object",
                "properties": {
                    "session_id": { "type": "string", "description": "The conversation: a session id from list_sessions or a chat id from search_history. Default: this one." },
                    "from": { "type": "string", "description": "Where to start reading full messages: \"start\", a message id, or the cursor the last page ended with." },
                    "limit": { "type": "integer", "description": "Most messages on one page (default 50)." },
                    "to_file": { "type": "string", "description": "A file name, like transcript.md: write every message from `from` (or the start) to the end into it, word for word, instead of returning them." }
                }
            }),
            HistoryOp::List => json!({ "type": "object", "properties": {} }),
        }
    }

    fn search_hint(&self) -> &str {
        match self.op {
            HistoryOp::Search => "search past conversations and messages",
            HistoryOp::Read => "read a conversation's messages",
            HistoryOp::List => "list past conversations",
        }
    }

    fn read_only(&self, _input: &Value) -> bool {
        true
    }

    fn validate_input(&self, input: &Value) -> Result<(), String> {
        if self.op == HistoryOp::Search
            && input["query"].as_str().is_none_or(|q| q.trim().is_empty())
        {
            return Err(
                "query can't be empty. To read a conversation, use read_session.".to_string(),
            );
        }
        Ok(())
    }

    fn activity(&self, _input: &Value) -> String {
        "looking through past conversations".to_string()
    }

    fn outcome(&self, _input: &Value) -> String {
        "Looked through past conversations".to_string()
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            match self.op {
                HistoryOp::Search => self.history.search(&input, ctx),
                HistoryOp::Read => self.history.read(&input, ctx),
                HistoryOp::List => self.history.list(ctx),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_three_tools_read_and_refuse_an_empty_search() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(&dir.path().join("h.db").to_string_lossy()).unwrap());
        let tools = History::new(store).tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert_eq!(names, ["search_history", "read_session", "list_sessions"]);
        assert!(
            tools
                .iter()
                .all(|t| t.read_only(&json!({})) && t.should_defer())
        );
        assert!(tools[0].validate_input(&json!({"query": " "})).is_err());
        let ctx = ToolContext {
            session_id: "s-none".into(),
            ..Default::default()
        };
        let read = tools[1].execute_dyn(&ctx, json!({})).await;
        assert!(read.content.starts_with("No messages"), "{}", read.content);
        assert_eq!(
            tools[2].execute_dyn(&ctx, json!({})).await.content,
            "No sessions."
        );
    }

    // The interview transcript (2026-10-06): 290 messages back, unreadable
    // past the last 50, each cut to 200 characters. A conversation now reads
    // in full, page by page, from its start or any message id — including
    // rows a compaction summary hides from the runner.
    #[tokio::test]
    async fn a_long_conversation_reads_in_full_page_by_page() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(&dir.path().join("h.db").to_string_lossy()).unwrap());
        let say = |id: &str, role: &str, text: &str| {
            store
                .create_chat_message_for_runner(id, "c1", role, text, None, None, None, None, None, None)
                .unwrap();
        };
        let long = "q".repeat(PAGE_CHARS + 5_000);
        say("m1", "user", "In late May of 2025");
        say("m2", "tool", "a tool result");
        store.compact_chat_history("c1", "sum", "summary", None).unwrap();
        say("m3", "assistant", "");
        say("m4", "user", &long);
        say("m5", "assistant", "the end");
        let read = &History::new(store.clone()).tools()[1];
        let ctx = ToolContext { session_id: "s-other".into(), ..Default::default() };
        let page = |input: Value| {
            let read = read;
            let ctx = &ctx;
            async move { read.execute_dyn(ctx, input).await.content }
        };

        let one = page(json!({"session_id": "c1", "from": "start"})).await;
        assert!(one.contains("In late May of 2025") && one.contains("[sum]"), "{one}");
        assert!(!one.contains("a tool result") && !one.contains("[m3]"), "{one}");
        assert!(one.ends_with("Next page: from=m4"), "{one}");

        let two = page(json!({"session_id": "c1", "from": "m4"})).await;
        assert!(two.ends_with(&format!("Next page: from=m4+{PAGE_CHARS}")), "{}", &two[two.len() - 60..]);
        let three = page(json!({"session_id": "c1", "from": format!("m4+{PAGE_CHARS}")})).await;
        assert!(three.contains("continued") && three.contains("the end") && three.ends_with("End of conversation."), "{three}");
        let xs = |p: &str| p.matches('q').count();
        assert_eq!(xs(&two) + xs(&three), long.len(), "the long message, whole, across two pages");

        let limited = page(json!({"session_id": "c1", "from": "start", "limit": 1})).await;
        assert!(limited.ends_with("Next page: from=sum"), "{limited}");
        assert!(page(json!({"session_id": "c1", "from": "nope"})).await.starts_with("No message nope"));
    }
}
