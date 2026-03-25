//! Store proxy handlers — forward marketplace queries to NeboLoop API.

use axum::extract::{Path, Query, State};
use axum::response::Json;
use serde::Deserialize;

use super::{to_error_response, HandlerResult};
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
/// Query params: type (skill|workflow|role), category, q, page, pageSize.
/// Returns `{ "skills": [...] }` enriched with local install state.
/// NeboLoop returns `{ "results": [...] }` — we normalize to `{ "skills": [...] }`.
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

    let mut out = normalize_to_skills(resp);

    // Enrich with local install state — NeboLoop doesn't know what's on this machine
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
    Ok(Json(normalize_to_skills(resp)))
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
    Ok(Json(normalize_to_apps(resp)))
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

// ── NeboLoop response normalization ────────────────────────────────
//
// NeboLoop returns `{ "results": [...], "total": N }` but our frontend
// expects `{ "skills": [...] }` for product lists and `{ "apps": [...] }`
// for featured. If the response already has the expected key, pass through.

fn normalize_to_skills(mut val: serde_json::Value) -> serde_json::Value {
    if val.get("skills").is_some() {
        return val;
    }
    if let Some(results) = val.as_object_mut().and_then(|o| o.remove("results")) {
        val.as_object_mut().unwrap().insert("skills".to_string(), results);
    }
    val
}

fn normalize_to_apps(mut val: serde_json::Value) -> serde_json::Value {
    if val.get("apps").is_some() {
        return val;
    }
    if let Some(results) = val.as_object_mut().and_then(|o| o.remove("results")) {
        val.as_object_mut().unwrap().insert("apps".to_string(), results);
    }
    val
}

// ── Local install state enrichment ─────────────────────────────────

/// Check if a skill is installed locally by slug, checking both
/// user and nebo artifact directories.
fn is_skill_locally_installed(slug: &str) -> bool {
    let (user_dir, nebo_dir) = match (config::user_dir(), config::nebo_dir()) {
        (Ok(u), Ok(n)) => (u, n),
        _ => return false,
    };

    // Check user dir: slug/SKILL.md
    let user_path = user_dir.join("skills").join(slug);
    if user_path.exists() {
        return true;
    }

    // Check nebo (marketplace) dir: slug/
    let nebo_path = nebo_dir.join("skills").join(slug);
    nebo_path.exists()
}

/// Check if a product is installed: roles use the DB, skills use the filesystem.
fn is_installed(slug: &str, name: &str, artifact_type: &str, store: &db::Store) -> bool {
    match artifact_type {
        "role" => store.role_installed_by_name(name).unwrap_or(false),
        _ => is_skill_locally_installed(slug),
    }
}

/// Enrich a single product JSON value with local install state.
fn enrich_installed_item(val: &mut serde_json::Value, store: &db::Store) {
    if let Some(obj) = val.as_object_mut() {
        let slug = obj.get("slug").and_then(|v| v.as_str()).unwrap_or("");
        let name = obj.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let artifact_type = obj.get("type").and_then(|v| v.as_str()).unwrap_or("skill");
        if !slug.is_empty() && is_installed(slug, name, artifact_type, store) {
            obj.insert("installed".to_string(), serde_json::Value::Bool(true));
        }
    }
}

/// Enrich a product list response with local install state.
/// Looks for `{ "skills": [...] }` structure.
fn enrich_installed_state(resp: &mut serde_json::Value, store: &db::Store) {
    if let Some(items) = resp.get_mut("skills").and_then(|v| v.as_array_mut()) {
        for item in items.iter_mut() {
            enrich_installed_item(item, store);
        }
    }
}

/// POST /store/products/{id}/install — install a product by ID.
///
/// Fetches the product detail from NeboLoop to get its install code, then
/// routes through the standard code-based install flow (persist to DB/disk,
/// activate roles, cascade dependencies, reload skill loader).
pub async fn install_store_product(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;

    // Fetch product detail to get its install code
    let detail = api
        .get_skill(&id)
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("fetch product detail: {e}"))))?;

    let code = detail
        .code
        .as_deref()
        .filter(|c| !c.is_empty())
        .ok_or_else(|| to_error_response(NeboError::Internal("product has no install code".into())))?;

    // Route through the standard code handler which handles the full lifecycle:
    // redeem → persist to DB/disk → activate → register agent → cascade deps
    let (code_type, validated_code) = crate::codes::detect_code(code)
        .ok_or_else(|| to_error_response(NeboError::Internal(format!("invalid code format: {code}"))))?;

    // Use a synthetic session ID for the install
    let session_id = format!("store-install-{}", id);
    crate::codes::handle_code(&state, code_type, validated_code, &session_id).await;

    Ok(Json(serde_json::json!({ "success": true })))
}

/// DELETE /store/products/{id}/install — uninstall a product.
///
/// Removes the product from NeboLoop, deletes the local DB record,
/// cleans up filesystem artifacts, and deactivates the role if active.
pub async fn uninstall_store_product(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;

    // Get product detail before removing so we know the slug and type
    let detail = api.get_skill(&id).await.ok();

    // Unregister from NeboLoop
    let _ = api.uninstall_product(&id).await;

    // Determine slug and type from local DB first, then NeboLoop as fallback
    let local_role = state.store.get_role(&id).ok().flatten();
    let slug = detail.as_ref().map(|d| d.item.slug.clone()).unwrap_or_default();
    let artifact_type = detail.as_ref().and_then(|d| d.artifact_type.as_deref()).unwrap_or("");

    // Derive slug from role name if NeboLoop didn't provide it
    let slug = if slug.is_empty() {
        local_role.as_ref()
            .map(|r| r.name.to_lowercase().replace(' ', "-"))
            .unwrap_or_default()
    } else {
        slug
    };

    // Determine artifact type from local DB kind or NeboLoop
    let is_role = artifact_type == "role"
        || local_role.is_some()
        || local_role.as_ref().and_then(|r| r.kind.as_deref()).map(|k| k.starts_with("ROLE-")).unwrap_or(false);

    // Clean up local DB
    if let Some(ref role) = local_role {
        // Deactivate from live registry
        state.role_registry.write().await.remove(&id);
        // Remove workflow bindings and triggers
        workflow::triggers::unregister_role_triggers(&id, &state.store);
        let _ = state.store.delete_role_workflows(&id);
        let _ = state.store.delete_role(&id);
        state.hub.broadcast(
            "role_deactivated",
            serde_json::json!({ "roleId": id, "name": role.name }),
        );
    }

    // Clean up filesystem
    if !slug.is_empty() {
        if let Ok(nebo_dir) = config::nebo_dir() {
            let subdir = if is_role { "roles" } else { "skills" };
            let artifact_dir = nebo_dir.join(subdir).join(&slug);
            if artifact_dir.exists() {
                let _ = std::fs::remove_dir_all(&artifact_dir);
            }
        }
    }

    // Reload skill loader in case a skill was removed
    state.skill_loader.load_all().await;

    Ok(Json(serde_json::json!({ "success": true })))
}
