use std::sync::Arc;

use serde::Deserialize;

use crate::registry::{DynTool, ToolResult};
use crate::origin::ToolContext;
use super::manager::WorkflowManager;

/// STRAP domain tool for managing and running workflows.
///
/// - `work(action: "list")` — list installed workflows
/// - `work(action: "install", code: "WORK-XXXX-XXXX")` — install from marketplace
/// - `work(action: "uninstall", id: "workflow-id")` — uninstall a workflow
/// - `work(resource: "my-workflow", action: "run")` — run a workflow (returns run_id)
/// - `work(resource: "my-workflow", action: "status")` — latest run status
/// - `work(resource: "my-workflow", action: "runs")` — list recent runs
/// - `work(resource: "my-workflow", action: "toggle")` — enable/disable
/// - `work(action: "cancel", id: "run-id")` — cancel a running workflow
pub struct WorkTool {
    manager: Arc<dyn WorkflowManager>,
}

#[derive(Deserialize)]
struct WorkInput {
    #[serde(default)]
    resource: String,
    #[serde(default)]
    action: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    inputs: serde_json::Value,
    #[serde(default)]
    name: String,
    #[serde(default)]
    definition: String,
}

impl WorkTool {
    pub fn new(manager: Arc<dyn WorkflowManager>) -> Self {
        Self { manager }
    }

    async fn execute_inner(&self, _ctx: &ToolContext, input: serde_json::Value) -> ToolResult {
        let parsed: WorkInput = match serde_json::from_value(input) {
            Ok(v) => v,
            Err(e) => return ToolResult::error(format!("invalid input: {}", e)),
        };

        // If resource is set, dispatch to that workflow
        if !parsed.resource.is_empty() {
            return self.dispatch_to_workflow(&parsed).await;
        }

        // Otherwise, handle lifecycle actions
        match parsed.action.as_str() {
            "list" => {
                let workflows = self.manager.list().await;
                let json = serde_json::json!({
                    "workflows": workflows,
                    "total": workflows.len(),
                });
                ToolResult::ok(serde_json::to_string_pretty(&json).unwrap_or_default())
            }
            "install" => {
                if parsed.code.is_empty() {
                    return ToolResult::error("code is required (format: WORK-XXXX-XXXX)");
                }
                match self.manager.install(&parsed.code).await {
                    Ok(info) => {
                        let json = serde_json::json!({
                            "installed": true,
                            "workflow": info,
                        });
                        ToolResult::ok(serde_json::to_string_pretty(&json).unwrap_or_default())
                    }
                    Err(e) => ToolResult::error(format!("install failed: {}", e)),
                }
            }
            "uninstall" => {
                let target = if !parsed.id.is_empty() { &parsed.id } else { "" };
                if target.is_empty() {
                    return ToolResult::error("id is required");
                }
                match self.manager.uninstall(target).await {
                    Ok(()) => ToolResult::ok(format!("Workflow {} uninstalled", target)),
                    Err(e) => ToolResult::error(format!("uninstall failed: {}", e)),
                }
            }
            "cancel" => {
                if parsed.id.is_empty() {
                    return ToolResult::error("id is required (run ID)");
                }
                match self.manager.cancel(&parsed.id).await {
                    Ok(()) => ToolResult::ok(format!("Workflow run {} cancelled", parsed.id)),
                    Err(e) => ToolResult::error(format!("cancel failed: {}", e)),
                }
            }
            "create" => {
                if parsed.definition.is_empty() {
                    return ToolResult::error("definition is required (workflow JSON)");
                }
                if parsed.name.is_empty() {
                    return ToolResult::error("name is required");
                }
                match self.manager.create(&parsed.name, &parsed.definition).await {
                    Ok(info) => {
                        let json = serde_json::json!({
                            "created": true,
                            "workflow": info,
                        });
                        ToolResult::ok(serde_json::to_string_pretty(&json).unwrap_or_default())
                    }
                    Err(e) => ToolResult::error(format!("create failed: {}", e)),
                }
            }
            "" => ToolResult::error("action is required. Use: list, create, install, uninstall, cancel. Or set resource to dispatch to a workflow."),
            other => ToolResult::error(format!(
                "unknown action: {:?}. Use: list, create, install, uninstall, cancel. Or set resource to dispatch to a workflow.",
                other
            )),
        }
    }

