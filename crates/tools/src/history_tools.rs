//! Past conversations: `search_history` finds messages, `read_session`
//! reads one conversation, `list_sessions` lists them.

use std::sync::Arc;

use db::Store;
use serde_json::{Value, json};

use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// Most messages `read_session` returns: the conversation's last ones.
const READ_LAST: usize = 50;

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

    fn search(&self, input: &Value) -> ToolResult {
        let query = input["query"].as_str().unwrap_or("");
        let limit = input["limit"].as_i64().unwrap_or(20);
        match self.store.search_chats(query, limit) {
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
                            "- {when} in \"{title}\" (chat {}), {}: {}",
                            h.chat_id, h.role, h.snippet
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

    /// Messages live under the session's active chat (sessions are decoupled
    /// from chats), resolved by the one derivation in the db crate.
    fn read(&self, input: &Value, ctx: &ToolContext) -> ToolResult {
        let session_id = input["session_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or(&ctx.session_id);
        let chat_id = self.store.resolve_session_chat_id(session_id);
        match self.store.get_chat_messages(&chat_id) {
            Ok(msgs) if msgs.is_empty() => ToolResult::ok(format!(
                "No messages in the active chat of session {session_id}."
            )),
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
                    "{} messages in session (showing the last {}):\n{}",
                    msgs.len(),
                    recent.len(),
                    lines.join("\n")
                ))
            }
            Err(e) => ToolResult::error(format!("Failed to read the session: {e}")),
        }
    }

    fn list(&self) -> ToolResult {
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
            HistoryOp::Read => "Reads the last messages of a conversation. Leave `session_id` out for this one.".to_string(),
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
                    "session_id": { "type": "string", "description": "The conversation's session id, from list_sessions. Default: this one." }
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
                HistoryOp::Search => self.history.search(&input),
                HistoryOp::Read => self.history.read(&input, ctx),
                HistoryOp::List => self.history.list(),
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
}
