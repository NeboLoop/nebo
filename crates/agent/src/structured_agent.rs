//! `StructuredRunner` — the agent-crate implementation of `tools::bot_tool::StructuredAgent`.
//!
//! The deep-research harness (in the tools crate) drives its sub-agents through this trait
//! so it never has to depend on the agent crate (which owns the AI providers). Each call
//! resolves a provider, wires the requested aux tools (currently `web`) into a forced
//! `StructuredOutput` loop via [`crate::structured::agent_structured`], and returns the
//! schema-validated JSON.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use ai::{Provider, ToolCall, ToolDefinition};
use serde_json::Value;

use tools::bot_tool::{StructuredAgent, StructuredTask};
use tools::origin::ToolContext;
use tools::registry::DynTool;
use tools::web_tool::WebTool;

use crate::structured::{StructuredRequest, agent_structured};

/// Runs forced-structured-output sub-agents for the deep-research harness.
pub struct StructuredRunner {
    providers: Arc<Vec<Arc<dyn Provider>>>,
    web: Arc<WebTool>,
}

impl StructuredRunner {
    pub fn new(providers: Arc<Vec<Arc<dyn Provider>>>, store: Arc<db::Store>) -> Self {
        Self {
            providers,
            web: Arc::new(WebTool::new().with_store(store)),
        }
    }
}

impl StructuredAgent for StructuredRunner {
    fn run<'a>(
        &'a self,
        task: StructuredTask,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>> {
        Box::pin(async move {
            let provider = self
                .providers
                .first()
                .ok_or_else(|| "no AI providers configured".to_string())?
                .clone();

            // Materialise the requested aux tools as ToolDefinitions. Only `web` is
            // offered to research sub-agents today.
            let mut aux = Vec::new();
            for name in &task.aux_tools {
                if name == "web" {
                    aux.push(ToolDefinition {
                        name: "web".to_string(),
                        description: self.web.description(),
                        input_schema: self.web.schema(),
                    });
                }
            }

            let req = StructuredRequest::new(task.system, task.task, task.schema, String::new())
                .with_aux_tools(aux);

            let web = self.web.clone();
            let tool_exec = move |tc: ToolCall| {
                let web = web.clone();
                async move {
                    if tc.name == "web" {
                        let ctx = ToolContext::default();
                        let result = web.execute_dyn(&ctx, tc.input).await;
                        if result.is_error {
                            Err(result.content)
                        } else {
                            Ok(result.content)
                        }
                    } else {
                        Err(format!(
                            "tool '{}' is not available to a research sub-agent",
                            tc.name
                        ))
                    }
                }
            };

            agent_structured(provider, req, tool_exec)
                .await
                .map_err(|e| e.to_string())
        })
    }
}