    async fn dispatch_to_workflow(&self, parsed: &WorkInput) -> ToolResult {
        // Resolve workflow by name or id
        let info = match self.manager.resolve(&parsed.resource).await {
            Ok(i) => i,
            Err(e) => return ToolResult::error(format!("workflow not found: {}", e)),
        };

        match parsed.action.as_str() {
            "run" => {
                let inputs = if parsed.inputs.is_null() {
                    serde_json::json!({})
                } else {
                    parsed.inputs.clone()
                };
                match self.manager.run(&info.id, inputs, "agent").await {
                    Ok(run_id) => {
                        let json = serde_json::json!({
                            "started": true,
                            "runId": run_id,
                            "workflow": info.name,
                            "message": "Workflow started in background. Use status to check progress.",
                        });
                        ToolResult::ok(serde_json::to_string_pretty(&json).unwrap_or_default())
                    }
                    Err(e) => ToolResult::error(format!("run failed: {}", e)),
                }
            }
            "status" => {
                // Get latest run
                let runs = self.manager.list_runs(&info.id, 1).await;
                match runs.first() {
                    Some(run) => {
                        let json = serde_json::to_value(run).unwrap_or_default();
                        ToolResult::ok(serde_json::to_string_pretty(&json).unwrap_or_default())
                    }
                    None => ToolResult::ok(format!("No runs found for workflow {:?}", info.name)),
                }
            }
            "runs" => {
                let runs = self.manager.list_runs(&info.id, 10).await;
                let json = serde_json::json!({
                    "runs": runs,
                    "total": runs.len(),
                    "workflow": info.name,
                });
                ToolResult::ok(serde_json::to_string_pretty(&json).unwrap_or_default())
            }
            "toggle" => {
                match self.manager.toggle(&info.id).await {
                    Ok(enabled) => {
                        let state = if enabled { "enabled" } else { "disabled" };
                        ToolResult::ok(format!("Workflow {:?} is now {}", info.name, state))
                    }
                    Err(e) => ToolResult::error(format!("toggle failed: {}", e)),
                }
            }
            "" => ToolResult::error("action is required when resource is set. Use: run, status, runs, toggle."),
            other => ToolResult::error(format!(
                "unknown action {:?} for workflow {:?}. Use: run, status, runs, toggle.",
                other, info.name
            )),
        }
    }
}

impl DynTool for WorkTool {
    fn name(&self) -> &str {
        "work"
    }

    fn description(&self) -> String {
        "Manage and run automated workflows. Use action: list/create/install/uninstall for lifecycle, or set resource to a workflow name for: run/status/runs/toggle.".to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "resource": {
                    "type": "string",
                    "description": "Name or ID of a workflow to dispatch to. Leave empty for lifecycle actions."
                },
                "action": {
                    "type": "string",
                    "description": "Lifecycle: list, create, install, uninstall. Dispatch: run, status, runs, toggle."
                },
                "code": {
                    "type": "string",
                    "description": "Marketplace code for install (WORK-XXXX-XXXX)"
                },
                "id": {
                    "type": "string",
                    "description": "Workflow ID for uninstall"
                },
                "inputs": {
                    "type": "object",
                    "description": "Input parameters for workflow run"
                },
                "name": {
                    "type": "string",
                    "description": "Workflow name (for create)"
                },
                "definition": {
                    "type": "string",
                    "description": "Workflow JSON definition (for create)"
                }
            },
            "required": ["action"],
            "additionalProperties": true
        })
    }

    fn requires_approval(&self) -> bool {
        true
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(self.execute_inner(ctx, input))
    }
}
