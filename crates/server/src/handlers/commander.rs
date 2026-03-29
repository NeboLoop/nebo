use axum::extract::{Path, State};
use axum::response::Json;
use serde::Deserialize;

use crate::state::AppState;
use super::{to_error_response, HandlerResult};

/// GET /commander/graph — full graph: nodes, computed + user edges, teams, positions.
pub async fn get_graph(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    // 1. Active roles from the in-memory registry
    let registry = state.agent_registry.read().await;
    let mut nodes: Vec<serde_json::Value> = Vec::new();

    // Main bot node
    nodes.push(serde_json::json!({
        "id": "main-bot",
        "type": "main",
        "name": "Nebo",
    }));

    // Agent nodes
    let agent_ids: Vec<String> = registry.keys().cloned().collect();
    for role in registry.values() {
        let description = state
            .store
            .get_agent(&role.agent_id)
            .ok()
            .flatten()
            .map(|r| r.description)
            .unwrap_or_default();
        let workflow_count = role
            .config
            .as_ref()
            .map(|c| c.workflows.len())
            .unwrap_or(0);

        nodes.push(serde_json::json!({
            "id": role.agent_id,
            "type": "agent",
            "name": role.name,
            "description": description,
            "status": "active",
            "workflowCount": workflow_count,
        }));
    }
    drop(registry);

    // 2. Default hierarchy edges: main bot → each agent (unless user-drawn edges override)
    let user_edges_db = state
        .store
        .list_commander_edges()
        .map_err(to_error_response)?;

    let mut edges: Vec<serde_json::Value> = Vec::new();

    // Compute event edges from emit chains (auto-detected, not user-drawn)
    edges.extend(compute_event_edges(&state, &agent_ids));

    // 3. User-drawn edges (reporting/coordination)
    for ue in &user_edges_db {
        edges.push(serde_json::json!({
            "id": ue.id,
            "source": ue.source_node_id,
            "target": ue.target_node_id,
            "type": ue.edge_type,
            "label": ue.label,
        }));
    }

    // 4. Teams
    let teams_db = state
        .store
        .list_commander_teams()
        .map_err(to_error_response)?;
    let members_db = state
        .store
        .list_commander_team_members()
        .map_err(to_error_response)?;

    let teams: Vec<serde_json::Value> = teams_db
        .iter()
        .map(|t| {
            let member_ids: Vec<&str> = members_db
                .iter()
                .filter(|m| m.team_id == t.id)
                .map(|m| m.agent_id.as_str())
                .collect();
            serde_json::json!({
                "id": t.id,
                "name": t.name,
                "color": t.color,
                "memberIds": member_ids,
                "position": { "x": t.position_x, "y": t.position_y },
            })
        })
        .collect();

    // 5. Saved node positions
    let positions_db = state
        .store
        .list_commander_node_positions()
        .map_err(to_error_response)?;

    // Merge positions into nodes
    for node in &mut nodes {
        if let Some(id) = node.get("id").and_then(|v| v.as_str()) {
            if let Some(pos) = positions_db.iter().find(|p| p.node_id == id) {
                node.as_object_mut().unwrap().insert(
                    "position".to_string(),
                    serde_json::json!({ "x": pos.position_x, "y": pos.position_y }),
                );
            }
        }
    }

    Ok(Json(serde_json::json!({
        "nodes": nodes,
        "edges": edges,
        "teams": teams,
    })))
}

/// Compute event edges by matching emit sources to event trigger subscriptions.
fn compute_event_edges(
    state: &AppState,
    active_agent_ids: &[String],
) -> Vec<serde_json::Value> {
    let mut edges = Vec::new();

    let emit_sources = match state.store.list_emit_sources() {
        Ok(sources) => sources,
        Err(_) => return edges,
    };

    let event_triggers = match state.store.list_active_event_triggers() {
        Ok(triggers) => triggers,
        Err(_) => return edges,
    };

    // Build slug -> agent_id map
    let agent_name_to_id: std::collections::HashMap<String, String> = {
        let mut map = std::collections::HashMap::new();
        for agent_id in active_agent_ids {
            if let Ok(Some(role)) = state.store.get_agent(agent_id) {
                let slug = role.name.to_lowercase().replace(' ', "-");
                map.insert(slug, agent_id.clone());
            }
        }
        map
    };

    for es in &emit_sources {
        let emitter_slug = es.agent_name.to_lowercase().replace(' ', "-");
        let emitter_agent_id = match agent_name_to_id.get(&emitter_slug) {
            Some(id) => id,
            None => continue,
        };

        let full_event = format!("{}.{}", emitter_slug, es.emit);

        for trigger in &event_triggers {
            if !active_agent_ids.contains(&trigger.agent_id) {
                continue;
            }

            let patterns: Vec<&str> = trigger.trigger_config.split(',').collect();
            for pattern in patterns {
                let pattern = pattern.trim();
                if matches_event_pattern(pattern, &full_event) {
                    edges.push(serde_json::json!({
                        "id": format!("emit-{}-{}-{}", emitter_agent_id, trigger.agent_id, es.emit),
                        "source": emitter_agent_id,
                        "target": trigger.agent_id,
                        "type": "event",
                        "label": es.emit,
                    }));
                }
            }
        }
    }

    edges
}

