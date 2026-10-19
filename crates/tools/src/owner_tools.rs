//! Reaching the owner outside a conversation, always through their Inbox:
//! `message_owner` leaves a message, `push_notification` posts (an update, or a
//! question they answer on the card; an urgent one raises the alert),
//! `check_dnd` reads whether Do Not Disturb is on, and `inbox` is the
//! assistant's way into the Inbox: it reads it, shows a card in chat,
//! answers one when the owner says so, and writes the morning brief.

use std::sync::Arc;

use db::Store;
use serde_json::{Value, json};

use crate::message_tool::NotifyFn;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// The server's side of the Inbox: the ONE answer path for an item
/// (`POST /notifications/{id}/answer`), so the assistant answers a card
/// exactly as the owner's tap would. Filled LATE by the server.
#[async_trait::async_trait]
pub trait InboxAnswers: Send + Sync {
    /// Answer item `id` with `value`; `via` is where the owner said it.
    async fn answer(&self, id: &str, value: &str, via: &str) -> Result<(), String>;
}

/// The shared cell [`InboxAnswers`] is read from, filled late.
pub type InboxAnswersCell = Arc<std::sync::RwLock<Option<Arc<dyn InboxAnswers>>>>;

pub struct Owner {
    store: Arc<Store>,
    /// The registry's broadcast cell, late-wired by the server: an alert
    /// reaches the bell and the desktop HUD through it.
    notify_fn: Arc<std::sync::RwLock<Option<NotifyFn>>>,
    inbox_answers: InboxAnswersCell,
}

impl Owner {
    pub fn new(store: Arc<Store>, notify_fn: Arc<std::sync::RwLock<Option<NotifyFn>>>, inbox_answers: InboxAnswersCell) -> Self {
        Self { store, notify_fn, inbox_answers }
    }

    pub fn tools(self) -> Vec<Box<dyn DynTool>> {
        let owner = Arc::new(self);
        [
            OwnerOp::Message,
            OwnerOp::Notify,
            OwnerOp::CheckDnd,
            OwnerOp::InboxRead,
            OwnerOp::InboxShow,
            OwnerOp::InboxAnswer,
            OwnerOp::InboxBrief,
        ]
            .into_iter()
            .map(|op| {
                Box::new(OwnerTool {
                    op,
                    owner: owner.clone(),
                }) as Box<dyn DynTool>
            })
            .collect()
    }

    /// A message for the owner, left in their Inbox, never written into a
    /// chat: the owner's chat is only the conversation he is having. One
    /// item per thread the call comes from ([`briefing_id`]): a newer message
    /// restates it and brings it back unread. It opens the work (the run, or
    /// the thread).
    fn message(&self, input: &Value, ctx: &ToolContext) -> ToolResult {
        let text = input["message"].as_str().unwrap_or("");
        let agent = Some(types::keyparser::extract_agent_id(&ctx.session_key))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| crate::team_tool::PRIMARY_AGENT_ID.to_string());
        let (link, link_label) = thread_link(&self.store, &agent, &ctx.session_key);
        let n = crate::owner_notify::OwnerNotification {
            id: &briefing_id(&ctx.session_key),
            kind: "message",
            title: "Sent you a message",
            body: Some(text),
            action_url: Some(&link),
            agent_id: Some(&agent),
            link_label: Some(link_label),
            latest: true,
            ..Default::default()
        };
        let notify = self.notify_fn.read().ok().and_then(|g| g.clone());
        let stored = match notify {
            Some(f) => crate::owner_notify::emit(&self.store, Some(&|ev, payload| f(ev, payload)), &n),
            None => crate::owner_notify::emit(&self.store, None, &n),
        };
        if !stored {
            return ToolResult::error("The message could not be saved to the owner's Inbox. Do not retry — this is a database error.");
        }
        notify_crate::send("Nebo", text);
        ToolResult::ok(format!("Left the owner a message in their Inbox ({} chars)", text.chars().count()))
    }

    /// An Inbox item from the employee: an update (a title, one sentence, a
    /// link to the work) or, with `options`, a question the owner answers on
    /// its card, whose answer comes back to this conversation. It is shown
    /// in the bell at once; an urgent one is the alert: a `notification`
    /// broadcast the desktop turns into the auto-dismissing HUD. Never a
    /// modal. An update from a run no one is watching (a schedule's or a
    /// heartbeat's fire, a workflow run) is that run's briefing: one item per
    /// thread, restated and brought back unread by each newer briefing
    /// ([`briefing_id`]).
    fn notify(&self, input: &Value, ctx: &ToolContext) -> ToolResult {
        let text = input["message"].as_str().unwrap_or("");
        let title = input["title"]
            .as_str()
            .filter(|t| !t.is_empty())
            .unwrap_or("Nebo");
        let urgent = input["urgent"].as_bool().unwrap_or(false);
        // Attributed to the employee that sent it.
        let agent_id =
            Some(types::keyparser::extract_agent_id(&ctx.session_key)).filter(|s| !s.is_empty());
        let id = uuid::Uuid::new_v4().to_string();
        // It opens the work it names, else the run or conversation it was
        // sent from.
        let link = input["link"]
            .as_str()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .or_else(|| agent_id.as_deref().map(|a| thread_link(&self.store, a, &ctx.session_key).0));
        let options: Vec<db::models::ItemOption> = input["options"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).map(str::trim).filter(|o| !o.is_empty()))
            .into_iter()
            .flatten()
            .map(|o| db::models::ItemOption { label: o.to_string(), value: o.to_string() })
            .collect();
        let asks = !options.is_empty();
        let briefing = (unwatched_run(ctx) && !asks).then(|| briefing_id(&ctx.session_key));
        let id = briefing.clone().unwrap_or(id);
        let n = crate::owner_notify::OwnerNotification {
            id: &id,
            kind: if urgent { "warning" } else { "info" },
            title,
            body: Some(text),
            action_url: link.as_deref(),
            agent_id: agent_id.as_deref(),
            loud: urgent,
            needs_you: asks,
            options: &options,
            link_label: input["link_label"].as_str().map(str::trim).filter(|l| !l.is_empty()),
            reply_to: asks.then_some(ctx.session_key.as_str()),
            latest: briefing.is_some(),
        };
        let notify = self.notify_fn.read().ok().and_then(|g| g.clone());
        match notify {
            Some(f) => crate::owner_notify::emit(&self.store, Some(&|ev, payload| f(ev, payload)), &n),
            None => crate::owner_notify::emit(&self.store, None, &n),
        };
        if urgent {
            return ToolResult::ok(format!("Alerted the owner: {title}"));
        }
        notify_crate::send(title, text);
        if asks {
            return ToolResult::ok(format!(
                "Asked the owner in their Inbox: '{title}'. Their answer comes back to this conversation; \
                 until then go on with anything that doesn't depend on it."
            ));
        }
        ToolResult::ok(format!(
            "Posted to the owner's Inbox ({} chars, title '{title}')",
            text.chars().count()
        ))
    }
}

