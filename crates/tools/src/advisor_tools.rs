//! The advisor panel: `consult_advisors` puts a question to the configured
//! advisors and returns their deliberation; `list_advisors` lists them.

use std::sync::Arc;

use db::Store;
use serde_json::{Value, json};

use crate::bot_tool::AdvisorDeliberator;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

pub struct Advisors {
    store: Arc<Store>,
    runner: Option<Arc<dyn AdvisorDeliberator>>,
}

impl Advisors {
    pub fn new(store: Arc<Store>, runner: Option<Arc<dyn AdvisorDeliberator>>) -> Self {
        Self { store, runner }
    }

    pub fn tools(self) -> Vec<Box<dyn DynTool>> {
        let advisors = Arc::new(self);
        [AdvisorOp::Consult, AdvisorOp::List]
            .into_iter()
            .map(|op| {
                Box::new(AdvisorTool {
                    op,
                    advisors: advisors.clone(),
                }) as Box<dyn DynTool>
            })
            .collect()
    }

    async fn consult(&self, input: &Value) -> ToolResult {
        let question = input["question"].as_str().unwrap_or("");
        // A live deliberation when the advisor engine is available.
        if let Some(ref runner) = self.runner {
            return match runner.deliberate(question).await {
                Ok(output) if output.is_empty() => ToolResult::ok(format!(
                    "No advisors configured. Proceeding with own judgment on: {question}"
                )),
                Ok(output) => ToolResult::ok(output),
                Err(e) => ToolResult::error(format!("Advisor deliberation failed: {e}")),
            };
        }
        // Otherwise the configured personas, with no model calls.
        match self.store.list_advisors() {
            Ok(advisors) => {
                let enabled: Vec<_> = advisors.iter().filter(|a| a.enabled != 0).collect();
                if enabled.is_empty() {
                    return ToolResult::ok(format!(
                        "No advisors configured. Proceeding with own judgment on: {question}"
                    ));
                }
                let perspectives: Vec<String> = enabled
                    .iter()
                    .map(|a| {
                        let persona = if a.persona.is_empty() {
                            "general advisor"
                        } else {
                            &a.persona
                        };
                        let role = if a.role.is_empty() {
                            "advisor"
                        } else {
                            &a.role
                        };
                        format!(
                            "**{}** ({role}): Consider this from the perspective of {persona}.",
                            a.name
                        )
                    })
                    .collect();
                ToolResult::ok(format!(
                    "No live deliberation ran (the advisor engine isn't available); these are the \
                     configured advisor personas only.\n\nQuestion: {question}\n\n{}\n\nSynthesize \
                     these perspectives to form your approach.",
                    perspectives.join("\n\n"),
                ))
            }
            Err(e) => ToolResult::error(format!("Failed to load advisors: {e}")),
        }
    }

    fn list(&self) -> ToolResult {
        match self.store.list_advisors() {
            Ok(advisors) if advisors.is_empty() => ToolResult::ok("No advisors configured."),
            Ok(advisors) => {
                let lines: Vec<String> = advisors
                    .iter()
                    .map(|a| {
                        let enabled = if a.enabled != 0 {
                            "enabled"
                        } else {
                            "disabled"
                        };
                        let desc = if a.description.is_empty() {
                            "-"
                        } else {
                            &a.description
                        };
                        format!("- {} [{enabled}] — {desc}", a.name)
                    })
                    .collect();
                ToolResult::ok(format!(
                    "{} advisors:\n{}",
                    advisors.len(),
                    lines.join("\n")
                ))
            }
            Err(e) => ToolResult::error(format!("Failed to list advisors: {e}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdvisorOp {
    Consult,
    List,
}

struct AdvisorTool {
    op: AdvisorOp,
    advisors: Arc<Advisors>,
}

impl DynTool for AdvisorTool {
    fn name(&self) -> &str {
        match self.op {
            AdvisorOp::Consult => "consult_advisors",
            AdvisorOp::List => "list_advisors",
        }
    }

    fn description(&self) -> String {
        match self.op {
            AdvisorOp::Consult => "Puts a question to the owner's advisor panel and returns their deliberation: each advisor's view, then where they agree and differ.\n\
                 - For a decision with real trade-offs, not for facts you can look up."
                .to_string(),
            AdvisorOp::List => "Lists the advisors on the panel and whether each is enabled.".to_string(),
        }
    }

    fn schema(&self) -> Value {
        match self.op {
            AdvisorOp::Consult => json!({
                "type": "object",
                "properties": {
                    "question": { "type": "string", "description": "The decision or question, with the context the advisors need." }
                },
                "required": ["question"]
            }),
            AdvisorOp::List => json!({ "type": "object", "properties": {} }),
        }
    }

    fn search_hint(&self) -> &str {
        match self.op {
            AdvisorOp::Consult => "ask the advisor panel a decision",
            AdvisorOp::List => "list the advisor panel",
        }
    }

    fn read_only(&self, _input: &Value) -> bool {
        self.op == AdvisorOp::List
    }

    /// A deliberation is the employee's own thinking.
    fn effects(&self, _input: &Value) -> types::permissions::CallEffects {
        types::permissions::CallEffects::none()
    }

    fn validate_input(&self, input: &Value) -> Result<(), String> {
        if self.op == AdvisorOp::Consult
            && input["question"]
                .as_str()
                .is_none_or(|q| q.trim().is_empty())
        {
            return Err("question can't be empty.".to_string());
        }
        Ok(())
    }

    fn activity(&self, _input: &Value) -> String {
        match self.op {
            AdvisorOp::Consult => "consulting the advisors".to_string(),
            AdvisorOp::List => "checking the advisors".to_string(),
        }
    }

    fn outcome(&self, _input: &Value) -> String {
        match self.op {
            AdvisorOp::Consult => "Consulted the advisors".to_string(),
            AdvisorOp::List => "Checked the advisors".to_string(),
        }
    }

    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        input: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            match self.op {
                AdvisorOp::Consult => self.advisors.consult(&input).await,
                AdvisorOp::List => self.advisors.list(),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Panel;

    impl AdvisorDeliberator for Panel {
        fn deliberate<'a>(
            &'a self,
            task: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
        {
            Box::pin(async move { Ok(format!("the panel on: {task}")) })
        }
    }

    #[tokio::test]
    async fn a_question_reaches_the_panel() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(&dir.path().join("a.db").to_string_lossy()).unwrap());
        let tools = Advisors::new(store.clone(), Some(Arc::new(Panel))).tools();
        let r = tools[0]
            .execute_dyn(
                &ToolContext::default(),
                json!({"question": "SQLite or Postgres?"}),
            )
            .await;
        assert_eq!(r.content, "the panel on: SQLite or Postgres?");
        assert!(tools[0].validate_input(&json!({"question": ""})).is_err());
        // Without the engine: the configured personas, no model call.
        let offline = Advisors::new(store, None).tools();
        let r = offline[0]
            .execute_dyn(
                &ToolContext::default(),
                json!({"question": "SQLite or Postgres?"}),
            )
            .await;
        assert!(
            !r.is_error && r.content.contains("Question: SQLite or Postgres?"),
            "{}",
            r.content
        );
        assert!(
            !offline[1]
                .execute_dyn(&ToolContext::default(), json!({}))
                .await
                .is_error
        );
    }
}
