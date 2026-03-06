use std::sync::Arc;

use serde::Deserialize;

use crate::registry::{DynTool, ToolResult};
use crate::origin::ToolContext;
use super::manager::NappManager;

/// STRAP domain tool for managing and dispatching to installed .napp tools.
///
/// - `tool(action: "list")` — list installed tools
/// - `tool(action: "install", code: "TOOL-XXXX-XXXX")` — install from marketplace
/// - `tool(action: "uninstall", id: "tool-id")` — uninstall a tool
/// - `tool(action: "sideload", path: "/path/to/dev/dir")` — sideload for development
/// - `tool(resource: "my-tool", action: "do-thing", ...)` — dispatch to an installed tool
pub struct ToolTool {
    manager: Arc<dyn NappManager>,
}

#[derive(Deserialize)]
struct ToolInput {
    #[serde(default)]
    resource: String,
    #[serde(default)]
    action: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    path: String,
}

impl ToolTool {
    pub fn new(manager: Arc<dyn NappManager>) -> Self {
        Self { manager }
    }

    async fn execute_inner(&self, ctx: &ToolContext, input: serde_json::Value) -> ToolResult {
        let parsed: ToolInput = match serde_json::from_value(input.clone()) {
            Ok(v) => v,
            Err(e) => return ToolResult::error(format!("invalid input: {}", e)),
        };

        // If resource is set, dispatch to that installed tool
        if !parsed.resource.is_empty() {
            return self.manager.dispatch(&parsed.resource, input, ctx).await;
        }

        // Otherwise, handle lifecycle actions
        match parsed.action.as_str() {
            "list" => {
                let tools = self.manager.list().await;
                let names = self.manager.tool_names().await;
                let json = serde_json::json!({
                    "tools": tools,
                    "total": tools.len(),
                    "dispatchable": names,
                });
                ToolResult::ok(serde_json::to_string_pretty(&json).unwrap_or_default())
            }
            "install" => {
                if parsed.code.is_empty() {
                    return ToolResult::error("code is required (format: TOOL-XXXX-XXXX)");
                }
                match self.manager.install(&parsed.code).await {
                    Ok(info) => {
                        let json = serde_json::json!({
                            "installed": true,
                            "tool": info,
                        });
                        ToolResult::ok(serde_json::to_string_pretty(&json).unwrap_or_default())
                    }
                    Err(e) => ToolResult::error(format!("install failed: {}", e)),
                }
            }
            "uninstall" => {
                if parsed.id.is_empty() {
                    return ToolResult::error("id is required");
                }
                match self.manager.uninstall(&parsed.id).await {
                    Ok(()) => ToolResult::ok(format!("Tool {} uninstalled", parsed.id)),
                    Err(e) => ToolResult::error(format!("uninstall failed: {}", e)),
                }
            }
            "sideload" => {
                if parsed.path.is_empty() {
                    return ToolResult::error("path is required");
                }
                match self.manager.sideload(&parsed.path).await {
                    Ok(info) => {
                        let json = serde_json::json!({
                            "sideloaded": true,
                            "tool": info,
                        });
                        ToolResult::ok(serde_json::to_string_pretty(&json).unwrap_or_default())
                    }
                    Err(e) => ToolResult::error(format!("sideload failed: {}", e)),
                }
            }
            "" => ToolResult::error("action is required. Use: list, install, uninstall, sideload. Or set resource to dispatch to an installed tool."),
            other => ToolResult::error(format!(
                "unknown action: {:?}. Use: list, install, uninstall, sideload. Or set resource to dispatch to an installed tool.",
                other
            )),
        }
    }
}

impl DynTool for ToolTool {
    fn name(&self) -> &str {
        "tool"
    }

    fn description(&self) -> String {
        "Manage and use installed .napp tools. Use action: list/install/uninstall/sideload for lifecycle, or set resource to dispatch to an installed tool.".to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "resource": {
                    "type": "string",
                    "description": "Name of an installed tool to dispatch to. Leave empty for lifecycle actions."
                },
                "action": {
                    "type": "string",
                    "description": "Lifecycle: list, install, uninstall, sideload. Dispatch: any action the installed tool supports."
                },
                "code": {
                    "type": "string",
                    "description": "Marketplace code for install (TOOL-XXXX-XXXX)"
                },
                "id": {
                    "type": "string",
                    "description": "Tool ID for uninstall"
                },
                "path": {
                    "type": "string",
                    "description": "Local directory path for sideload"
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