impl Owner {
    /// The assistant's way into the Inbox: read it, show an item in chat,
    /// answer one on the owner's word, write the day's brief.
    async fn inbox(&self, op: OwnerOp, input: &Value, ctx: &ToolContext) -> ToolResult {
        let me = types::keyparser::extract_agent_id(&ctx.session_key);
        if !(me.is_empty() || me == crate::team_tool::PRIMARY_AGENT_ID) {
            return ToolResult::error(
                "The Inbox is read by the owner's assistant. Post to it with push_notification instead.",
            );
        }
        let user = match self.store.ensure_local_user_id() {
            Ok(u) => u,
            Err(e) => return ToolResult::error(format!("The Inbox can't be read: {e}. Do not retry.")),
        };
        match op {
            OwnerOp::InboxRead => self.inbox_list(input, &user),
            OwnerOp::InboxShow => self.inbox_show(input, &user),
            OwnerOp::InboxAnswer => self.inbox_answer(input, ctx, &user).await,
            _ => self.inbox_brief(input),
        }
    }

    /// The employee's name an item is from.
    fn employee(&self, agent_id: Option<&str>) -> String {
        agent_id
            .filter(|a| !a.is_empty())
            .and_then(|a| self.store.get_agent(a).ok().flatten())
            .map(|a| a.name)
            .unwrap_or_else(|| "Nebo".to_string())
    }

    /// One item as the assistant reads it.
    fn item_json(&self, n: &db::models::Notification) -> Value {
        json!({
            "id": n.id,
            "from": self.employee(n.agent_id.as_deref()),
            "kind": if n.needs_you { "needs_you" } else { "update" },
            "status": n.status,
            "title": n.title,
            "summary": n.body,
            "answers": n.options.iter().map(|o| json!({ "label": o.label, "value": o.value })).collect::<Vec<_>>(),
            "answered": n.answer,
            "read": n.read_at.is_some(),
            "at": n.created_at,
        })
    }

    fn inbox_list(&self, input: &Value, user: &str) -> ToolResult {
        let view = input["view"].as_str().and_then(db::models::InboxView::parse).unwrap_or_default();
        let since = input["since_hours"].as_i64().map(|h| chrono::Utc::now().timestamp() - h.max(1) * 3600);
        match self.store.list_user_notifications(user, view, None, 100, 0) {
            Ok(items) => {
                let items: Vec<Value> = items
                    .iter()
                    // The day's brief is the assistant's own writing.
                    .filter(|n| n.notification_type != BRIEF_TYPE)
                    .filter(|n| since.is_none_or(|t| n.created_at >= t || n.status == "open"))
                    .map(|n| self.item_json(n))
                    .collect();
                ToolResult::ok(json!({ "items": items, "total": items.len() }).to_string())
            }
            Err(e) => ToolResult::error(format!("The Inbox can't be read: {e}. Do not retry.")),
        }
    }

