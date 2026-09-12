//! `context` — the ONE way a seat writes its own context section.
//!
//! A seat meets its industry, franchise, and company layers by reading them
//! once per change in an update run and writing what matters to its job.
//! That section is resident in the seat's static prompt; nothing about the
//! layers is fetched at work time. This tool is the write half. The read
//! half is the prompt.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent_tool::AgentRegistry;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

pub struct ContextTool {
    store: Arc<db::Store>,
    /// The live registry the runner reads; updated in place so the next turn
    /// carries the new section without a restart.
    agent_registry: Option<AgentRegistry>,
}

impl ContextTool {
    pub fn new(store: Arc<db::Store>, agent_registry: Option<AgentRegistry>) -> Self {
        Self { store, agent_registry }
    }
}

impl DynTool for ContextTool {
    fn name(&self) -> &str {
        "context"
    }

    fn description(&self) -> String {
        "Your own context section: what you know about this company and its trade, written by you from the industry, franchise, and company layers. \
         `write` replaces the section (call it at the end of an update run, once, with the whole section). `show` returns the current section and what it was written against."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["write", "show"] },
                "section": {
                    "type": "string",
                    "description": "The whole section, in your own words: the facts and rules you will work by. Markdown. Only for `write`."
                }
            },
            "required": ["action"]
        })
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
        let agent_id = types::keyparser::extract_agent_id(&ctx.session_key);
        if agent_id.is_empty() {
            return ToolResult::error("context: this session is not an employee's");
        }
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("show");
        match action {
            "show" => match self.store.get_agent(&agent_id) {
                Ok(Some(a)) => ToolResult::ok(
                    json!({
                        "section": a.context_section.unwrap_or_default(),
                        "stamp": a.context_stamp.and_then(|s| serde_json::from_str::<Value>(&s).ok()).unwrap_or(Value::Null),
                    })
                    .to_string(),
                ),
                Ok(None) => ToolResult::error("context: employee not found"),
                Err(e) => ToolResult::error(format!("context: {e}")),
            },
            "write" => {
                let section = input.get("section").and_then(|v| v.as_str()).unwrap_or("").trim();
                if section.is_empty() {
                    return ToolResult::error("context write: `section` is required and must not be empty");
                }
                let stamp = match self.store.get_agent(&agent_id) {
                    Ok(Some(a)) => a
                        .context_stamp
                        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                        .unwrap_or_else(|| json!({})),
                    _ => json!({}),
                };
                let mut stamp = stamp;
                stamp["status"] = json!("written");
                stamp["written_at"] = json!(chrono::Utc::now().timestamp());
                stamp["written_in"] = json!(ctx.session_key);
                if let Err(e) = self.store.set_agent_context_section(&agent_id, section, &stamp.to_string()) {
                    return ToolResult::error(format!("context write: {e}"));
                }
                if let Some(reg) = &self.agent_registry {
                    if let Some(entry) = reg.write().await.get_mut(&agent_id) {
                        entry.context_section = Some(section.to_string());
                    }
                }
                ToolResult::ok(format!(
                    "Context section written ({} chars). It is in your instructions from your next turn.",
                    section.chars().count()
                ))
            }
            other => ToolResult::error(format!("context: unknown action `{other}`")),
        }
        })
    }
}
