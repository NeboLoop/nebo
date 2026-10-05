//! `ask_owner`: one question to the owner, and the wait for the answer. In a
//! run nobody is watching, the question goes up the reporting line instead.

use std::sync::Arc;

use db::Store;
use serde_json::{Value, json};

use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

pub struct AskOwnerTool {
    store: Arc<Store>,
    /// The coworker rail the `message` tool holds: an unanswerable question
    /// in an unattended run travels up the reporting line on it.
    coworker_rail: crate::coworker::CoworkerRailCell,
}

impl AskOwnerTool {
    pub fn new(store: Arc<Store>, coworker_rail: crate::coworker::CoworkerRailCell) -> Self {
        Self {
            store,
            coworker_rail,
        }
    }

    /// Take a question this seat cannot answer to the seat it answers to.
    ///
    /// The reporting line (`agents.reports_to`) read through the store's ONE
    /// walk, delivered on the ONE coworker rail — the manager receives it in
    /// their own session, under their own persona and memory, exactly as if
    /// a coworker had messaged them. The call returns at once; their reply
    /// comes back to this session as a notification.
    ///
    /// `None` when there is no reporting line, no rail wired, or the delivery
    /// failed: the caller then decides for itself, as every seat did before
    /// the line existed.
    async fn ask_up_the_line(&self, ctx: &ToolContext, text: &str) -> Option<ToolResult> {
        let me = types::keyparser::extract_agent_id(&ctx.session_key);
        if me.is_empty() {
            return None;
        }
        let (manager_id, _) = self.store.manager_chain(&me).ok()?.into_iter().next()?;
        let rail = self.coworker_rail.read().ok()?.clone()?;
        let my_name = self
            .store
            .get_agent(&me)
            .ok()
            .flatten()
            .map(|a| a.name)
            .unwrap_or_else(|| me.clone());
        let asked = format!(
            "[{my_name} cannot finish this without a decision, and there is nobody at the \
             keyboard. You are the employee they answer to.]\n\n{text}"
        );
        match crate::coworker::deliver(&rail, ctx, &manager_id, &asked, None).await {
            Ok(delivery) => Some(ToolResult::ok(format!(
                "Nobody is at the keyboard, so this went to {name}, who you answer to. They decide \
                 in their own session, and their answer comes to you as a notification. Until \
                 then carry on with anything that doesn't depend on it, and don't treat it as \
                 decided.",
                name = delivery.to_name
            ))),
            Err(e) => {
                tracing::warn!(agent = %me, manager = %manager_id, error = %e,
                    "escalation up the reporting line failed; the seat decides for itself");
                None
            }
        }
    }