fn matches_event_pattern(pattern: &str, event: &str) -> bool {
    if pattern == event {
        return true;
    }
    if pattern.ends_with(".*") {
        let prefix = &pattern[..pattern.len() - 2];
        return event.starts_with(prefix) && event.len() > prefix.len();
    }
    if pattern.ends_with('*') {
        let prefix = &pattern[..pattern.len() - 1];
        return event.starts_with(prefix);
    }
    false
}

/// PUT /commander/layout — batch save node positions.
pub async fn save_layout(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let positions = body
        .get("positions")
        .and_then(|v| v.as_array())
        .ok_or_else(|| to_error_response(types::NeboError::Validation("missing positions array".into())))?;

    let parsed: Vec<(String, f64, f64)> = positions
        .iter()
        .filter_map(|p| {
            let id = p.get("nodeId")?.as_str()?.to_string();
            let x = p.get("x")?.as_f64()?;
            let y = p.get("y")?.as_f64()?;
            Some((id, x, y))
        })
        .collect();

    state
        .store
        .save_commander_node_positions(&parsed)
        .map_err(to_error_response)?;

    Ok(Json(serde_json::json!({ "saved": parsed.len() })))
}

// ── Teams ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTeamRequest {
    pub name: String,
    #[serde(default = "default_team_color")]
    pub color: String,
    #[serde(default)]
    pub member_ids: Vec<String>,
}

fn default_team_color() -> String {
    "#6366f1".to_string()
}

/// POST /commander/teams
pub async fn create_team(
    State(state): State<AppState>,
    Json(body): Json<CreateTeamRequest>,
) -> HandlerResult<serde_json::Value> {
    let id = uuid::Uuid::new_v4().to_string();
    let team = state
        .store
        .create_commander_team(&id, &body.name, &body.color)
        .map_err(to_error_response)?;

    if !body.member_ids.is_empty() {
        state
            .store
            .set_commander_team_members(&id, &body.member_ids)
            .map_err(to_error_response)?;
    }

    Ok(Json(serde_json::json!({
        "team": team,
        "memberIds": body.member_ids,
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTeamRequest {
    pub name: Option<String>,
    pub color: Option<String>,
    pub member_ids: Option<Vec<String>>,
}

/// PUT /commander/teams/{id}
pub async fn update_team(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateTeamRequest>,
) -> HandlerResult<serde_json::Value> {
    let teams = state
        .store
        .list_commander_teams()
        .map_err(to_error_response)?;
    let existing = teams
        .iter()
        .find(|t| t.id == id)
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let name = body.name.as_deref().unwrap_or(&existing.name);
    let color = body.color.as_deref().unwrap_or(&existing.color);

    state
        .store
        .update_commander_team(&id, name, color)
        .map_err(to_error_response)?;

    if let Some(member_ids) = &body.member_ids {
        state
            .store
            .set_commander_team_members(&id, member_ids)
            .map_err(to_error_response)?;
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// DELETE /commander/teams/{id}
pub async fn delete_team(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    state
        .store
        .delete_commander_team(&id)
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Edges (user-drawn reporting/coordination) ─────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateEdgeRequest {
    pub source: String,
    pub target: String,
    #[serde(default = "default_edge_type")]
    pub edge_type: String,
    #[serde(default)]
    pub label: String,
}

fn default_edge_type() -> String {
    "reports_to".to_string()
}

/// POST /commander/edges — create a user-drawn reporting/coordination edge.
pub async fn create_edge(
    State(state): State<AppState>,
    Json(body): Json<CreateEdgeRequest>,
) -> HandlerResult<serde_json::Value> {
    let id = uuid::Uuid::new_v4().to_string();
    let edge = state
        .store
        .create_commander_edge(&id, &body.source, &body.target, &body.edge_type, &body.label)
        .map_err(to_error_response)?;

    Ok(Json(serde_json::json!({ "edge": edge })))
}

/// DELETE /commander/edges/{id}
pub async fn delete_edge(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    state
        .store
        .delete_commander_edge(&id)
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}
