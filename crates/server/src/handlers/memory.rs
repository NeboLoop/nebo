use axum::extract::{Path, Query, State};
use axum::response::Json;
use serde::Deserialize;

use crate::state::AppState;
use super::{to_error_response, HandlerResult};

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
    #[serde(default)]
    pub namespace: Option<String>,
}

fn default_limit() -> i64 {
    50
}

/// GET /api/v1/memories
pub async fn list_memories(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> HandlerResult<serde_json::Value> {
    let memories = if let Some(ref ns) = q.namespace {
        state.store.list_memories_by_namespace(ns, q.limit, q.offset)
    } else {
        state.store.list_memories(q.limit, q.offset)
    }
    .map_err(to_error_response)?;

    let total = if let Some(ref ns) = q.namespace {
        state.store.count_memories_by_namespace(ns).unwrap_or(0)
    } else {
        state.store.count_memories().unwrap_or(0)
    };

    Ok(Json(serde_json::json!({
        "memories": memories,
        "total": total,
    })))
}

/// GET /api/v1/memories/search
pub async fn search_memories(
    State(state): State<AppState>,
    Query(q): Query<SearchQuery>,
) -> HandlerResult<serde_json::Value> {
    let memories = state
        .store
        .search_memories(&q.q, q.limit, q.offset)
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"memories": memories})))
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    pub q: String,
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

/// GET /api/v1/memories/stats
pub async fn get_stats(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let total = state.store.count_memories().unwrap_or(0);
    let namespaces = state.store.get_distinct_namespaces().unwrap_or_default();

    // Compute layer counts by grouping namespaces by prefix before '/'
    let mut layer_counts = std::collections::HashMap::<String, i64>::new();
    for ns in &namespaces {
        let layer = ns.split('/').next().unwrap_or("other").to_string();
        let count = state.store.count_memories_by_namespace(ns).unwrap_or(0);
        *layer_counts.entry(layer).or_insert(0) += count;
    }

    Ok(Json(serde_json::json!({
        "totalCount": total,
        "layerCounts": layer_counts,
        "namespaces": namespaces,
    })))
}

/// GET /api/v1/memories/:id
pub async fn get_memory(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> HandlerResult<serde_json::Value> {
    let mem = state
        .store
        .get_memory(id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    // Increment access count
    let _ = state.store.increment_memory_access(id);

    Ok(Json(serde_json::json!({"memory": mem})))
}

/// PUT /api/v1/memories/:id
pub async fn update_memory(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    // Tags can arrive as a JSON array or a string; store as JSON array string
    let tags_str = match &body["tags"] {
        serde_json::Value::Array(_) => Some(body["tags"].to_string()),
        serde_json::Value::String(s) => Some(s.clone()),
        _ => None,
    };
    state
        .store
        .update_memory(
            id,
            body["value"].as_str(),
            tags_str.as_deref(),
            body["metadata"].as_str(),
        )
        .map_err(to_error_response)?;

    let mem = state.store.get_memory(id).map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"memory": mem})))
}

/// DELETE /api/v1/memories/:id
pub async fn delete_memory(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> HandlerResult<serde_json::Value> {
    state.store.delete_memory(id).map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"success": true})))
}