    fn inbox_show(&self, input: &Value, user: &str) -> ToolResult {
        let id = input["id"].as_str().unwrap_or("");
        match self.store.get_notification(id, user) {
            Ok(Some(n)) => ToolResult::ok(format!(
                "Shown to the owner in this chat as its Inbox card: '{}'. They can answer it on the card.",
                n.title
            ))
            // The chat renders the Inbox's own card for it, answered on the
            // same path as in the Inbox.
            .with_payload(json!({ "kind": "inbox_item", "item": n })),
            Ok(None) => ToolResult::error(format!("No Inbox item '{id}'. Read the Inbox (read_inbox) for the ids.")),
            Err(e) => ToolResult::error(format!("The Inbox can't be read: {e}. Do not retry.")),
        }
    }

    /// Answer a card for the owner. Only on his own word in his own chat:
    /// the assistant never decides one by itself.
    async fn inbox_answer(&self, input: &Value, ctx: &ToolContext, user: &str) -> ToolResult {
        if !ctx.owner_request || !matches!(ctx.door, types::permissions::Door::Chat | types::permissions::Door::Voice) {
            return ToolResult::error(
                "Only the owner answers Inbox items. Answer one only when the owner tells you to, in their \
                 own words in this chat; otherwise show it to them with inbox_card.",
            );
        }
        let (id, value) = (input["id"].as_str().unwrap_or(""), input["value"].as_str().unwrap_or("").trim());
        let item = match self.store.get_notification(id, user) {
            Ok(Some(n)) => n,
            Ok(None) => return ToolResult::error(format!("No Inbox item '{id}'. Read the Inbox (read_inbox) for the ids.")),
            Err(e) => return ToolResult::error(format!("The Inbox can't be read: {e}. Do not retry.")),
        };
        if item.answered_at.is_some() {
            return ToolResult::ok(format!(
                "'{}' was already answered ({}). Nothing changed.",
                item.title,
                item.answer.as_deref().unwrap_or("answered")
            ));
        }
        // The answer is one the card takes: an option's value or its words.
        let value = item
            .options
            .iter()
            .find(|o| o.value.eq_ignore_ascii_case(value) || o.label.eq_ignore_ascii_case(value))
            .map_or(value.to_string(), |o| o.value.clone());
        let answers = self.inbox_answers.read().ok().and_then(|g| g.clone());
        let Some(answers) = answers else {
            return ToolResult::error("Inbox answers aren't available here. Show the card instead.");
        };
        let via = if matches!(ctx.door, types::permissions::Door::Voice) { "voice" } else { "chat" };
        match answers.answer(id, &value, via).await {
            Ok(()) => ToolResult::ok(format!("Answered '{}' with {value} for the owner.", item.title)),
            Err(e) => ToolResult::error(format!("'{}' was not answered: {e}", item.title)),
        }
    }

    /// The day's brief, at the top of the Inbox: one item a day, rewritten
    /// in place when the assistant writes it again the same day.
    fn inbox_brief(&self, input: &Value) -> ToolResult {
        let text = input["text"].as_str().map(str::trim).unwrap_or("");
        if text.is_empty() {
            return ToolResult::error("text can't be empty: write the brief in a few plain sentences.");
        }
        let day = crate::owner_clock::OwnerZone::of(&self.store).now().format("%Y-%m-%d").to_string();
        let id = brief_id(&day);
        let n = crate::owner_notify::OwnerNotification {
            id: &id,
            kind: BRIEF_TYPE,
            title: "Your brief",
            body: Some(text),
            action_url: Some(&crate::owner_notify::link::assistant()),
            agent_id: Some(crate::team_tool::PRIMARY_AGENT_ID),
            ..Default::default()
        };
        let notify = self.notify_fn.read().ok().and_then(|g| g.clone());
        let broadcast = |ev: &str, payload: Value| {
            if let Some(f) = &notify {
                f(ev, payload);
            }
        };
        if !crate::owner_notify::restate(&self.store, Some(&broadcast), &n) {
            crate::owner_notify::emit(&self.store, Some(&broadcast), &n);
        }
        ToolResult::ok(format!("Today's brief is at the top of the owner's Inbox ({day})."))
    }
}

/// Whether the call comes from a run no one is watching that briefs the
/// owner: a schedule's or a heartbeat's fire, or a workflow run.
fn unwatched_run(ctx: &ToolContext) -> bool {
    use types::permissions::Door;
    matches!(ctx.door, Door::Schedule | Door::Heartbeat | Door::Workflow)
}

