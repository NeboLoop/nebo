//! Store proxy handlers — forward marketplace queries to NeboAI API.

use axum::extract::{Path, Query, State};
use axum::response::Json;
use serde::Deserialize;

use super::{HandlerResult, to_error_response};
use crate::codes::build_api_client;
use crate::state::AppState;
use types::NeboError;

#[derive(Deserialize)]
pub struct StoreQuery {
    pub q: Option<String>,
    pub category: Option<String>,
    pub page: Option<i64>,
    #[serde(rename = "pageSize")]
    pub page_size: Option<i64>,
    #[serde(rename = "type")]
    pub artifact_type: Option<String>,
}

// ── Marketplace Endpoints (unified on /store/products) ──────────────

/// GET /store/products — unified product listing via `/api/v1/products`.
/// Query params: type (skill|workflow|agent), category, q, page, pageSize.
/// Returns `{ "skills": [...] }` enriched with local install state.
/// NeboAI returns `{ "results": [...] }` — we normalize to `{ "skills": [...] }`.
pub async fn list_store_products(
    State(state): State<AppState>,
    Query(params): Query<StoreQuery>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .list_products(
            params.artifact_type.as_deref(),
            params.q.as_deref(),
            params.category.as_deref(),
            params.page,
            params.page_size,
        )
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("list_products: {e}"))))?;

    // NeboAI returns the canonical { "products": [...], "total": N } envelope —
    // pass it through verbatim, only enriching with local install state (which
    // NeboAI can't know). No client-side key remapping.
    let mut out = resp;
    enrich_installed_state(&mut out, &state.store);

    Ok(Json(out))
}

/// GET /store/products/top — top/popular products.
pub async fn list_store_products_top(
    State(state): State<AppState>,
    Query(params): Query<StoreQuery>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .list_top_skills(params.page, params.page_size)
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("list_top_skills: {e}"))))?;
    Ok(Json(resp))
}

/// GET /store/featured — featured products.
pub async fn list_store_featured(
    State(state): State<AppState>,
    Query(params): Query<StoreQuery>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .get_featured(params.artifact_type.as_deref())
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("get_featured: {e}"))))?;
    Ok(Json(resp))
}

/// GET /store/categories — list all marketplace categories with counts.
pub async fn list_store_categories(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .list_categories()
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("list_categories: {e}"))))?;
    Ok(Json(resp))
}

/// GET /store/screenshots/{type} — screenshots for a product type.
pub async fn get_store_screenshots(
    State(state): State<AppState>,
    Path(screenshot_type): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .get_screenshots(&screenshot_type)
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("get_screenshots: {e}"))))?;
    Ok(Json(resp))
}

/// GET /store/products/{id} — single product detail.
pub async fn get_store_product(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .get_skill(&id)
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("get_product: {e}"))))?;
    let mut val = serde_json::to_value(resp).unwrap_or_default();

    // Enrich single product with local install state
    enrich_installed_item(&mut val, &state.store);

    Ok(Json(val))
}

/// GET /store/products/{id}/reviews — product reviews.
pub async fn get_store_product_reviews(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<StoreQuery>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .get_skill_reviews(&id, params.page, params.page_size)
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("get_product_reviews: {e}"))))?;
    Ok(Json(serde_json::to_value(resp).unwrap_or_default()))
}

/// POST /store/products/{id}/reviews — submit a review.
pub async fn submit_store_product_review(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .submit_skill_review(&id, &body)
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("submit_review: {e}"))))?;
    Ok(Json(resp))
}

/// GET /store/products/{id}/similar — similar products.
pub async fn get_store_product_similar(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .get_similar_apps(&id)
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("get_similar: {e}"))))?;
    Ok(Json(resp))
}

/// GET /store/products/{id}/media — product media (screenshots, videos).
pub async fn get_store_product_media(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .get_skill_media(&id)
        .await
        .unwrap_or_else(|_| serde_json::json!({ "media": [] }));
    Ok(Json(resp))
}

/// GET /store/products/{id}/feedback — product feedback.
pub async fn get_store_product_feedback(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<StoreQuery>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .get_skill_feedback(&id, params.page, params.page_size)
        .await
        .unwrap_or_else(|_| serde_json::json!({ "feedback": [] }));
    Ok(Json(resp))
}

/// POST /store/products/{id}/feedback — submit feedback.
pub async fn submit_store_product_feedback(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .submit_skill_feedback(&id, &body)
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("submit_feedback: {e}"))))?;
    Ok(Json(resp))
}

// ── Local install state enrichment ─────────────────────────────────

