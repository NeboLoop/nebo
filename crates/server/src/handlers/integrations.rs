use axum::extract::{Path, State};
use axum::response::Json;
use tracing::warn;

use crate::state::AppState;
use super::{to_error_response, HandlerResult};

/// Fix legacy integrations that were saved as "stdio" but have HTTP URLs.
fn fix_server_type(state: &AppState) {
    if let Ok(integrations) = state.store.list_mcp_integrations() {
        for i in &integrations {
            if i.server_type == "stdio" {
                if let Some(ref url) = i.server_url {
                    if url.starts_with("http://") || url.starts_with("https://") {
                        let _ = state.store.update_mcp_integration(
                            &i.id, None, None, None, None,
                            None,
                        );
                        // Direct SQL update for server_type since update_mcp_integration doesn't expose it
                        let _ = state.store.set_mcp_server_type(&i.id, "http");
                    }
                }
            }
        }
    }
}

/// Re-sync the MCP bridge after integration changes.
async fn sync_bridge(state: &AppState) {
    if let Ok(integrations) = state.store.list_mcp_integrations() {
        let infos: Vec<mcp::bridge::IntegrationInfo> = integrations
            .iter()
            .map(|i| mcp::bridge::IntegrationInfo {
                id: i.id.clone(),
                name: i.name.clone(),
                server_type: i.server_type.clone(),
                server_url: i.server_url.clone(),
                auth_type: i.auth_type.clone(),
                is_enabled: i.is_enabled.unwrap_or(0) != 0,
                connection_status: i.connection_status.clone(),
            })
            .collect();
        if let Err(e) = state.bridge.sync_all(&infos).await {
            warn!("MCP bridge sync failed: {}", e);
        }
    }
}

/// GET /api/v1/integrations
pub async fn list_integrations(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    // Auto-fix legacy "stdio" records that have HTTP URLs
    fix_server_type(&state);
    let integrations = state.store.list_mcp_integrations().map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"integrations": integrations})))
}

/// POST /api/v1/integrations
pub async fn create_integration(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let name = body["name"]
        .as_str()
        .ok_or_else(|| to_error_response(types::NeboError::Validation("name required".into())))?;
    let server_url = body["serverUrl"].as_str();
    // Infer server_type from URL if not explicitly provided
    let server_type = body["serverType"].as_str().unwrap_or_else(|| {
        if let Some(url) = server_url {
            if url.starts_with("http://") || url.starts_with("https://") {
                "http"
            } else {
                "stdio"
            }
        } else {
            "stdio"
        }
    });
    let auth_type = body["authType"].as_str().unwrap_or("none");
    let metadata = body.get("metadata").map(|v| v.to_string());

    let id = uuid::Uuid::new_v4().to_string();
    let integration = state
        .store
        .create_mcp_integration(&id, name, server_type, server_url, auth_type, metadata.as_deref())
        .map_err(to_error_response)?;

    Ok(Json(serde_json::json!({"integration": integration})))
}

/// GET /api/v1/integrations/:id
pub async fn get_integration(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let integration = state
        .store
        .get_mcp_integration(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;
    Ok(Json(serde_json::json!(integration)))
}

/// PUT /api/v1/integrations/:id
pub async fn update_integration(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    state
        .store
        .update_mcp_integration(
            &id,
            body["name"].as_str(),
            body["serverUrl"].as_str(),
            body["authType"].as_str(),
            body["isEnabled"].as_i64().map(|v| v != 0),
            body.get("metadata").map(|v| v.to_string()).as_deref(),
        )
        .map_err(to_error_response)?;

    // Sync bridge to reflect changes
    sync_bridge(&state).await;

    let updated = state.store.get_mcp_integration(&id).map_err(to_error_response)?;
    Ok(Json(serde_json::json!(updated)))
}

/// DELETE /api/v1/integrations/:id
pub async fn delete_integration(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    // Disconnect before deleting
    state.bridge.disconnect(&id).await;
    state.store.delete_mcp_integration(&id).map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"success": true})))
}