/// The one Inbox item a thread briefs the owner in: a workflow run's (all
/// of its steps are one thread), else the session's (a schedule's thread,
/// a heartbeat's, a conversation).
fn briefing_id(session_key: &str) -> String {
    match types::keyparser::workflow_step(session_key) {
        Some((run, _)) => format!("briefing:workflow:{run}"),
        None => format!("briefing:{session_key}"),
    }
}

/// Where an item from `session_key` opens, and its link's words: a workflow
/// step's run, else the conversation the session holds.
fn thread_link(store: &Store, agent_id: &str, session_key: &str) -> (String, &'static str) {
    match types::keyparser::workflow_step(session_key) {
        Some((run, _)) => (crate::owner_notify::link::run(agent_id, run), "Run"),
        None => (crate::owner_notify::link::session_chat(store, agent_id, session_key), "Chat"),
    }
}

/// The Inbox type of the assistant's daily brief.
pub const BRIEF_TYPE: &str = "brief";

/// The day's brief item (`day` on the owner's clock, `YYYY-MM-DD`).
pub fn brief_id(day: &str) -> String {
    format!("brief:{day}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OwnerOp {
    Message,
    Notify,
    CheckDnd,
    InboxRead,
    InboxShow,
    InboxAnswer,
    InboxBrief,
}


struct OwnerTool {
    op: OwnerOp,
    owner: Arc<Owner>,
}

impl DynTool for OwnerTool {
    fn name(&self) -> &str {
        match self.op {
            OwnerOp::Message => "message_owner",
            OwnerOp::Notify => "push_notification",
            OwnerOp::CheckDnd => "check_dnd",
            OwnerOp::InboxRead => "read_inbox",
            OwnerOp::InboxShow => "inbox_card",
            OwnerOp::InboxAnswer => "answer_inbox_item",
            OwnerOp::InboxBrief => "write_brief",
        }
    }

    fn description(&self) -> String {
        match self.op {
            OwnerOp::Message => "Leaves the owner a message in their Inbox, with a notification; it never goes into a chat. For work nobody is watching (a schedule, a workflow), and only when there is something for them; in a conversation, just reply.".to_string(),
            OwnerOp::Notify => "Posts to the owner's Inbox, the one place employees leave updates for them.\n\
                 - An update: a short `title`, ONE plain sentence in `message`, and a `link` to the work (its chat, file or run) with `link_label`.\n\
                 - A question: add `options`; the owner answers on the card and the answer comes back to this conversation.\n\
                 - `urgent: true` raises the alert: for something that needs them now. check_dnd first if it can wait.\n\
                 Don't post routine steps (checking memory, loading tools): only what the owner would want to know."
                .to_string(),
            OwnerOp::CheckDnd => "Reads whether Do Not Disturb (Focus) is on for the owner, and which modes.".to_string(),
            OwnerOp::InboxRead => "Reads the owner's Inbox, where the employees leave updates and questions (the owner's assistant only). \
                 `view`: needs_you, updates or history; `since_hours` narrows it. Summarise it in plain words when asked what they missed."
                .to_string(),
            OwnerOp::InboxShow => "Puts one Inbox item's card in this chat (the owner's assistant only); the owner answers it on the card.".to_string(),
            OwnerOp::InboxAnswer => "Answers an Inbox item with one of its answers (the owner's assistant only), ONLY when the owner tells you to in their own words.".to_string(),
            OwnerOp::InboxBrief => "Writes today's brief at the top of the owner's Inbox (the owner's assistant only): a few plain sentences, what needs them first, then what went well.".to_string(),
        }
    }

    fn schema(&self) -> Value {
        match self.op {
            OwnerOp::Message => json!({
                "type": "object",
                "properties": { "message": { "type": "string", "description": "What to tell the owner." } },
                "required": ["message"]
            }),
            OwnerOp::Notify => json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string", "description": "One plain sentence: what happened, or what you need to know." },
                    "title": { "type": "string", "description": "Short title, a few words (\"Shopify orders synced\")." },
                    "link": { "type": "string", "description": "Where the work is: an app path (\"/<employee>/runs/<run id>\") or a web address. Default: this conversation." },
                    "link_label": { "type": "string", "description": "The link's words (\"Orders sheet\", \"Report\")." },
                    "options": { "type": "array", "items": { "type": "string" }, "description": "Makes it a question with these answers, shown as buttons." },
                    "urgent": { "type": "boolean", "description": "Raise the alert: it needs the owner now." }
                },
                "required": ["message"]
            }),
            OwnerOp::CheckDnd => json!({ "type": "object", "properties": {} }),
            OwnerOp::InboxRead => json!({
                "type": "object",
                "properties": {
                    "view": { "type": "string", "enum": ["needs_you", "updates", "history"], "description": "Which part. Default: everything not put away." },
                    "since_hours": { "type": "integer", "description": "Only items from the last N hours (open decisions always)." }
                }
            }),
            OwnerOp::InboxShow => json!({
                "type": "object",
                "properties": { "id": { "type": "string", "description": "The item's id from read_inbox." } },
                "required": ["id"]
            }),
            OwnerOp::InboxAnswer => json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "The item's id from read_inbox." },
                    "value": { "type": "string", "description": "One of the item's answers (its value or its words)." }
                },
                "required": ["id", "value"]
            }),
            OwnerOp::InboxBrief => json!({
                "type": "object",
                "properties": { "text": { "type": "string", "description": "The brief." } },
                "required": ["text"]
            }),
        }
    }

    fn search_hint(&self) -> &str {
        match self.op {
            OwnerOp::Message => "message the owner in their chat",
            OwnerOp::Notify => "post an update or question to the inbox",
            OwnerOp::CheckDnd => "is do not disturb on",
            OwnerOp::InboxRead => "read and summarise the owner's inbox",
            OwnerOp::InboxShow => "put an inbox card in chat",
            OwnerOp::InboxAnswer => "answer an inbox item for the owner",
            OwnerOp::InboxBrief => "write the owner's daily brief",
        }
    }

    fn read_only(&self, _input: &Value) -> bool {
        match self.op {
            OwnerOp::CheckDnd => true,
            OwnerOp::InboxRead | OwnerOp::InboxShow => true,
            OwnerOp::Message | OwnerOp::Notify | OwnerOp::InboxAnswer | OwnerOp::InboxBrief => false,
        }
    }

    /// Telling the owner is not an outside effect.
    fn effects(&self, _input: &Value) -> types::permissions::CallEffects {
        types::permissions::CallEffects::none()
    }

    fn validate_input(&self, input: &Value) -> Result<(), String> {
        let blank = |k: &str| input[k].as_str().is_none_or(|v| v.trim().is_empty());
        match self.op {
            OwnerOp::InboxShow | OwnerOp::InboxAnswer if blank("id") => {
                return Err("id can't be empty: take it from read_inbox.".to_string());
            }
            OwnerOp::InboxAnswer if blank("value") => return Err("value can't be empty: one of the item's answers.".to_string()),
            OwnerOp::InboxBrief if blank("text") => return Err("text can't be empty: write the brief in a few plain sentences.".to_string()),
            OwnerOp::InboxRead | OwnerOp::InboxShow | OwnerOp::InboxAnswer | OwnerOp::InboxBrief => return Ok(()),
            _ => {}
        }
        if self.op != OwnerOp::CheckDnd
            && input["message"]
                .as_str()
                .is_none_or(|m| m.trim().is_empty())
        {
            return Err("message can't be empty.".to_string());
        }
        Ok(())
    }

    fn describe(&self, _input: &Value) -> String {
        match self.op {
            OwnerOp::Message => "messaging you".to_string(),
            OwnerOp::Notify => "posting to your Inbox".to_string(),
            OwnerOp::CheckDnd => "checking Do Not Disturb".to_string(),
            OwnerOp::InboxRead => "reading your Inbox".to_string(),
            OwnerOp::InboxShow => "showing an Inbox item".to_string(),
            OwnerOp::InboxAnswer => "answering an Inbox item for you".to_string(),
            OwnerOp::InboxBrief => "writing your brief".to_string(),
        }
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            match self.op {
                OwnerOp::Message => self.owner.message(&input, ctx),
                OwnerOp::Notify => self.owner.notify(&input, ctx),
                OwnerOp::CheckDnd => handle_dnd_status().await,
                OwnerOp::InboxRead | OwnerOp::InboxShow | OwnerOp::InboxAnswer | OwnerOp::InboxBrief => {
                    self.owner.inbox(self.op, &input, ctx).await
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Execution raises a real OS notification, so only the interface is
    /// exercised here.
    #[test]
    fn the_owner_tools_need_a_message() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(&dir.path().join("o.db").to_string_lossy()).unwrap());
        let tools = Owner::new(store, Arc::new(std::sync::RwLock::new(None)), Arc::new(std::sync::RwLock::new(None))).tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert_eq!(names, ["message_owner", "push_notification", "check_dnd", "read_inbox", "inbox_card", "answer_inbox_item", "write_brief"]);
        assert!(tools[0].validate_input(&json!({"message": ""})).is_err());
        assert!(tools[1].validate_input(&json!({"title": "x"})).is_err());
        assert!(tools[2].validate_input(&json!({})).is_ok() && tools[2].read_only(&json!({})));
    }

    /// Nothing reaches the owner's chat unprompted: message_owner leaves its
    /// message in his Inbox from every door. What a run no one watches has
    /// for him is that thread's ONE briefing: a schedule's fire, and a
    /// workflow run's steps (one item per run, opening the run), each newer
    /// message or update restating it and bringing it back unread. A
    /// question is its own item, and an update from his chat is not a
    /// briefing.
    #[tokio::test]
    async fn runs_brief_the_owner_in_his_inbox_never_his_chat() {
        use types::permissions::Door;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(&dir.path().join("o.db").to_string_lossy()).unwrap());
        let tools = Owner::new(store.clone(), Arc::new(std::sync::RwLock::new(None)), Arc::new(std::sync::RwLock::new(None))).tools();
        let tool = |name: &str| tools.iter().find(|t| t.name() == name).unwrap();
        let user = store.ensure_local_user_id().unwrap();
        let items = || store.list_user_notifications(&user, db::models::InboxView::All, None, 50, 0).unwrap();
        let item = |id: &str| items().into_iter().find(|n| n.id == id).unwrap_or_else(|| panic!("{id}: {:#?}", items()));

        let fire = ToolContext { session_key: "agent:assistant:cron:hourly-calendar-check".into(), door: Door::Schedule, ..Default::default() };
        let r = tool("message_owner").execute_dyn(&fire, json!({ "message": "Tomorrow: packed Monday — three meetings from 9." })).await;
        assert!(!r.is_error && r.content.contains("Inbox"), "{}", r.content);
        let r = tool("push_notification").execute_dyn(&fire, json!({ "title": "Tomorrow", "message": "Tomorrow: packed Monday — four meetings from 9." })).await;
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(items().len(), 1, "one briefing per schedule: {:#?}", items());
        let briefing = item("briefing:agent:assistant:cron:hourly-calendar-check");
        assert_eq!(briefing.body.as_deref(), Some("Tomorrow: packed Monday — four meetings from 9."));
        assert!(briefing.read_at.is_none());

        for (step, text) in [("agent:ops:workflow:run-1:pull-orders:0", "12 new orders."), ("agent:ops:workflow:run-1:report:0", "Orders sheet updated: 12 new.")] {
            let step = ToolContext { session_key: step.into(), door: Door::Workflow, ..Default::default() };
            assert!(!tool("message_owner").execute_dyn(&step, json!({ "message": text })).await.is_error);
        }
        let run = item("briefing:workflow:run-1");
        assert_eq!(run.body.as_deref(), Some("Orders sheet updated: 12 new."));
        assert_eq!((run.action_url.as_deref(), run.agent_id.as_deref()), (Some("/ops/runs/run-1"), Some("ops")), "it opens the run");
        assert_eq!(items().len(), 2, "one item per workflow run");

        let asked = tool("push_notification").execute_dyn(&fire, json!({ "message": "Move the 9:00?", "options": ["Yes", "No"] })).await;
        assert!(!asked.is_error, "{}", asked.content);
        let chat = ToolContext { session_key: "agent:assistant:web".into(), ..Default::default() };
        assert!(!tool("push_notification").execute_dyn(&chat, json!({ "message": "Sheet synced." })).await.is_error);
        assert_eq!(items().len(), 4, "a question and an update from chat are their own items");

        let chats = store.list_chats(100, 0).unwrap();
        assert!(chats.is_empty(), "no chat was written: {chats:#?}");
    }

    /// Answers the `inbox` tool hands the server, recorded.
    struct Recorded(std::sync::Mutex<Vec<(String, String, String)>>);

    #[async_trait::async_trait]
    impl InboxAnswers for Recorded {
        async fn answer(&self, id: &str, value: &str, via: &str) -> Result<(), String> {
            self.0.lock().unwrap().push((id.into(), value.into(), via.into()));
            Ok(())
        }
    }

    /// The assistant reads the Inbox, writes one brief a day (written again,
    /// it is rewritten in place), and answers a card only on the owner's own
    /// word in his chat, with the card's own answer, through the server's one
    /// answer path. Another employee can't read it.
    #[tokio::test]
    async fn the_assistant_reads_the_inbox_and_answers_only_on_the_owners_word() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(&dir.path().join("o.db").to_string_lossy()).unwrap());
        let recorded = Arc::new(Recorded(Default::default()));
        let cell: InboxAnswersCell = Arc::new(std::sync::RwLock::new(Some(recorded.clone() as Arc<dyn InboxAnswers>)));
        let tools = Owner::new(store.clone(), Arc::new(std::sync::RwLock::new(None)), cell).tools();
        let tool = |name: &str| tools.iter().find(|t| t.name() == name).unwrap();
        let options = [db::models::ItemOption { label: "Lumen · $14.20".into(), value: "lumen".into() }];
        crate::owner_notify::emit(&store, None, &crate::owner_notify::OwnerNotification {
            id: "q:1", kind: "info", title: "Which supplier?", needs_you: true, options: &options,
            reply_to: Some("agent:store:cron:x"), agent_id: Some("store"), ..Default::default()
        });
        let pa = |owner_request: bool| ToolContext {
            session_key: "agent:assistant:web".into(),
            owner_request,
            ..Default::default()
        };

        let listed = tool("read_inbox").execute_dyn(&pa(false), json!({ "view": "needs_you" })).await;
        assert!(listed.content.contains("Which supplier?") && listed.content.contains("lumen"), "{}", listed.content);
        let shown = tool("inbox_card").execute_dyn(&pa(false), json!({ "id": "q:1" })).await;
        assert_eq!(shown.payload.as_ref().unwrap()["kind"], "inbox_item");

        let unasked = tool("answer_inbox_item").execute_dyn(&pa(false), json!({ "id": "q:1", "value": "lumen" })).await;
        assert!(unasked.is_error && recorded.0.lock().unwrap().is_empty(), "never without the owner's word");
        let asked = tool("answer_inbox_item").execute_dyn(&pa(true), json!({ "id": "q:1", "value": "Lumen · $14.20" })).await;
        assert!(!asked.is_error, "{}", asked.content);
        assert_eq!(*recorded.0.lock().unwrap(), [("q:1".to_string(), "lumen".to_string(), "chat".to_string())]);

        for text in ["Two things need you.", "Two things need you; the rest went well."] {
            let r = tool("write_brief").execute_dyn(&pa(false), json!({ "text": text })).await;
            assert!(!r.is_error, "{}", r.content);
        }
        let user = store.ensure_local_user_id().unwrap();
        let briefs: Vec<_> = store
            .list_user_notifications(&user, db::models::InboxView::All, None, 50, 0)
            .unwrap()
            .into_iter()
            .filter(|n| n.notification_type == BRIEF_TYPE)
            .collect();
        assert_eq!(briefs.len(), 1, "one brief a day");
        assert_eq!(briefs[0].body.as_deref(), Some("Two things need you; the rest went well."));
        assert!(!tool("read_inbox").execute_dyn(&pa(false), json!({})).await.content.contains("Two things need you"), "the brief isn't listed back");

        let other = ToolContext { session_key: "agent:store:web".into(), owner_request: true, ..Default::default() };
        assert!(tool("read_inbox").execute_dyn(&other, json!({})).await.is_error);
    }
}

