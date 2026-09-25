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
    /// a coworker had messaged them, and their reply is this call's result.
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
        match crate::coworker::deliver(&rail, ctx, &manager_id, &asked, true).await {
            Ok(delivery) => Some(match delivery.reply {
                Some(reply) => ToolResult::ok(format!(
                    "Nobody is at the keyboard, so this went to {}, who you answer to. Their \
                     answer:\n\n{reply}",
                    delivery.to_name
                )),
                None => ToolResult::ok(format!(
                    "Nobody is at the keyboard, so this went to {name}, who you answer to. They \
                     are deciding in their own session and you will be woken when they answer — \
                     report this as \"asked {name} — waiting\", never as done.",
                    name = delivery.to_name
                )),
            }),
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
        let widgets =
            json!([{ "type": "options", "multiSelect": multi_select, "options": options }]);
        match ctx.ask_user(question, widgets).await {
            Some(response) if response == crate::origin::SKIP_SENTINEL => ToolResult::ok(
                "The owner skipped this question. Make a reasonable assumption and carry on, but \
                 tell them what you assumed.",
            ),
            Some(response) => ToolResult::ok(json!({ "response": response }).to_string()),
            None => ToolResult::error(
                "No app is connected to show the question. Make a reasonable decision and \
                 proceed, or set out the options in your reply.",
            ),
        }
    }
}

impl DynTool for AskOwnerTool {
    fn name(&self) -> &str {
        "ask_owner"
    }

    fn description(&self) -> String {
        "Asks the owner one question and waits for the answer.\n\
         - Give `options` for a choice (two options like Yes/No for a confirmation); leave them out for a free answer.\n\
         - Ask only when the next step truly needs their decision."
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
}
