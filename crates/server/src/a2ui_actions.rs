//! Deterministic A2UI action dispatcher.
//!
//! Routes button clicks and other A2UI events by action type:
//! - `mcp_call` → MCP bridge tool call → result injected into data model
//! - `navigate` → view navigation (delegates to A2UIManager::navigate_view)
//! - `update_data` → direct data model update (no LLM, no MCP)
//! - `agent` (default) → existing path: build ChatConfig, call run_chat()

use std::sync::Arc;

use serde_json::json;
use tracing::{debug, info, warn};

use crate::a2ui::A2UIManager;
use crate::state::AppState;

/// Parsed action binding from views.json or inline action context.
#[derive(Debug)]
pub struct ActionBinding {
    pub action_type: String,
    /// For mcp_call: MCP server/integration slug
    pub server: Option<String>,
    /// For mcp_call: tool name on the MCP server
    pub tool: Option<String>,
    /// For mcp_call: arguments to pass to the tool
    pub args: Option<serde_json::Value>,
    /// For mcp_call: JSON Pointer path where result is injected into data model
    pub update_path: Option<String>,
    /// For navigate: target view ID
    pub view: Option<String>,
    /// For navigate/update_data: parameters or value
    pub params: Option<serde_json::Value>,
    /// For update_data: JSON Pointer path
    pub path: Option<String>,
    /// For update_data: value to set
    pub value: Option<serde_json::Value>,
    /// For agent: prompt template (may contain {{context.field}} placeholders)
    pub prompt_template: Option<String>,
}

impl ActionBinding {
    /// Parse an action binding from a JSON context object.
    pub fn from_context(ctx: &serde_json::Value) -> Option<Self> {
        let action_type = ctx
            .get("type")
            .or_else(|| ctx.get("actionType"))
            .and_then(|v| v.as_str())
            .unwrap_or("agent")
            .to_string();

        Some(Self {
            action_type,
            server: ctx
                .get("server")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            tool: ctx
                .get("tool")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            args: ctx.get("args").cloned(),
            update_path: ctx
                .get("update_path")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            view: ctx
                .get("view")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            params: ctx.get("params").cloned(),
            path: ctx
                .get("path")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            value: ctx.get("value").cloned(),
            prompt_template: ctx
                .get("prompt_template")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        })
    }

    /// Try to resolve an action binding from views.json for a given action name.
    pub fn from_views_json(
        views_json: &serde_json::Value,
        view_id: &str,
        action_name: &str,
    ) -> Option<Self> {
        let view = views_json.get(view_id)?;
        let actions = view.get("actions")?.as_object()?;
        let binding = actions.get(action_name)?;
        Self::from_context(binding)
    }
}

/// Dispatch an A2UI action based on its binding type.
///
/// Returns `true` if the action was handled deterministically (no LLM needed),
/// `false` if it should fall through to the default agent/LLM path.
pub async fn dispatch(
    state: &AppState,
    a2ui: &Arc<A2UIManager>,
    agent_id: &str,
    surface_id: &str,
    action_name: &str,
    _component_id: &str,
    raw_context: &serde_json::Value,
    views_json: Option<&serde_json::Value>,
) -> bool {
    // Try to resolve binding: first from inline context, then from views.json
    let binding = ActionBinding::from_context(raw_context)
        .filter(|b| b.action_type != "agent") // Only use inline if it's deterministic
        .or_else(|| {
            // Try views.json action bindings
            let view_id = surface_id.split(':').nth(2).unwrap_or("default");
            views_json.and_then(|v| ActionBinding::from_views_json(v, view_id, action_name))
        });

    let binding = match binding {
        Some(b) if b.action_type != "agent" => b,
        _ => return false, // Fall through to LLM
    };

    match binding.action_type.as_str() {
        "mcp_call" => {
            handle_mcp_call(state, a2ui, surface_id, &binding).await;
            true
        }
        "navigate" => {
            handle_navigate(state, a2ui, agent_id, surface_id, &binding, views_json).await;
            true
        }
        "update_data" => {
            handle_update_data(a2ui, surface_id, &binding).await;
            true
        }
        _ => false, // Unknown type → fall through to LLM
    }
}

/// Handle mcp_call action: call MCP tool and inject result into data model.
async fn handle_mcp_call(
    state: &AppState,
    a2ui: &Arc<A2UIManager>,
    surface_id: &str,
    binding: &ActionBinding,
) {
    let server = match &binding.server {
        Some(s) => s.clone(),
        None => {
            warn!("mcp_call action missing 'server' field");
            return;
        }
    };
    let tool = match &binding.tool {
        Some(t) => t.clone(),
        None => {
            warn!("mcp_call action missing 'tool' field");
            return;
        }
    };
    let args = binding.args.clone().unwrap_or(json!({}));

    debug!(server = %server, tool = %tool, "dispatching mcp_call action");

    match state.bridge.call_tool(&server, &tool, args).await {
        Ok(result) => {
            if let Some(ref path) = binding.update_path {
                // Inject result into the surface's data model at the specified path
                let value = serde_json::to_value(&result.content).unwrap_or(json!(null));
                if let Err(e) = a2ui.update_data_model(surface_id, Some(path), value).await {
                    warn!(error = %e, "mcp_call: failed to update data model");
                }
            }
            info!(server = %server, tool = %tool, "mcp_call action completed");
        }
        Err(e) => {
            warn!(error = %e, server = %server, tool = %tool, "mcp_call action failed");
            // Optionally inject error into data model
            if let Some(ref path) = binding.update_path {
                let error_val = json!({ "error": e.to_string() });
                let _ = a2ui
                    .update_data_model(surface_id, Some(path), error_val)
                    .await;
            }
        }
    }
}

/// Handle navigate action: switch to a different view.
async fn handle_navigate(
    state: &AppState,
    a2ui: &Arc<A2UIManager>,
    agent_id: &str,
    surface_id: &str,
    binding: &ActionBinding,
    views_json: Option<&serde_json::Value>,
) {
    let target_view = match &binding.view {
        Some(v) => v.as_str(),
        None => {
            warn!("navigate action missing 'view' field");
            return;
        }
    };

    let from_view = surface_id.split(':').nth(2).unwrap_or("default");

    // Use provided views_json or look up from filesystem
    let owned_views;
    let views = match views_json {
        Some(v) => v,
        None => {
            owned_views = crate::handlers::agents::get_agent_views(state, agent_id).await;
            match &owned_views {
                Some(v) => v,
                None => {
                    warn!(agent_id = %agent_id, "navigate: no views.json found");
                    return;
                }
            }
        }
    };

    if let Err(e) = a2ui
        .navigate_view(
            agent_id,
            from_view,
            target_view,
            binding.params.clone(),
            views,
        )
        .await
    {
        warn!(error = %e, "navigate action failed");
    }
}

/// Handle update_data action: direct data model update.
async fn handle_update_data(a2ui: &Arc<A2UIManager>, surface_id: &str, binding: &ActionBinding) {
    let path = binding.path.as_deref();
    let value = match &binding.value {
        Some(v) => v.clone(),
        None => {
            // If params provided, use those as value
            binding.params.clone().unwrap_or(json!(null))
        }
    };

    if let Err(e) = a2ui.update_data_model(surface_id, path, value).await {
        warn!(error = %e, "update_data action failed");
    }
}
