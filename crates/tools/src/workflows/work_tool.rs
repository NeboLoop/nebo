use std::sync::Arc;

use serde::Deserialize;

use super::manager::WorkflowManager;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

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
    #[serde(default)]
    agent: String,
}

impl WorkTool {
    pub fn new(manager: Arc<dyn WorkflowManager>) -> Self {
        Self { manager }
    }

    /// Derive the calling agent's id from the session key ("agent:{id}:{channel}").
    /// Workflows are agent-owned, so creation/listing is always scoped to the
    /// agent whose session invoked the tool.
    fn calling_agent_id(ctx: &ToolContext) -> &str {
        if ctx.session_key.starts_with("agent:") {
            if let Some(id) = ctx.session_key.split(':').nth(1) {
                if !id.is_empty() {
                    return id;
                }
            }
        }
        ""
    }

    async fn execute_inner(&self, ctx: &ToolContext, input: serde_json::Value) -> ToolResult {
        let parsed: WorkInput = match serde_json::from_value(input) {
            Ok(v) => v,
            Err(e) => return ToolResult::error(format!("invalid input: {}", e)),
        };

        // Workflows belong to an agent. Default scope is the CALLING agent
        // (from the session key); an explicit `agent` reference re-targets the
        // call — resolved strictly, so a typo'd name errors instead of
        // silently self-scoping (which is how weekend workflows once landed on
        // the assistant instead of the employee they were meant for).
        let resolved;
        let agent_id = if parsed.agent.is_empty() {
            Self::calling_agent_id(ctx)
        } else {
            match self.manager.resolve_agent(&parsed.agent).await {
                Ok(id) => {
                    resolved = id;
                    &resolved
                }
                Err(e) => return ToolResult::error(e),
            }
        };

        // If resource is set, dispatch to that workflow
        if !parsed.resource.is_empty() {
            return self.dispatch_to_workflow(agent_id, &parsed).await;
        }

        // Otherwise, handle lifecycle actions
        match parsed.action.as_str() {
            "list" => {
                let workflows = self.manager.list(agent_id).await;
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
                let target = if !parsed.id.is_empty() {
                    &parsed.id
                } else {
                    ""
                };
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
                if agent_id.is_empty() {
                    return ToolResult::error(
                        "workflow creation must be scoped to an agent (no agent in this session)",
                    );
                }
                match self
                    .manager
                    .create(agent_id, &parsed.name, &parsed.definition)
                    .await
                {
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
            "update" | "edit" => {
                if parsed.definition.is_empty() {
                    return ToolResult::error("definition is required (the full replacement workflow JSON — update is not a partial patch)");
                }
                if parsed.name.is_empty() {
                    return ToolResult::error("name is required (the workflow's name)");
                }
                if agent_id.is_empty() {
                    return ToolResult::error("no agent in this session");
                }
                match self
                    .manager
                    .update(agent_id, &parsed.name, &parsed.definition)
                    .await
                {
                    Ok(info) => {
                        let json = serde_json::json!({
                            "updated": true,
                            "workflow": info,
                        });
                        ToolResult::ok(serde_json::to_string_pretty(&json).unwrap_or_default())
                    }
                    Err(e) => ToolResult::error(format!("update failed: {}", e)),
                }
            }
            "delete" => {
                if parsed.name.is_empty() {
                    return ToolResult::error("name is required (the workflow's name)");
                }
                if agent_id.is_empty() {
                    return ToolResult::error("no agent in this session");
                }
                match self.manager.delete(agent_id, &parsed.name).await {
                    Ok(()) => ToolResult::ok(format!("Workflow '{}' deleted", parsed.name)),
                    Err(e) => ToolResult::error(format!("delete failed: {}", e)),
                }
            }
            "" => ToolResult::error(
                "action is required. Use: list, create, update, delete, install, uninstall, cancel. Or set resource to dispatch to a workflow.",
            ),
            other => ToolResult::error(format!(
                "unknown action: {:?}. Use: list, create, install, uninstall, cancel. Or set resource to dispatch to a workflow.",
                other
            )),
        }
    }

    async fn dispatch_to_workflow(&self, agent_id: &str, parsed: &WorkInput) -> ToolResult {
        // Resolve workflow by name or id — the calling agent's own bindings
        // are matched first, so anything list() shows is dispatchable.
        let info = match self.manager.resolve(agent_id, &parsed.resource).await {
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
            "toggle" => match self.manager.toggle(&info.id).await {
                Ok(enabled) => {
                    let state = if enabled { "enabled" } else { "disabled" };
                    ToolResult::ok(format!("Workflow {:?} is now {}", info.name, state))
                }
                Err(e) => ToolResult::error(format!("toggle failed: {}", e)),
            },
            "" => ToolResult::error(
                "action is required when resource is set. Use: run, status, runs, toggle.",
            ),
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
        "Workflow management & execution. Workflows BELONG TO AN AGENT: calls scope to the calling agent by default; pass agent: \"Name\" to manage another agent's workflows (e.g. when the owner asks you to change an employee's duties — the workflow goes on THAT employee, never on yourself).\n\
         USE THIS when: user wants to manage or run automated workflows.\n\
         (agent(resource: \"registry\", automations/add_automations) also works when creating/configuring an agent wholesale.)\n\n\
         Lifecycle actions (no resource):\n\
         - work(action: \"list\") — List this agent's workflows and their status (add agent: \"Name\" for another agent's)\n\
         - work(action: \"create\", name: \"My Workflow\", agent: \"Content Creator\", definition: \"{\\\"trigger\\\": {\\\"type\\\": \\\"schedule\\\", \\\"cron\\\": \\\"0 9 * * MON-FRI\\\"}, \\\"activities\\\": [{\\\"id\\\": \\\"run\\\", \\\"intent\\\": \\\"...\\\", \\\"steps\\\": [\\\"concrete step\\\", ...]}]}\") — Create a workflow the target agent owns (appears in its Workflows panel and fires on its trigger; omit agent to create on yourself). Activities are the ONLY executable unit: each runs as its own scoped execution of its intent + steps. A top-level `steps` array is accepted as shorthand for one activity. Omit trigger for a manual workflow.\n\
         - work(action: \"update\", name: \"my-workflow\", definition: \"{...}\") — Full-replacement edit of an existing workflow (same definition shape as create; run history stays attached; errors if the name doesn't exist)\n\
         - work(action: \"delete\", name: \"my-workflow\") — Delete one of this agent's workflows by name\n\
         - work(action: \"install\", code: \"WORK-XXXX-XXXX\") — Install from marketplace\n\
         - work(action: \"uninstall\", id: \"workflow-id\") — Uninstall a marketplace-installed workflow (by its install id, not name)\n\n\
         Dispatch to workflow (set resource = workflow name):\n\
         - work(resource: \"weekly-report\", action: \"run\") — Run the workflow (returns immediately with run_id)\n\
         - work(resource: \"weekly-report\", action: \"run\", inputs: {\"week\": \"2024-03\"}) — Run with inputs\n\
         - work(resource: \"weekly-report\", action: \"status\") — Check latest run status\n\
         - work(resource: \"weekly-report\", action: \"runs\") — List recent runs\n\
         - work(resource: \"weekly-report\", action: \"toggle\") — Enable/disable\n\n\
         First use work(action: \"list\") to see available workflows, then dispatch with resource.\n\
         Workflows run as background subagents — run returns a run_id immediately."
            .to_string()
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
                    "description": "Lifecycle: list, create, update, delete, install, uninstall. Dispatch: run, status, runs, toggle."
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
                    "description": "Workflow name (for create/update/delete)"
                },
                "definition": {
                    "type": "string",
                    "description": "Workflow JSON definition (for create)"
                },
                "agent": {
                    "type": "string",
                    "description": "Target agent (name or id) whose workflows to manage. Defaults to the calling agent — set this whenever the workflow belongs to a different agent/employee."
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