/// Check if an artifact is installed locally by slug, checking both
/// user and nebo artifact directories.
///
/// All artifact types (skills, agents, plugins) use filesystem-based
/// discovery. The DB stores mutable state (enabled, input_values) but
/// is NOT the source of truth for installation.
fn is_locally_installed(slug: &str, artifact_type: &str) -> bool {
    let (user_dir, nebo_dir) = match (config::user_dir(), config::nebo_dir()) {
        (Ok(u), Ok(n)) => (u, n),
        _ => return false,
    };

    // Check user dir
    let user_path = user_dir.join(artifact_type).join(slug);
    if user_path.exists() {
        return true;
    }

    // Check nebo (marketplace) dir
    let nebo_path = nebo_dir.join(artifact_type).join(slug);
    nebo_path.exists()
}

/// Check if a product is installed on the filesystem.
fn is_installed(slug: &str, _name: &str, artifact_type: &str, _store: &db::Store) -> bool {
    let dir_type = match artifact_type {
        "agent" => "agents",
        "skill" => "skills",
        "plugin" => "plugins",
        _ => "skills",
    };
    is_locally_installed(slug, dir_type)
}

/// Enrich a single product JSON value with local install state and update availability.
fn enrich_installed_item(val: &mut serde_json::Value, store: &db::Store) {
    if let Some(obj) = val.as_object_mut() {
        let slug = obj.get("slug").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let name = obj.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let artifact_type = obj.get("type").and_then(|v| v.as_str()).unwrap_or("skill").to_string();
        let artifact_id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if !slug.is_empty() && is_installed(&slug, &name, &artifact_type, store) {
            obj.insert("installed".to_string(), serde_json::Value::Bool(true));

            // Check if an update is available for this artifact
            let lookup_id = if artifact_id.is_empty() { &slug } else { &artifact_id };
            if let Ok(pending) = store.list_artifacts_with_updates() {
                if let Some(pref) = pending.iter().find(|p| p.artifact_id == *lookup_id || p.artifact_id == slug) {
                    obj.insert("updateAvailable".to_string(), serde_json::Value::Bool(true));
                    obj.insert(
                        "remoteVersion".to_string(),
                        serde_json::Value::String(pref.remote_version.clone()),
                    );
                }
            }
        }
    }
}

/// Enrich a product list response with local install state.
/// Looks for `{ "skills": [...] }` structure.
fn enrich_installed_state(resp: &mut serde_json::Value, store: &db::Store) {
    if let Some(items) = resp.get_mut("products").and_then(|v| v.as_array_mut()) {
        for item in items.iter_mut() {
            enrich_installed_item(item, store);
        }
    }
}

/// POST /store/products/{id}/install — install a product by ID.
///
/// Fetches the product detail from NeboAI to get its install code, then
/// routes through the standard code-based install flow (persist to DB/disk,
/// activate roles, cascade dependencies, reload skill loader).
pub async fn install_store_product(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;

    // Fetch product detail to get its install code
    let detail = api.get_skill(&id).await.map_err(|e| {
        to_error_response(NeboError::Internal(format!("fetch product detail: {e}")))
    })?;

    let code = detail
        .item
        .code
        .as_deref()
        .filter(|c| !c.is_empty())
        .ok_or_else(|| {
            to_error_response(NeboError::Internal("product has no install code".into()))
        })?;

    // Route through the standard code handler which handles the full lifecycle:
    // redeem → persist to DB/disk → activate → register agent → cascade deps
    let (code_type, validated_code) = crate::codes::detect_code(code).ok_or_else(|| {
        to_error_response(NeboError::Internal(format!("invalid code format: {code}")))
    })?;

    // Use a synthetic session ID for the install
    let session_id = format!("store-install-{}", id);
    crate::codes::handle_code(&state, code_type, validated_code, &session_id).await;

    // The marketplace product id equals the installed artifact/agent id, so the
    // frontend can address the agent directly instead of matching by name.
    Ok(Json(serde_json::json!({ "success": true, "agentId": id })))
}

