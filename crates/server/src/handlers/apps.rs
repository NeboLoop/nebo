//! App platform handlers.
//!
//! Serves static UI assets, proxies requests to sidecar binaries,
//! and provides storage/agent/janus endpoints for the @neboai/app-sdk.

use std::path::PathBuf;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use tokio::fs;
use tracing::warn;

use crate::state::AppState;
use db;

/// GET /apps/{agent_id}/ui/*path — serve static app assets with SPA fallback.
pub async fn serve_app_ui(
    State(state): State<AppState>,
    Path((agent_id, path)): Path<(String, String)>,
) -> Response {
    // Look up the agent to get its UI path
    let agent = match state.store.get_agent(&agent_id) {
        Ok(Some(a)) => a,
        _ => return StatusCode::NOT_FOUND.into_response(),
    };

    let ui_path = match agent.app_ui_path {
        Some(ref p) => PathBuf::from(p),
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    // Sanitize path to prevent directory traversal
    let clean_path = path.trim_start_matches('/');
    if clean_path.contains("..") {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let file_path = ui_path.join(clean_path);

    // Try the exact file first, then SPA fallback to index.html
    let target = if file_path.is_file() {
        file_path
    } else {
        let index = ui_path.join("index.html");
        if index.is_file() {
            index
        } else {
            return StatusCode::NOT_FOUND.into_response();
        }
    };

    match fs::read(&target).await {
        Ok(contents) => {
            let mime = mime_from_path(&target);
            let mut response = Response::new(Body::from(contents));
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_str(mime).unwrap_or(HeaderValue::from_static("application/octet-stream")),
            );
            // Cache static assets (not index.html for SPA freshness)
            if target.file_name().and_then(|n| n.to_str()) != Some("index.html") {
                response.headers_mut().insert(
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("public, max-age=3600"),
                );
            }
            response
        }
        Err(e) => {
            warn!(path = %target.display(), error = %e, "failed to read app UI file");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// GET/PUT/DELETE /apps/{agent_id}/storage/{key} — app-scoped KV storage.
pub async fn get_storage(
    State(state): State<AppState>,
    Path((agent_id, key)): Path<(String, String)>,
) -> Response {
    let plugin_name = format!("app:{}", agent_id);
    match state.store.get_plugin_setting(&plugin_name, &key) {
        Ok(Some(v)) => axum::Json(serde_json::json!({ "key": key, "value": v })).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            warn!(error = %e, "app storage get failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn put_storage(
    State(state): State<AppState>,
    Path((agent_id, key)): Path<(String, String)>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> Response {
    let plugin_name = format!("app:{}", agent_id);
    let value = match body.get("value") {
        Some(v) => v.to_string(),
        None => return StatusCode::BAD_REQUEST.into_response(),
    };
    // Ensure plugin_registry entry exists for storage
    if let Err(e) = state.store.ensure_plugin_registry_entry(&plugin_name) {
        warn!(error = %e, "failed to ensure plugin registry for app storage");
    }
    match state.store.set_plugin_setting(&plugin_name, &key, &value) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            warn!(error = %e, "app storage put failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn delete_storage(
    State(state): State<AppState>,
    Path((agent_id, key)): Path<(String, String)>,
) -> Response {
    let plugin_name = format!("app:{}", agent_id);
    // Delete by setting empty — plugin_settings doesn't have a delete, use set with empty
    match state.store.set_plugin_setting(&plugin_name, &key, "") {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            warn!(error = %e, "app storage delete failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn list_storage(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Response {
    let plugin_name = format!("app:{}", agent_id);
    match state.store.list_plugin_settings(&plugin_name) {
        Ok(items) => axum::Json(serde_json::json!({ "items": items })).into_response(),
        Err(e) => {
            warn!(error = %e, "app storage list failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// ANY /apps/{agent_id}/api/*path — proxy HTTP request to sidecar via gRPC UIService.
///
/// Sidecar binaries communicate over Unix socket. Nebo converts the HTTP request
/// to a gRPC `UIService.HandleRequest` call and returns the response.
pub async fn proxy_to_sidecar(
    State(state): State<AppState>,
    Path((agent_id, path)): Path<(String, String)>,
    req: axum::http::Request<Body>,
) -> Response {
    let agent = match state.store.get_agent(&agent_id) {
        Ok(Some(a)) if a.is_app.unwrap_or(0) != 0 => a,
        _ => return StatusCode::NOT_FOUND.into_response(),
    };

    // Find the Unix socket for this app's sidecar
    let sock_path = match sidecar_sock_path(&agent) {
        Some(p) if p.exists() => p,
        _ => return (StatusCode::SERVICE_UNAVAILABLE, "app sidecar not running").into_response(),
    };

    // Extract HTTP request parts for gRPC
    let method = req.method().to_string();
    let query = req.uri().query().unwrap_or("").to_string();
    let mut headers_map = std::collections::HashMap::new();
    for (name, value) in req.headers() {
        if let Ok(v) = value.to_str() {
            headers_map.insert(name.to_string(), v.to_string());
        }
    }
    let body_bytes = match axum::body::to_bytes(req.into_body(), 10 * 1024 * 1024).await {
        Ok(b) => b.to_vec(),
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    // Connect to sidecar via Unix socket gRPC
    let endpoint = format!("unix://{}", sock_path.display());
    let channel = match tonic::transport::Endpoint::from_shared(endpoint)
        .and_then(|e| Ok(e.connect_lazy()))
    {
        Ok(c) => c,
        Err(e) => {
            warn!(agent = %agent_id, error = %e, "failed to connect to sidecar gRPC");
            return (StatusCode::BAD_GATEWAY, "sidecar connection failed").into_response();
        }
    };

    let mut client = proto::ui_service_client::UiServiceClient::new(channel);
    let grpc_req = proto::HttpRequest {
        method,
        path: path.trim_start_matches('/').to_string(),
        query,
        headers: headers_map,
        body: body_bytes,
    };

    match client.handle_request(grpc_req).await {
        Ok(resp) => {
            let inner = resp.into_inner();
            let status = StatusCode::from_u16(inner.status_code as u16)
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let mut response = Response::builder().status(status);
            for (name, value) in &inner.headers {
                if let Ok(v) = HeaderValue::from_str(value) {
                    response = response.header(name.as_str(), v);
                }
            }
            response.body(Body::from(inner.body))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Err(e) => {
            warn!(agent = %agent_id, error = %e, "sidecar gRPC call failed");
            (StatusCode::BAD_GATEWAY, format!("sidecar error: {}", e)).into_response()
        }
    }
}

/// Get the Unix socket path for an app's sidecar process.
fn sidecar_sock_path(agent: &db::models::Agent) -> Option<std::path::PathBuf> {
    // Socket lives next to the agent directory: {agent_dir}/{id}.sock
    agent.napp_path.as_ref()
        .map(|p| std::path::PathBuf::from(p).join(format!("{}.sock", agent.id)))
}

/// POST /apps/{agent_id}/http/proxy — CORS-free outbound HTTP proxy.
pub async fn http_proxy(
    State(_state): State<AppState>,
    Path(agent_id): Path<String>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> Response {
    let url = match body.get("url").and_then(|v| v.as_str()) {
        Some(u) => u.to_string(),
        None => return (StatusCode::BAD_REQUEST, "missing url field").into_response(),
    };

    let method = body.get("method")
        .and_then(|v| v.as_str())
        .unwrap_or("GET")
        .to_uppercase();

    let headers: Vec<(String, String)> = body.get("headers")
        .and_then(|h| h.as_object())
        .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect())
        .unwrap_or_default();

    let req_body = body.get("body").and_then(|b| b.as_str()).map(String::from);

    let client = reqwest::Client::new();
    let mut req = match method.as_str() {
        "GET" => client.get(&url),
        "POST" => client.post(&url),
        "PUT" => client.put(&url),
        "DELETE" => client.delete(&url),
        "PATCH" => client.patch(&url),
        _ => return (StatusCode::BAD_REQUEST, "unsupported method").into_response(),
    };

    for (k, v) in &headers {
        req = req.header(k.as_str(), v.as_str());
    }

    // Add identifying header
    req = req.header("X-Nebo-App", &agent_id);

    if let Some(b) = req_body {
        req = req.body(b);
    }

    match req.send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let resp_headers: Vec<(String, String)> = resp.headers().iter()
                .filter_map(|(k, v)| Some((k.to_string(), v.to_str().ok()?.to_string())))
                .collect();
            let body_bytes = resp.bytes().await.unwrap_or_default();
            axum::Json(serde_json::json!({
                "status": status,
                "headers": resp_headers.into_iter().collect::<std::collections::HashMap<_, _>>(),
                "body": String::from_utf8_lossy(&body_bytes),
            })).into_response()
        }
        Err(e) => {
            (StatusCode::BAD_GATEWAY, format!("proxy error: {}", e)).into_response()
        }
    }
}

/// Determine MIME type from file extension.
fn mime_from_path(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "application/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    }
}
