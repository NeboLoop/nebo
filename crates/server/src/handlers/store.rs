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

/// GET /store/apps — list marketplace apps.
pub async fn list_store_apps(
    State(state): State<AppState>,
    Query(params): Query<StoreQuery>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .list_apps(
            params.q.as_deref(),
            params.category.as_deref(),
            params.page,
            params.page_size,
        )
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("list_apps: {e}"))))?;
    Ok(Json(serde_json::to_value(resp).unwrap_or_default()))
}

/// GET /store/skills — list marketplace skills.
pub async fn list_store_skills(
    State(state): State<AppState>,
    Query(params): Query<StoreQuery>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .list_skills(
            params.q.as_deref(),
            params.category.as_deref(),
            params.page,
            params.page_size,
        )
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("list_skills: {e}"))))?;
    Ok(Json(serde_json::to_value(resp).unwrap_or_default()))
}

/// GET /store/workflows — list marketplace workflows.
pub async fn list_store_workflows(
    State(state): State<AppState>,
    Query(params): Query<StoreQuery>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .list_workflows(
            params.q.as_deref(),
            params.category.as_deref(),
            params.page,
            params.page_size,
        )
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("list_workflows: {e}"))))?;
    Ok(Json(serde_json::to_value(resp).unwrap_or_default()))
}

/// POST /store/skills/{id}/install — install a skill by code/id.
pub async fn install_store_skill(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .install_skill(&id)
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("install_skill: {e}"))))?;
    Ok(Json(serde_json::to_value(resp).unwrap_or_default()))
}

/// DELETE /store/skills/{id}/install — uninstall a skill.
pub async fn uninstall_store_skill(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    api.uninstall_skill(&id)
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("uninstall_skill: {e}"))))?;
    Ok(Json(serde_json::json!({ "success": true })))
}

// ── New Marketplace Proxy Endpoints ─────────────────────────────────

/// GET /store/products — unified product listing via `/api/v1/products`.
/// Query params: type (skill|workflow|role), category, q, page, pageSize.
/// Returns `{ "skills": [...] }` from NeboLoop's unified products endpoint,
/// enriched with local install state.
pub async fn list_store_products(
    State(state): State<AppState>,
    Query(params): Query<StoreQuery>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let mut resp = api
        .list_products(
            params.artifact_type.as_deref(),
            params.q.as_deref(),
            params.category.as_deref(),
            params.page,
            params.page_size,
        )
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("list_products: {e}"))))?;

    // Enrich with local install state — NeboLoop doesn't know what's on this machine
    enrich_installed_state(&mut resp);

    Ok(Json(resp))
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
    enrich_installed_item(&mut val);

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
        .map_err(|e| to_error_response(NeboError::Internal(format!("get_media: {e}"))))?;
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
        .map_err(|e| to_error_response(NeboError::Internal(format!("get_feedback: {e}"))))?;
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

/// Check if a product is installed locally by slug/name, checking both
/// user and nebo artifact directories for skills and roles.
fn is_locally_installed(slug: &str, artifact_type: &str) -> bool {
    let (user_dir, nebo_dir) = match (config::user_dir(), config::nebo_dir()) {
        (Ok(u), Ok(n)) => (u, n),
        _ => return false,
    };

    let subdir = match artifact_type {
        "role" => "roles",
        "skill" | _ => "skills",
    };

    // Check user dir: slug/ or slug.yaml
    let user_path = user_dir.join(subdir).join(slug);
    if user_path.exists() {
        return true;
    }
    if user_dir.join(subdir).join(format!("{}.yaml", slug)).exists() {
        return true;
    }

    // Check nebo (marketplace) dir: slug/
    let nebo_path = nebo_dir.join(subdir).join(slug);
    if nebo_path.exists() {
        return true;
    }

    false
}

/// Enrich a single product JSON value with local install state.
fn enrich_installed_item(val: &mut serde_json::Value) {
    if let Some(obj) = val.as_object_mut() {
        let slug = obj.get("slug").and_then(|v| v.as_str()).unwrap_or("");
        let artifact_type = obj.get("type").and_then(|v| v.as_str()).unwrap_or("skill");
        if !slug.is_empty() && is_locally_installed(slug, artifact_type) {
            obj.insert("installed".to_string(), serde_json::Value::Bool(true));
        }
    }
}

/// Enrich a product list response with local install state.
/// Looks for `{ "skills": [...] }` structure returned by NeboLoop.
fn enrich_installed_state(resp: &mut serde_json::Value) {
    if let Some(items) = resp.get_mut("skills").and_then(|v| v.as_array_mut()) {
        for item in items.iter_mut() {
            enrich_installed_item(item);
        }
    }
}

/// POST /store/products/{id}/install — install a product by code/id.
pub async fn install_store_product(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .install_skill(&id)
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("install_product: {e}"))))?;
    Ok(Json(serde_json::to_value(resp).unwrap_or_default()))
}
