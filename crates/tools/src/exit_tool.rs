//! Exit tool — allows a workflow activity to terminate the workflow early.
//! Always injected by the engine alongside emit. When called, signals the
//! engine to stop cleanly. The run is marked "exited" — not a failure.

use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

pub const EXIT_SENTINEL: &str = "__WORKFLOW_EXIT__:";

pub struct ExitTool;

impl ExitTool {
    pub fn new() -> Self { Self }
}

impl Default for ExitTool {
    fn default() -> Self { Self::new() }
}

impl DynTool for ExitTool {
    fn name(&self) -> &str { "exit" }

    fn description(&self) -> String {
        "Stop this workflow early when there is nothing meaningful to do — \
         no items found, condition not met, or task already complete. \
         This is a clean stop, not an error.\n\n\
         Examples:\n  \
         exit(reason: \"No urgent emails found\")\n  \
         exit(reason: \"Nothing new since last check\")\n  \
         exit(reason: \"Condition not met\")".to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "reason": {
                    "type": "string",
                    "description": "Why the workflow is exiting early"
                }
            },
            "required": ["reason"]
        })
    }

    fn requires_approval(&self) -> bool { false }

    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let reason = input["reason"].as_str().unwrap_or("workflow exited early");
            ToolResult::ok(format!("{}{}", EXIT_SENTINEL, reason))
        })
    }
}