// ---------------------------------------------------------------------------
// DND status
// ---------------------------------------------------------------------------

async fn handle_dnd_status() -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        // Focus (macOS 12+) records every active mode as an assertion in
        // ~/Library/DoNotDisturb/DB/Assertions.json; an empty record list
        // means no Focus is on. That is the state itself, not a menu-bar
        // preference.
        let assertions =
            dirs::home_dir().map(|h| h.join("Library/DoNotDisturb/DB/Assertions.json"));
        let read = match assertions {
            Some(ref p) => tokio::fs::read_to_string(p)
                .await
                .map_err(|e| e.to_string()),
            None => Err("home directory unknown".to_string()),
        };
        match read.map(|s| focus_assertions(&s)) {
            Ok(Some(modes)) => ToolResult::ok(
                serde_json::json!({
                    "dnd_enabled": !modes.is_empty(),
                    "active_focus_modes": modes,
                    "source": "~/Library/DoNotDisturb/DB/Assertions.json",
                })
                .to_string(),
            ),
            Ok(None) => ToolResult::ok(
                serde_json::json!({
                    "dnd_enabled": null,
                    "note": "DND state unknown: ~/Library/DoNotDisturb/DB/Assertions.json was read but is not in the expected shape",
                })
                .to_string(),
            ),
            Err(e) => {
                // Legacy (pre-Focus) preference, still a real DND flag on
                // old systems; on new ones the key is absent.
                let legacy = command::new::<tokio::process::Command>("defaults", command::Console::Hidden)
                    .args(["read", "com.apple.ncprefs", "dnd_prefs"])
                    .output()
                    .await;
                if let Ok(o) = legacy
                    && o.status.success()
                {
                    let stdout = String::from_utf8_lossy(&o.stdout);
                    let enabled = stdout.contains("dndDisplayLock = 1") || stdout.contains("dndMirrored = 1");
                    return ToolResult::ok(
                        serde_json::json!({
                            "dnd_enabled": enabled,
                            "source": "defaults read com.apple.ncprefs dnd_prefs (legacy)",
                        })
                        .to_string(),
                    );
                }
                ToolResult::ok(
                    serde_json::json!({
                        "dnd_enabled": null,
                        "note": format!("DND state unknown: could not read ~/Library/DoNotDisturb/DB/Assertions.json ({}); ask the owner to grant Nebo Full Disk Access if this persists", e),
                    })
                    .to_string(),
                )
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        // Try D-Bus to check GNOME DND
        let output = command::new::<tokio::process::Command>("dbus-send", command::Console::Hidden)
            .args([
                "--session",
                "--print-reply",
                "--dest=org.freedesktop.Notifications",
                "/org/freedesktop/Notifications",
                "org.freedesktop.DBus.Properties.Get",
                "string:org.freedesktop.Notifications",
                "string:DoNotDisturb",
            ])
            .output()
            .await;

        match output {
            Ok(o) if o.status.success() => {
                let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();
                let enabled = stdout.contains("true");
                return ToolResult::ok(
                    serde_json::json!({
                        "dnd_enabled": enabled,
                        "source": "org.freedesktop.Notifications DoNotDisturb property via dbus-send",
                    })
                    .to_string(),
                );
            }
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
                return ToolResult::ok(
                    serde_json::json!({
                        "dnd_enabled": null,
                        "note": format!("DND state unknown: D-Bus query failed (dbus-send exited {}: {})", o.status.code().unwrap_or(-1), stderr),
                    })
                    .to_string(),
                );
            }
            Err(e) => {
                return ToolResult::ok(
                    serde_json::json!({
                        "dnd_enabled": null,
                        "note": format!("DND state unknown: D-Bus query failed (dbus-send could not run: {})", e),
                    })
                    .to_string(),
                );
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        // Focus Assist keeps its state in an undocumented binary blob; the
        // only honest reading is whether the key exists and how big it is.
        let script = r#"try { $val = Get-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\CloudStore\Store\DefaultAccount\Current\default$windows.data.notifications.quiethourssettings\windows.data.notifications.quiethourssettings' -ErrorAction Stop; Write-Output ("" + $val.Data.Length) } catch { Write-Output 'unavailable' }"#;
        let r = run_powershell(script).await;
        if r.is_error {
            return r;
        }
        let note = match r.content.trim().parse::<usize>() {
            Ok(n) => format!(
                "DND state unknown: Focus Assist stores its state as an undocumented {} byte binary blob under HKCU ...quiethourssettings, which Nebo does not decode",
                n
            ),
            Err(_) => "DND state unknown: the Focus Assist registry key (HKCU ...quiethourssettings) is not present".to_string(),
        };
        return ToolResult::ok(
            serde_json::json!({
                "dnd_enabled": null,
                "note": note,
            })
            .to_string(),
        );
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("Do Not Disturb status is not available on this platform. Do not retry.")
}

/// Active Focus mode identifiers from the contents of
/// `~/Library/DoNotDisturb/DB/Assertions.json`: every `storeAssertionRecords`
/// entry under `data` is one active mode. `None` when the document is not in
/// that shape (so the caller reports "unknown", never a guessed boolean).
#[cfg(target_os = "macos")]
fn focus_assertions(json: &str) -> Option<Vec<String>> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let data = v.get("data")?.as_array()?;
    let mut modes = Vec::new();
    for entry in data {
        let records = entry
            .get("storeAssertionRecords")
            .and_then(|r| r.as_array())
            .map(|r| r.as_slice())
            .unwrap_or(&[]);
        for record in records {
            let mode = record
                .get("assertionDetails")
                .and_then(|d| d.get("assertionDetailsModeIdentifier"))
                .and_then(|m| m.as_str())
                .unwrap_or("unknown");
            modes.push(mode.to_string());
        }
    }
    Some(modes)
}

#[cfg(all(test, target_os = "macos"))]
mod focus_tests {
    use super::focus_assertions;

    #[test]
    fn active_focus_lists_mode_identifiers() {
        let json = r#"{"data":[{"storeAssertionRecords":[{"assertionDetails":{"assertionDetailsModeIdentifier":"com.apple.donotdisturb.mode.default"}}]}]}"#;
        assert_eq!(
            focus_assertions(json),
            Some(vec!["com.apple.donotdisturb.mode.default".to_string()])
        );
    }

    #[test]
    fn no_focus_is_empty_not_unknown() {
        let json = r#"{"data":[{"storeAssertionRecords":[]}]}"#;
        assert_eq!(focus_assertions(json), Some(vec![]));
    }

    #[test]
    fn unexpected_shape_is_unknown() {
        assert_eq!(focus_assertions("not json"), None);
        assert_eq!(focus_assertions(r#"{"other":1}"#), None);
    }
}

// ---------------------------------------------------------------------------
// Helper: run PowerShell (Windows)
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
async fn run_powershell(script: &str) -> ToolResult {
    let output = command::powershell::<tokio::process::Command>(&script)
        .output()
        .await;

    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if stdout.is_empty() {
                ToolResult::ok("OK")
            } else {
                ToolResult::ok(stdout)
            }
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
            ToolResult::error(format!(
                "PowerShell error: {}. Do not retry — this is a system error.",
                stderr
            ))
        }
        Err(e) => ToolResult::error(format!(
            "Failed to run PowerShell: {}. Do not retry — this is a system error.",
            e
        )),
    }
}