/// DELETE /store/products/{id}/install — uninstall a product.
///
/// Removes the product from NeboAI, deletes the local DB record,
/// cleans up filesystem artifacts, and deactivates the role if active.
pub async fn uninstall_store_product(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;

    // Get product detail before removing so we know the slug and type
    let detail = api.get_skill(&id).await.ok();

    // Unregister from NeboAI
    let _ = api.uninstall_product(&id).await;

    // Look up local agent by NeboAI product ID first, then by name as fallback
    let local_agent = state.store.get_agent(&id).ok().flatten().or_else(|| {
        let name = detail.as_ref().map(|d| d.item.name.as_str()).unwrap_or("");
        if !name.is_empty() {
            state.store.get_agent_by_name(name).ok().flatten()
        } else {
            None
        }
    });
    let slug = detail
        .as_ref()
        .map(|d| d.item.slug.clone())
        .unwrap_or_default();
    let artifact_type = detail
        .as_ref()
        .and_then(|d| d.item.artifact_type.as_deref())
        .unwrap_or("");

    // Derive slug from role name if NeboAI didn't provide it
    let slug = if slug.is_empty() {
        local_agent
            .as_ref()
            .map(|r| r.name.to_lowercase().replace(' ', "-"))
            .unwrap_or_default()
    } else {
        slug
    };

    // Determine artifact type from local DB kind or NeboAI
    let is_agent = artifact_type == "agent"
        || local_agent.is_some()
        || local_agent
            .as_ref()
            .and_then(|r| r.kind.as_deref())
            .map(|k| k.starts_with("AGNT-"))
            .unwrap_or(false);

    // Clean up local DB — use the local agent's ID (may differ from NeboAI product ID)
    if let Some(ref agent_rec) = local_agent {
        let agent_id = &agent_rec.id;
        // Stop agent worker
        state.agent_workers.stop_agent(agent_id).await;
        // Deactivate from live registry
        state.agent_registry.write().await.remove(agent_id);
        // Remove workflow bindings and triggers
        workflow::triggers::unregister_agent_triggers(agent_id, &state.store);
        state.event_dispatcher.unsubscribe_agent(agent_id).await;
        let _ = state.store.delete_agent_workflows(agent_id);
        let _ = state.store.delete_agent(agent_id);
        // Clean up filesystem: napp_path, nebo/agents/{slug}, user/agents/{slug}
        if let Some(ref napp_path) = agent_rec.napp_path {
            let path = std::path::Path::new(napp_path);
            if path.exists() {
                let _ = std::fs::remove_dir_all(path);
            }
        }
        if let Ok(user_dir) = config::user_dir() {
            let dir = user_dir.join("agents").join(&slug);
            if dir.exists() {
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
        // Deregister agent from NeboAI (non-blocking)
        {
            let st = state.clone();
            let agent_name = agent_rec.name.clone();
            tokio::spawn(async move {
                if let Err(e) = crate::codes::deregister_agent_from_loop(&st, &agent_name).await {
                    tracing::warn!(agent = %agent_name, error = %e, "failed to deregister agent from loop on uninstall");
                }
            });
        }
    }

    // Clean up filesystem
    if !slug.is_empty() {
        if let Ok(nebo_dir) = config::nebo_dir() {
            let subdir = if is_agent { "agents" } else { "skills" };
            let artifact_dir = nebo_dir.join(subdir).join(&slug);
            if artifact_dir.exists() {
                let _ = std::fs::remove_dir_all(&artifact_dir);
            }
        }
    }

    // Reload the loader for the removed artifact BEFORE notifying the frontend,
    // then broadcast. list_agents()/list_skills() enumerate the loaders
    // (filesystem source of truth); reloading here keeps the broadcast's refetch
    // consistent instead of racing the loader's own filesystem-watch reload —
    // the "requires a hard refresh" bug. Mirrors the install path (codes.rs).
    if is_agent {
        state.agent_loader.load_all().await;
        if let Some(ref agent_rec) = local_agent {
            state.hub.broadcast(
                "agent_uninstalled",
                serde_json::json!({ "agentId": agent_rec.id, "name": agent_rec.name }),
            );
        }
    } else {
        state.skill_loader.load_all().await;
    }

    Ok(Json(serde_json::json!({ "success": true })))
}

// ── Collections ───────────────────────────────────────────────────
//
// Collection CRUD for org-scoped marketplace bundles.
// Currently stubs — will proxy to NeboAI API when collection
// endpoints are available server-side.

/// GET /store/collections — list all collections (proxied from NeboAI).
pub async fn list_store_collections(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .list_collections()
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("list_collections: {e}"))))?;
    Ok(Json(resp))
}

/// GET /store/collections/{id} — get a single collection (proxied from NeboAI).
pub async fn get_store_collection(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .get_collection(&id)
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("get_collection: {e}"))))?;
    Ok(Json(resp))
}

/// POST /store/collections — create a new collection.
pub async fn create_store_collection(
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    Ok(Json(serde_json::json!({ "collection": body })))
}

/// PUT /store/collections/{id} — update a collection.
pub async fn update_store_collection(
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    Ok(Json(serde_json::json!({ "collection": body, "id": id })))
}

/// DELETE /store/collections/{id} — delete a collection.
pub async fn delete_store_collection(Path(id): Path<String>) -> HandlerResult<serde_json::Value> {
    Ok(Json(serde_json::json!({ "deleted": true, "id": id })))
}

/// POST /store/collections/{id}/items — add an item to a collection.
pub async fn add_collection_item(
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    Ok(Json(
        serde_json::json!({ "collection": { "id": id }, "added": body }),
    ))
}

/// DELETE /store/collections/{id}/items/{item_id} — remove an item from a collection.
pub async fn remove_collection_item(
    Path((id, item_id)): Path<(String, String)>,
) -> HandlerResult<serde_json::Value> {
    Ok(Json(
        serde_json::json!({ "collection": { "id": id }, "removedItem": item_id }),
    ))
}

// ── Organizations ─────────────────────────────────────────────────

/// GET /store/orgs — list organizations the user has access to.
pub async fn list_store_orgs(State(_state): State<AppState>) -> HandlerResult<serde_json::Value> {
    Ok(Json(serde_json::json!({ "orgs": [] })))
}
