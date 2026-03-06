use axum::extract::{Path, State};
use axum::response::Json;

use crate::state::AppState;
use super::{to_error_response, HandlerResult};

/// GET /api/v1/tools
pub async fn list_tools(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let tools = state.napp_registry.list_tools().await;
    let total = tools.len();
    Ok(Json(serde_json::json!({
        "tools": tools,
        "total": total,
    })))
}

/// GET /api/v1/tools/:id
pub async fn get_tool(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let manifest = state
        .napp_registry
        .get_manifest(&id)
        .await
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;
    Ok(Json(serde_json::json!({
        "tool": manifest,
    })))
}

/// POST /api/v1/tools/sideload
pub async fn sideload_tool(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let path = body["path"]
        .as_str()
        .ok_or_else(|| to_error_response(types::NeboError::Validation("path required".into())))?;
    let project_dir = std::path::Path::new(path);

    let tool_id = state
        .napp_registry
        .sideload(project_dir)
        .await
        .map_err(|e| to_error_response(types::NeboError::Server(e.to_string())))?;

    let manifest = state.napp_registry.get_manifest(&tool_id).await;

    state.hub.broadcast(
        "tool_installed",
        serde_json::json!({
            "toolId": tool_id,
            "sideloaded": true,
        }),
    );

    // Sync napp adapter for agent dispatch
    let _ = state.napp_manager.create_adapter(&tool_id).await;

    Ok(Json(serde_json::json!({
        "tool": manifest,
    })))
}

/// DELETE /api/v1/tools/:id/sideload
pub async fn unsideload_tool(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    state
        .napp_registry
        .unsideload(&id)
        .await
        .map_err(|e| to_error_response(types::NeboError::Server(e.to_string())))?;

    state.hub.broadcast(
        "tool_uninstalled",
        serde_json::json!({ "toolId": id }),
    );

    // Remove napp adapter
    state.napp_manager.remove_adapter(&id).await;

    Ok(Json(serde_json::json!({
        "message": "Tool unsideloaded",
    })))
}

/// POST /api/v1/tools/install
pub async fn install_tool(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let url = body["url"]
        .as_str()
        .ok_or_else(|| to_error_response(types::NeboError::Validation("url required".into())))?;

    let tool_id = state
        .napp_registry
        .install_from_url(url)
        .await
        .map_err(|e| to_error_response(types::NeboError::Server(e.to_string())))?;

    let manifest = state.napp_registry.get_manifest(&tool_id).await;

    state.hub.broadcast(
        "tool_installed",
        serde_json::json!({
            "toolId": tool_id,
            "sideloaded": false,
        }),
    );

    // Sync napp adapter for agent dispatch
    let _ = state.napp_manager.create_adapter(&tool_id).await;

    Ok(Json(serde_json::json!({
        "tool": manifest,
    })))
}

/// DELETE /api/v1/tools/:id
pub async fn uninstall_tool(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    state
        .napp_registry
        .uninstall(&id)
        .await
        .map_err(|e| to_error_response(types::NeboError::Server(e.to_string())))?;

    // Remove napp adapter
    state.napp_manager.remove_adapter(&id).await;

    state.hub.broadcast(
        "tool_uninstalled",
        serde_json::json!({ "toolId": id }),
    );

    Ok(Json(serde_json::json!({
        "message": "Tool uninstalled",
    })))
}
