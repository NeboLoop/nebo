//! `suggest_goal`: the interface onto the harness's agreed goal. The
//! behaviour (the approval card, setting the goal, the done check) is the
//! harness's (`agent::harness::goal`); it binds itself here through
//! [`GoalSuggester`], since this crate can't depend on the agent crate.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use serde_json::{Value, json};

use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// The longest goal a suggestion may carry: the owner reads all of it on
/// the approval card.
pub const MAX_SUGGESTED_CONDITION_CHARS: usize = 500;

/// Handles a `suggest_goal` call for the conversation `ctx` belongs to.
/// `Ok` is the result the model reads; `Err` is a refusal it can correct.
pub trait GoalSuggester: Send + Sync {
    fn suggest<'a>(
        &'a self,
        ctx: &'a ToolContext,
        condition: &'a str,
        ask_owner: bool,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;
}

/// Late-binding handle: created empty with the registry, bound by the
/// harness once it exists.
pub type GoalHandle = Arc<OnceLock<Arc<dyn GoalSuggester>>>;

pub fn new_handle() -> GoalHandle {
    Arc::new(OnceLock::new())
}

pub struct SuggestGoalTool {
    goals: GoalHandle,
}

impl SuggestGoalTool {
    pub fn new(goals: GoalHandle) -> Self {
        Self { goals }
    }
}

impl DynTool for SuggestGoalTool {
    fn name(&self) -> &str {
        "suggest_goal"
    }

    /// What a goal does, that the proposal doesn't wait, when to ask and
    /// when to set it directly, when to propose, and how to state the
    /// condition for a check that reads only the conversation.
    fn description(&self) -> String {
        "Proposes an agreed goal: an end state that keeps you working until a separate check confirms it is met. It doesn't wait: the owner sees it on a card while you keep working.\n\
         - Propose one only when the owner asked for an outcome with a checkable end state (\"every test passes\", \"all 40 invoices are filed\") and the work spans several turns. Not for one-off tasks, and never to widen scope: the goal follows from what they asked.\n\
         - The owner approves it on the card. Set ask_owner false only when the owner's own words in this conversation stated this outcome; if you inferred it, or are unsure, ask. Either way you're told when the goal is set.\n\
         - The check reads only the conversation; it can't run commands or open files. State one end state and how it is shown, in at most 500 characters.\n\
         - One goal at a time: a new one replaces the current one. If the owner declines, you aren't told; don't ask about it or propose it again."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "condition": {
                    "type": "string",
                    "maxLength": MAX_SUGGESTED_CONDITION_CHARS,
                    "description": "The end state, stated so a separate check can confirm it from the conversation."
                },
                "ask_owner": {
                    "type": "boolean",
                    "default": true,
                    "description": "Ask the owner to approve. Set false only when the owner's own words in this conversation stated this outcome."
                }
            },
            "required": ["condition"]
        })
    }

    fn search_hint(&self) -> &str {
        "propose a goal for the owner to approve"
    }

    /// Deferred, since it is needed rarely: listed by name and loaded with
    /// find_tools.
    fn should_defer(&self) -> bool {
        true
    }

    /// The goal is the employee's own work; the owner approves it.
    fn effects(&self, _input: &Value) -> types::permissions::CallEffects {
        types::permissions::CallEffects::none()
    }

    /// One suggestion waits on the owner at a time.
    fn concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    fn validate_input(&self, input: &Value) -> Result<(), String> {
        if input["condition"]
            .as_str()
            .is_none_or(|c| c.trim().is_empty())
        {
            return Err(
                "condition can't be empty: say the end state the work should reach.".to_string(),
            );
        }
        Ok(())
    }

    fn activity(&self, _input: &Value) -> String {
        "suggesting a goal".to_string()
    }

    fn outcome(&self, _input: &Value) -> String {
        "Suggested a goal".to_string()
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let Some(goals) = self.goals.get().cloned() else {
                return ToolResult::error(
                    "Goals can't be set in this conversation. Keep working toward what the owner asked.",
                );
            };
            let condition = input["condition"].as_str().unwrap_or("").trim();
            let ask_owner = input["ask_owner"].as_bool().unwrap_or(true);
            match goals.suggest(ctx, condition, ask_owner).await {
                Ok(told) => ToolResult::ok(told),
                Err(refused) => ToolResult::error(refused),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Seen(Mutex<Vec<(String, String, bool)>>);

    impl GoalSuggester for Seen {
        fn suggest<'a>(
            &'a self,
            ctx: &'a ToolContext,
            condition: &'a str,
            ask_owner: bool,
        ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
            self.0
                .lock()
                .unwrap()
                .push((ctx.session_id.clone(), condition.to_string(), ask_owner));
            Box::pin(async { Ok("on a card".to_string()) })
        }
    }

    #[tokio::test]
    async fn a_suggestion_reaches_the_bound_goal_and_asks_by_default() {
        let handle = new_handle();
        let tool = SuggestGoalTool::new(handle.clone());
        let ctx = ToolContext {
            session_id: "s1".into(),
            ..Default::default()
        };
        let unbound = tool
            .execute_dyn(&ctx, json!({"condition": "all tests pass"}))
            .await;
        assert!(unbound.is_error, "{}", unbound.content);
        let seen = Arc::new(Seen::default());
        let _ = handle.set(seen.clone());
        assert_eq!(
            tool.execute_dyn(&ctx, json!({"condition": " all tests pass "}))
                .await
                .content,
            "on a card"
        );
        tool.execute_dyn(
            &ctx,
            json!({"condition": "the invoice is sent", "ask_owner": false}),
        )
        .await;
        assert_eq!(
            seen.0.lock().unwrap().as_slice(),
            &[
                ("s1".to_string(), "all tests pass".to_string(), true),
                ("s1".to_string(), "the invoice is sent".to_string(), false)
            ]
        );
        assert!(tool.should_defer() && tool.validate_input(&json!({"condition": ""})).is_err());
        assert_eq!(
            tool.schema()["properties"]["condition"]["maxLength"],
            MAX_SUGGESTED_CONDITION_CHARS
        );
    }

    /// D18: suggest_goal says what a goal does, that proposing doesn't wait, when to propose, and how to
    /// state a condition for a check that reads only the conversation. It
    /// stays deferred.
    #[test]
    fn suggest_goal_says_what_a_goal_does_and_when_to_propose_one() {
        let tool = SuggestGoalTool::new(new_handle());
        let text = tool.description();
        for part in [
            "an end state that keeps you working until a separate check confirms it is met",
            "It doesn't wait: the owner sees it on a card while you keep working.",
            "the work spans several turns. Not for one-off tasks",
            "The check reads only the conversation; it can't run commands or open files.",
            "If the owner declines, you aren't told; don't ask about it or propose it again.",
        ] {
            assert!(text.contains(part), "{part:?} missing from:\n{text}");
        }
        assert!(tool.should_defer());
    }
}
