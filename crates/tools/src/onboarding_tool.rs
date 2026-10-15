//! `onboarding`: the assistant's side of the guided first-run chat. The
//! steps and what each answer changes live on the server
//! (`handlers::onboarding` in nebo-server), behind [`FirstRun`]; this tool is
//! how the assistant reads the step and reports each answer.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// The guided first-run chat as the server runs it. Defined in tools crate,
/// implemented in server crate.
pub trait FirstRun: Send + Sync {
    /// The step it is on and what to do now, as JSON text.
    fn status<'a>(&'a self) -> Pin<Box<dyn Future<Output = String> + Send + 'a>>;

    /// The owner's answer at `step`: what comes next, as JSON text, or why
    /// the answer can't be taken.
    fn answer<'a>(
        &'a self,
        step: &'a str,
        value: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;
}

pub struct OnboardingTool {
    first_run: Arc<dyn FirstRun>,
}

impl OnboardingTool {
    pub fn new(first_run: Arc<dyn FirstRun>) -> Self {
        Self { first_run }
    }

    async fn run(&self, ctx: &ToolContext, input: &Value) -> ToolResult {
        if types::keyparser::extract_agent_id(&ctx.session_key) != crate::team_tool::PRIMARY_AGENT_ID {
            return ToolResult::error("Only the assistant runs the first-run chat.");
        }
        match input["action"].as_str().unwrap_or("status") {
            "status" => ToolResult::ok(self.first_run.status().await),
            "answer" => {
                let step = input["step"].as_str().unwrap_or("");
                let value = input["value"].as_str().unwrap_or("");
                match self.first_run.answer(step, value).await {
                    Ok(next) => ToolResult::ok(next),
                    Err(e) => ToolResult::error(e),
                }
            }
            other => ToolResult::error(format!("Unknown action '{other}': use status or answer.")),
        }
    }
}

impl DynTool for OnboardingTool {
    fn name(&self) -> &str {
        "onboarding"
    }

    fn description(&self) -> String {
        "The guided first-run chat with the owner (the assistant only).\n\
         - status: the step you are on and what to do now.\n\
         - answer: report the owner's answer at a step; it says what comes next.\n\
         - At any step, value \"skip\" moves on and \"finish\" ends the first-run chat."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["status", "answer"] },
                "step": { "type": "string", "enum": ["name", "focus", "connect", "plate", "day", "daily", "hire", "set"] },
                "value": { "type": "string", "description": "The answer, as the step's instructions name it." }
            },
            "required": ["action"]
        })
    }

    fn search_hint(&self) -> &str {
        "first-run guided chat onboarding steps"
    }

    fn read_only(&self, input: &Value) -> bool {
        input["action"].as_str() != Some("answer")
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move { self.run(ctx, &input).await })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A first-run chat that answers from fixed text, for registry tests.
    pub(crate) struct Fixed;

    impl FirstRun for Fixed {
        fn status<'a>(&'a self) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
            Box::pin(async { json!({ "step": "name" }).to_string() })
        }
        fn answer<'a>(&'a self, step: &'a str, _value: &'a str) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
            Box::pin(async move { Ok(json!({ "after": step }).to_string()) })
        }
    }
}
