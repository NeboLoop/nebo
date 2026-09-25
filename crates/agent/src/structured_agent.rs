//! `StructuredRunner` — the agent-crate implementation of `tools::bot_tool::StructuredAgent`.
//!
//! The deep-research pipeline (tools crate) drives its sub-agents through this trait so it
//! never depends on the agent crate. Each sub-agent is a helper of the research run, a turn
//! on the one loop (`Helpers::answer_as_data`): the harness's model call, tool round,
//! permission check and stop token, with its answer's shape held by the turn's end check.
//! There is no second model loop. The pipeline's deterministic fetches dispatch through the
//! canonical [`tools::Registry::execute`] under the sub-agent's session, so each sub-agent
//! owns its browser tab while siblings (`subagent:<run>:sa-<node>`) share the run's
//! visited-page cache.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use serde_json::Value;

use tools::Registry;
use tools::bot_tool::{StructuredAgent, StructuredTask};
use tools::origin::ToolContext;
use tools::registry::ToolResult;

use crate::harness::delegation::{Helpers, split_helper_key};

/// Runs the research pipeline's sub-agents as helpers of the run, and its single tool
/// calls, through the canonical tool registry.
pub struct StructuredRunner {
    registry: Arc<Registry>,
    /// Bound once the helper registry exists (it is built after the tools).
    helpers: OnceLock<Arc<Helpers>>,
}

impl StructuredRunner {
    pub fn new(registry: Arc<Registry>) -> Self {
        Self { registry, helpers: OnceLock::new() }
    }

    /// The helper registry sub-agents run in. Once.
    pub fn bind(&self, helpers: Arc<Helpers>) {
        if self.helpers.set(helpers).is_err() {
            tracing::warn!("structured runner bound twice; the first binding stays");
        }
    }

    /// A `ToolContext` scoped to one sub-agent's tab/session. `session_id` keys the
    /// browser tab (1:1 ownership); `session_key`'s `subagent:{run}:sa-{id}` shape lets
    /// `web_tool::session_group_key` share the run's visited-page cache across siblings.
    fn ctx_for(tab_key: &str) -> ToolContext {
        let mut ctx = ToolContext::default();
        ctx.session_key = tab_key.to_string();
        ctx.session_id = tab_key.to_string();
        // A research helper: the permission check decides its calls under
        // the grant of the employee its session key names.
        ctx.door = types::permissions::Door::Helper;
        ctx
    }
}

/// What the sub-agent is told: its role, its task, and the shape its answer takes.
fn brief(task: &StructuredTask) -> String {
    let mut text = format!("{}\n\n{}", task.system.trim(), task.task.trim());
    if let Some(turns) = task.max_tool_turns {
        text.push_str(&format!("\n\nMake at most {turns} tool call{}.", if turns == 1 { "" } else { "s" }));
    }
    text.push_str(&format!(
        "\n\nYour final answer is read as data: one JSON object matching this schema, and nothing else.\n{}",
        task.schema
    ));
    text
}

impl StructuredAgent for StructuredRunner {
    fn run<'a>(
        &'a self,
        task: StructuredTask,
        activity: Option<Arc<std::sync::atomic::AtomicU64>>,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>> {
        Box::pin(async move {
            let helpers = self.helpers.get().ok_or("research can't run yet: the server is still starting")?;
            let (work_key, task_id) =
                split_helper_key(&task.tab_key).ok_or_else(|| format!("{} is not a helper's session", task.tab_key))?;
            let node = task_id.strip_prefix("sa-").unwrap_or(task_id);
            helpers
                .answer_as_data(work_key, node, brief(&task), task.schema.clone(), task.aux_tools.clone(), activity)
                .await
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
