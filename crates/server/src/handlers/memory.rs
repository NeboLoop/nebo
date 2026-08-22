use axum::extract::{Path, Query, State};
use axum::response::Json;
use serde::Deserialize;

use super::{HandlerResult, to_error_response};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
    #[serde(default)]
    pub namespace: Option<String>,
    /// Scope the listing to a single agent's memory. The main bot ("assistant"/
    /// empty) uses the raw owner scope; every other agent uses
    /// "{owner}:agent:{agent_id}" — matching how the runner scopes writes/reads.
    #[serde(default)]
    pub agent_id: Option<String>,
}

fn default_limit() -> i64 {
    50
}

/// GET /api/v1/memories
pub async fn list_memories(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> HandlerResult<serde_json::Value> {
    // agent_id scoping takes precedence — the per-agent Memory view shows only
    // that agent's memories (matched by the `:agent:<id>` scope suffix), so the
    // user can trust memory is not global. "assistant"/"main" is the main bot's
    // UI route id → the empty runtime agent_id.
    let (memories, total) = if let Some(ref aid) = q.agent_id {
        let aid = if aid == "assistant" || aid == "main" {
            ""
        } else {
            aid.as_str()
        };
        let mems = state
            .store
            .list_memories_for_agent(aid, q.limit, q.offset)
            .map_err(to_error_response)?;
        let total = mems.len() as i64;
        (mems, total)
    } else if let Some(ref ns) = q.namespace {
        (
            state
                .store
                .list_memories_by_namespace(ns, q.limit, q.offset)
                .map_err(to_error_response)?,
            state.store.count_memories_by_namespace(ns).unwrap_or(0),
        )
    } else {
        (
            state
                .store
                .list_memories(q.limit, q.offset)
                .map_err(to_error_response)?,
            state.store.count_memories().unwrap_or(0),
        )
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
    // agent_id scoping mirrors list_memories: the per-agent Memory view's
    // search must never surface another agent's scopes. Without it this was
    // the one unscoped read in the memory surface (isolation audit
    // 2026-08-22). No agent_id = the global owner memory manager.
    let memories = if let Some(ref aid) = q.agent_id {
        let aid = if aid == "assistant" || aid == "main" {
            ""
        } else {
            aid.as_str()
        };
        state
            .store
            .search_memories_for_agent(aid, &q.q, q.limit, q.offset)
            .map_err(to_error_response)?
    } else {
        state
            .store
            .search_memories(&q.q, q.limit, q.offset)
            .map_err(to_error_response)?
    };
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
    #[serde(default, rename = "agentId")]
    pub agent_id: Option<String>,
}

/// GET /api/v1/memories/stats
pub async fn get_stats(State(state): State<AppState>) -> HandlerResult<serde_json::Value> {
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

    // Re-embed on edit: without this, the OLD text keeps serving through
    // vector recall until restart — an edited-out (possibly redacted) value
    // stayed searchable (isolation audit 2026-08-22). embed_memories deletes
    // the stale chunks first and invalidates the FTS index; with no embedding
    // provider, drop the stale chunks and invalidate directly.
    if body["value"].as_str().is_some() {
        if let Some(ref m) = mem {
            match state.embedding_provider {
                Some(ref ep) => agent::memory::embed_memories_async(
                    state.store.clone(),
                    ep.clone(),
                    vec![(m.namespace.clone(), m.key.clone())],
                    m.user_id.clone(),
                ),
                None => {
                    let _ = state.store.delete_chunks_for_memory(id);
                    agent::search_adapter::invalidate_index(&m.user_id);
                }
            }
        }
    }
    Ok(Json(serde_json::json!({"memory": mem})))
}

/// DELETE /api/v1/memories/:id
pub async fn delete_memory(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> HandlerResult<serde_json::Value> {
    // Capture the row before deleting so its chunks/embeddings and search
    // index entry go with it — a deleted memory must stop serving through
    // vector recall immediately, not at next restart.
    let mem = state.store.get_memory(id).map_err(to_error_response)?;
    state.store.delete_memory(id).map_err(to_error_response)?;
    let _ = state.store.delete_chunks_for_memory(id);
    if let Some(m) = mem {
        agent::search_adapter::invalidate_index(&m.user_id);
    }
    Ok(Json(serde_json::json!({"success": true})))
}