    async fn ask(&self, input: &Value, ctx: &ToolContext) -> ToolResult {
        let question = input["question"].as_str().unwrap_or("");
        // Asking needs someone at the keyboard: an automated, workflow,
        // channel or helper run would wait on a card nobody sees.
        if crate::origin::ExecutionMode::from(ctx.origin)
            != crate::origin::ExecutionMode::Interactive
            || ctx.ask_channels.is_none()
        {
            // A seat that answers to another seat is not on its own: the
            // question goes up the reporting line. A seat that answers to the
            // owner still decides for itself: this is not a new way to
            // interrupt the owner.
            if let Some(answered) = self.ask_up_the_line(ctx, question).await {
                return answered;
            }
            return ToolResult::error(
                "Nobody is at the keyboard in this run, and you answer to the owner directly \
                 rather than to another employee — make a reasonable decision and proceed, and \
                 tell the owner what you assumed.",
            );
        }
        let options = input.get("options").cloned().unwrap_or_else(|| json!([]));
        let multi_select = input["multi_select"].as_bool().unwrap_or(false);
        let labels: Vec<String> = options
            .as_array()
            .map(|a| a.iter().filter_map(|o| o.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        let widgets =
            json!([{ "type": "options", "multiSelect": multi_select, "options": options }]);
        match ctx.ask_user(question, widgets).await {
            Some(response) if response == crate::origin::SKIP_SENTINEL => ToolResult::ok(
                "The owner skipped this question. Make a reasonable assumption and carry on, but \
                 tell them what you assumed.",
            ),
            Some(response) => ToolResult::ok(answer(&labels, multi_select, &response).to_string()),
            None => ToolResult::error(
                "No app is connected to show the question. Make a reasonable decision and \
                 proceed, or set out the options in your reply.",
            ),
        }
    }
}

/// The owner's answer as the employee reads it. On a card with options,
/// only an answer that is one of them (on a multi-select card, several
/// joined by ", ") is a pick; anything else is his own words and picks
/// nothing. Live 2026-10-02: the owner typed "k" while a choice waited, and
/// the employee told him "you picked option A".
fn answer(options: &[String], multi_select: bool, response: &str) -> Value {
    if options.is_empty() {
        return json!({ "response": response });
    }
    let is_option = |p: &str| options.iter().any(|o| o == p.trim());
    if is_option(response) {
        return if multi_select { json!({ "picked": [response.trim()] }) } else { json!({ "picked": response.trim() }) };
    }
    if multi_select {
        let parts: Vec<&str> = response.split(", ").map(str::trim).collect();
        if parts.iter().all(|p| is_option(p)) {
            return json!({ "picked": parts });
        }
    }
    json!({
        "own_words": response,
        "note": "He wrote this himself: it is none of the options, so nothing was picked. Take it as what he wrote; if it doesn't settle the question, ask again.",
    })
}

impl DynTool for AskOwnerTool {
    fn name(&self) -> &str {
        "ask_owner"
    }

    fn description(&self) -> String {
        "Asks the owner one question; the work waits for the answer.\n\
         - Only for real ambiguity you can't resolve: readings that lead to different work, or a choice only the owner can make.\n\
         - A clear instruction your permission mode allows is carried out, never asked back to confirm it or how you'll do it.\n\
         - To have the owner pick, give `options`: buttons in his chat on desktop and in the mobile app. Never list them in text or draw a panel. Leave them out for a free answer."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "question": { "type": "string", "description": "The question, complete enough to answer without scrolling back." },
                "options": { "type": "array", "items": { "type": "string" }, "description": "Short labels to choose from, the recommended one first. Leave out for a free answer." },
                "multi_select": { "type": "boolean", "description": "Allow more than one option." }
            },
            "required": ["question"]
        })
    }

    fn search_hint(&self) -> &str {
        "ask the owner a question"
    }

    fn should_defer(&self) -> bool {
        false
    }

    /// Asking changes nothing, but it holds the owner's attention: never
    /// alongside other calls.
    fn read_only(&self, _input: &Value) -> bool {
        true
    }

    fn concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    fn validate_input(&self, input: &Value) -> Result<(), String> {
        if input["question"]
            .as_str()
            .is_none_or(|q| q.trim().is_empty())
        {
            return Err("question can't be empty.".to_string());
        }
        Ok(())
    }

    fn activity(&self, _input: &Value) -> String {
        "asking you".to_string()
    }

    fn outcome(&self, _input: &Value) -> String {
        "Asked you".to_string()
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move { self.ask(&input, ctx).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_unattended_run_with_no_manager_decides_for_itself() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(&dir.path().join("a.db").to_string_lossy()).unwrap());
        let tool = AskOwnerTool::new(store, crate::coworker::new_rail_cell());
        assert!(
            !tool.should_defer()
                && tool.read_only(&json!({}))
                && !tool.concurrency_safe(&json!({}))
        );
        let ctx = ToolContext {
            origin: crate::origin::Origin::Workflow,
            ..Default::default()
        };
        let r = tool
            .execute_dyn(&ctx, json!({"question": "Which vendor?"}))
            .await;
        assert!(
            r.is_error && r.content.contains("make a reasonable decision"),
            "{}",
            r.content
        );
    }

    /// Ask with the owner at the keyboard: the card that goes out, and the
    /// result once `reply` answers it, as the WS `ask_response` does.
    async fn asked(input: Value, reply: &str) -> (ai::StreamEvent, ToolResult) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(&dir.path().join("a.db").to_string_lossy()).unwrap());
        let tool = AskOwnerTool::new(store, crate::coworker::new_rail_cell());
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let channels: crate::origin::AskChannels = Default::default();
        let ctx = ToolContext {
            origin: crate::origin::Origin::User,
            stream_tx: Some(tx),
            ask_channels: Some(channels.clone()),
            ..Default::default()
        };
        let run = tokio::spawn(async move { tool.execute_dyn(&ctx, input).await });
        let card = rx.recv().await.expect("the card goes out");
        let id = card.error.clone().expect("the card has its request id");
        channels.lock().await.remove(&id).expect("the call waits on it").send(reply.to_string()).unwrap();
        (card, run.await.unwrap())
    }

    /// Live 2026-10-02: asked "can you use the cards?", Chief drew an A2UI
    /// panel nobody saw. A pick for the owner is this card: its options go
    /// out as buttons, and the one he taps comes back as his pick.
    #[tokio::test]
    async fn a_pick_goes_out_as_buttons_and_the_tapped_option_is_the_pick() {
        let q = json!({"question": "What should I focus on first?", "options": ["Invoicing", "Marketing", "A project"]});
        let (card, r) = asked(q, "Marketing").await;
        assert_eq!(card.text, "What should I focus on first?");
        let widgets = card.widgets.expect("the card has its options");
        assert_eq!(widgets[0]["type"], "options");
        assert_eq!(widgets[0]["options"], json!(["Invoicing", "Marketing", "A project"]));
        assert!(!r.is_error, "{}", r.content);
        let v: Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["picked"], "Marketing", "{v}");

        // Several, on a multi-select card.
        let q = json!({"question": "Which days?", "options": ["Mon", "Tue", "Wed"], "multi_select": true});
        let (_, r) = asked(q, "Mon, Wed").await;
        let v: Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["picked"], json!(["Mon", "Wed"]), "{v}");
    }

    /// Live 2026-10-02: the owner typed "k" while a choice waited and was
    /// told "you picked option A". Words that are none of the options pick
    /// nothing, and the employee is told so.
    #[tokio::test]
    async fn his_own_words_are_never_a_pick() {
        for words in ["k", "Marketing and invoicing"] {
            let q = json!({"question": "What should I focus on first?", "options": ["Invoicing", "Marketing", "A project"]});
            let (_, r) = asked(q, words).await;
            let v: Value = serde_json::from_str(&r.content).unwrap();
            assert!(v.get("picked").is_none(), "{words}: {v}");
            assert_eq!(v["own_words"], words);
            assert!(v["note"].as_str().unwrap().contains("nothing was picked"), "{v}");
        }
        // A free question has no options to pick from: the answer is the answer.
        let (_, r) = asked(json!({"question": "What's the client's name?"}), "Acme").await;
        assert_eq!(serde_json::from_str::<Value>(&r.content).unwrap(), json!({"response": "Acme"}));
    }

    /// The model is told where the card shows and that it is the way to have
    /// the owner pick.
    #[test]
    fn the_description_makes_it_the_way_to_have_the_owner_pick() {
        let dir = tempfile::tempdir().unwrap();
        let tool = AskOwnerTool::new(
            Arc::new(Store::new(&dir.path().join("a.db").to_string_lossy()).unwrap()),
            crate::coworker::new_rail_cell(),
        );
        let d = tool.description();
        assert!(d.contains("To have the owner pick, give `options`: buttons in his chat on desktop and in the mobile app"), "{d}");
        assert!(d.contains("Never list them in text or draw a panel"), "{d}");
    }
}