/// POST /api/v1/integrations/:id/test
pub async fn test_integration(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let integration = state
        .store
        .get_mcp_integration(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    // If a URL is provided, always try to reach it regardless of server_type
    let (success, message) = if let Some(ref url) = integration.server_url {
        if url.starts_with("http://") || url.starts_with("https://") {
            match reqwest::Client::new()
                .get(url.as_str())
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await
            {
                Ok(resp) => (true, format!("Server reachable (HTTP {})", resp.status())),
                Err(e) => (false, format!("Cannot reach server: {}", e)),
            }
        } else {
            (true, "Configuration looks valid".to_string())
        }
    } else {
        match integration.server_type.as_str() {
            "stdio" => (true, "Configuration looks valid (stdio server will be started on demand)".to_string()),
            _ => (false, "No server URL configured".to_string()),
        }
    };

    Ok(Json(serde_json::json!({
        "success": success,
        "integration": integration.name,
        "message": message,
    })))
}

/// POST /api/v1/integrations/:id/connect — connect to the MCP server and register its tools.
pub async fn connect_integration(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let integration = state
        .store
        .get_mcp_integration(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let server_url = integration.server_url.as_deref().unwrap_or("");
    if server_url.is_empty() {
        return Err(to_error_response(types::NeboError::Validation(
            "No server URL configured".into(),
        )));
    }

    // Try to connect and list tools
    match state
        .bridge
        .connect(&id, &integration.server_type, server_url, None)
        .await
    {
        Ok(tools) => {
            let tool_count = tools.len();
            // Update connection status in DB
            let _ = state.store.update_mcp_integration(
                &id,
                None,
                None,
                None,
                None,
                Some(&serde_json::json!({"connection_status": "connected", "tool_count": tool_count}).to_string()),
            );
            // Also update connection_status column directly
            let _ = state.store.set_mcp_connection_status(&id, "connected", tool_count as i64);

            let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
            Ok(Json(serde_json::json!({
                "success": true,
                "message": format!("Connected — {} tools registered", tool_count),
                "toolCount": tool_count,
                "tools": tool_names,
            })))
        }
        Err(e) => {
            let _ = state.store.set_mcp_connection_status(&id, "error", 0);
            Ok(Json(serde_json::json!({
                "success": false,
                "message": format!("Connection failed: {}", e),
            })))
        }
    }
}

/// GET /api/v1/integrations/registry
pub async fn list_registry() -> HandlerResult<serde_json::Value> {
    // Built-in list of known MCP servers that users can install
    Ok(Json(serde_json::json!({
        "registry": [
            {
                "name": "filesystem",
                "description": "Read, write, and manage files on the local filesystem",
                "serverType": "stdio",
                "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-filesystem"]
            },
            {
                "name": "brave-search",
                "description": "Web search via Brave Search API",
                "serverType": "stdio",
                "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-brave-search"]
            },
            {
                "name": "github",
                "description": "GitHub repository management",
                "serverType": "stdio",
                "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-github"]
            },
            {
                "name": "sqlite",
                "description": "SQLite database operations",
                "serverType": "stdio",
                "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-sqlite"]
            },
            {
                "name": "memory",
                "description": "Knowledge graph-based persistent memory",
                "serverType": "stdio",
                "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-memory"]
            }
        ]
    })))
}

/// GET /api/v1/integrations/tools
pub async fn list_tools(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    // Return all registered tools (built-in + MCP)
    let tool_defs = state.tools.list().await;
    let tools: Vec<serde_json::Value> = tool_defs
        .iter()
        .map(|t| {
            serde_json::json!({
                "name": t.name,
                "description": t.description,
                "source": "builtin",
                "inputSchema": t.input_schema,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({"tools": tools})))
}
