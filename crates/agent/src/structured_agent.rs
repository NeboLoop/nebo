//! `StructuredRunner` — the agent-crate implementation of `tools::bot_tool::StructuredAgent`.
//!
//! The deep-research harness (tools crate) drives its sub-agents through this trait so it
//! never depends on the agent crate (which owns the AI providers). Every tool call — both
//! a sub-agent's aux-tool use and the harness's deterministic fetch — dispatches through
//! the canonical [`tools::Registry::execute`], so there is exactly ONE web pathway: the
//! fully-wired registry tool (browser, store, broadcaster). Each call carries the
//! sub-agent's `tab_key` as the `ToolContext` session, giving 1:1 sub-agent→browser-tab
//! ownership while siblings (`subagent:{parent}:sa-{id}`) share the parent's dedup cache.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use ai::{Provider, ToolCall};
use serde_json::Value;

use tools::Registry;
use tools::bot_tool::{StructuredAgent, StructuredTask};
use tools::origin::ToolContext;
use tools::registry::ToolResult;

use crate::structured::{StructuredRequest, agent_structured};

/// Runs forced-structured-output sub-agents (and single tool calls) for the deep-research
/// harness, dispatching everything through the canonical tool registry.
pub struct StructuredRunner {
    providers: Arc<Vec<Arc<dyn Provider>>>,
    registry: Arc<Registry>,
}

impl StructuredRunner {
    pub fn new(providers: Arc<Vec<Arc<dyn Provider>>>, registry: Arc<Registry>) -> Self {
        Self { providers, registry }
    }

    /// A `ToolContext` scoped to one sub-agent's tab/session. `session_id` keys the
    /// browser tab (1:1 ownership); `session_key`'s `subagent:{parent}:sa-{id}` shape lets
    /// `web_tool::session_group_key` share the parent's visited-page cache across siblings.
    fn ctx_for(tab_key: &str) -> ToolContext {
        let mut ctx = ToolContext::default();
        ctx.session_key = tab_key.to_string();
        ctx.session_id = tab_key.to_string();
        ctx
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

            // Offer the requested aux tools from the canonical registry — these are the
            // real, fully-wired tool definitions (e.g. the browser-enabled `web`).
            let mut aux = Vec::new();
            for name in &task.aux_tools {
                if let Some(def) = self.registry.definition(name).await {
                    aux.push(def);
                }
            }

            let req = StructuredRequest::new(task.system, task.task, task.schema, String::new())
                .with_aux_tools(aux);

            let registry = self.registry.clone();
            let ctx = Self::ctx_for(&task.tab_key);
            let tool_exec = move |tc: ToolCall| {
                let registry = registry.clone();
                let ctx = ctx.clone();
                async move {
                    let result = registry.execute(&ctx, &tc.name, tc.input).await;
                    if result.is_error {
                        Err(result.content)
                    } else {
                        Ok(result.content)
                    }
                }
            };

            agent_structured(provider, req, tool_exec)
                .await
                .map_err(|e| e.to_string())
        })
    }

    fn execute_tool<'a>(
        &'a self,
        tab_key: String,
        tool: String,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let ctx = Self::ctx_for(&tab_key);
            self.registry.execute(&ctx, &tool, input).await
        })
    }

    fn close_tab<'a>(&'a self, tab_key: String) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.registry.close_browser_session(&tab_key).await;
        })
    }
}
