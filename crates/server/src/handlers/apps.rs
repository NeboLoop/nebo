//! App platform handlers.
//!
//! Serves static UI assets, proxies requests to sidecar binaries,
//! and provides storage/agent/janus endpoints for the @neboai/app-sdk.

use std::path::{Path as StdPath, PathBuf};

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use futures::Stream;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::handlers::{HandlerResult, to_error_response};
use crate::state::AppState;
use db;

/// Validate the per-app auth token from the `Authorization: Bearer <token>` header.
///
/// Returns Ok(()) if the token matches the running app's token.
/// Returns 401 if the token is missing/invalid, or if the app has no running lifecycle.
async fn validate_app_token(
    state: &AppState,
    agent_id: &str,
    headers: &axum::http::HeaderMap,
) -> Result<(), Response> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    let token = match token {
        Some(t) => t,
        None => {
            return Err((StatusCode::UNAUTHORIZED, "missing Authorization: Bearer <token>").into_response());
        }
    };

    let lifecycles = state.app_lifecycles.read().await;
    let lifecycle = match lifecycles.get(agent_id) {
        Some(lc) => lc,
        None => {
            return Err((StatusCode::UNAUTHORIZED, "app not running").into_response());
        }
    };

    let expected = lifecycle.app_token().await;
    if expected.is_empty() || token != expected {
        return Err((StatusCode::UNAUTHORIZED, "invalid app token").into_response());
    }

    Ok(())
}

/// Check that the app has `network:{domain}` permission for the target URL.
async fn check_network_permission(
    state: &AppState,
    agent_id: &str,
    url: &str,
) -> Result<(), Response> {
    let domain = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(String::from));
    let domain = match domain {
        Some(d) => d,
        None => {
            return Err((StatusCode::BAD_REQUEST, "invalid URL").into_response());
        }
    };

    let lifecycles = state.app_lifecycles.read().await;
    if let Some(lifecycle) = lifecycles.get(agent_id) {
        let perm = format!("network:{}", domain);
        if !lifecycle.has_permission(&perm).await {
            return Err((
                StatusCode::FORBIDDEN,
                format!("app lacks permission: {}", perm),
            )
                .into_response());
        }
    }
    Ok(())
}

/// Check that the app has `subagent:{target}` permission to invoke another agent.
async fn check_subagent_permission(
    state: &AppState,
    app_agent_id: &str,
    target_agent_id: &str,
) -> Result<(), Response> {
    // Self-invocation is always allowed
    if app_agent_id == target_agent_id {
        return Ok(());
    }
    let lifecycles = state.app_lifecycles.read().await;
    if let Some(lifecycle) = lifecycles.get(app_agent_id) {
        let perm = format!("subagent:{}", target_agent_id);
        if !lifecycle.has_permission(&perm).await {
            return Err((
                StatusCode::FORBIDDEN,
                format!("app lacks permission: {}", perm),
            )
                .into_response());
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct InvokeRequest {
    message: String,
    agent: Option<String>,
    data: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InvokeResponse {
    text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct JanusRequest {
    messages: Vec<ai::Message>,
    model: Option<String>,
    temperature: Option<f64>,
    max_tokens: Option<i32>,
    system: Option<String>,
}

/// Resolve the UI directory path for an app agent.
async fn resolve_app_ui_path(state: &AppState, agent_id: &str) -> Option<PathBuf> {
    let agents = state.agent_loader.list().await;
    let fs_match = agents.iter().find(|a| {
        a.id.as_deref() == Some(agent_id)
            || a.agent_def.name.eq_ignore_ascii_case(agent_id)
    });
    if let Some(p) = fs_match.and_then(|a| a.app_ui_path.clone()) {
        return Some(p);
    }
    // Fall back to DB
    if let Ok(Some(a)) = state.store.get_agent(agent_id) {
        if let Some(p) = a.app_ui_path {
            return Some(PathBuf::from(p));
        }
    }
    None
}

/// GET /apps/{agent_id}/ui/ — serve the app's index.html at the root path.
pub async fn serve_app_ui_root(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Response {
    serve_app_ui_inner(&state, &agent_id, "").await
}

/// GET /apps/{agent_id}/ui/*path — serve static app assets with SPA fallback.
pub async fn serve_app_ui(
    State(state): State<AppState>,
    Path((agent_id, path)): Path<(String, String)>,
) -> Response {
    serve_app_ui_inner(&state, &agent_id, &path).await
}

async fn serve_app_ui_inner(state: &AppState, agent_id: &str, path: &str) -> Response {
    let ui_path = match resolve_app_ui_path(state, agent_id).await {
        Some(p) => p,
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    // Sanitize path to prevent directory traversal
    let clean_path = path.trim_start_matches('/');
    if clean_path.contains("..") {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let file_path = ui_path.join(clean_path);

    // Try the exact file first, then SPA fallback (index.html, then 200.html)
    let target = if file_path.is_file() {
        file_path
    } else {
        let index = ui_path.join("index.html");
        if index.is_file() {
            index
        } else {
            let fallback = ui_path.join("200.html");
            if fallback.is_file() {
                fallback
            } else {
                return StatusCode::NOT_FOUND.into_response();
            }
        }
    };

    match fs::read(&target).await {
        Ok(contents) => {
            let mime = mime_from_path(&target);
            let is_entry = matches!(target.file_name().and_then(|n| n.to_str()), Some("index.html" | "200.html"));

            let mut response = Response::new(Body::from(contents));
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_str(mime)
                    .unwrap_or(HeaderValue::from_static("application/octet-stream")),
            );
            if !is_entry {
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

/// GET /sdk/nebo.global.js — serve the app SDK IIFE build for vanilla/HTMX apps.
pub async fn serve_sdk_iife() -> Response {
    let path =
        StdPath::new(env!("CARGO_MANIFEST_DIR")).join("../../app/node_modules/@neboai/app-sdk/dist/nebo.global.js");
    match fs::read(&path).await {
        Ok(contents) => {
            let mut response = Response::new(Body::from(contents));
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/javascript; charset=utf-8"),
            );
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=3600"),
            );
            response
        }
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// GET/PUT/DELETE /apps/{agent_id}/storage/{key} — app-scoped KV storage.
pub async fn get_storage(
    State(state): State<AppState>,
    Path((agent_id, key)): Path<(String, String)>,
    headers: axum::http::HeaderMap,
) -> Response {
    if let Err(r) = validate_app_token(&state, &agent_id, &headers).await {
        return r;
    }
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
    headers: axum::http::HeaderMap,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> Response {
    if let Err(r) = validate_app_token(&state, &agent_id, &headers).await {
        return r;
    }
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
    headers: axum::http::HeaderMap,
) -> Response {
    if let Err(r) = validate_app_token(&state, &agent_id, &headers).await {
        return r;
    }
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
    headers: axum::http::HeaderMap,
) -> Response {
    if let Err(r) = validate_app_token(&state, &agent_id, &headers).await {
        return r;
    }
    let plugin_name = format!("app:{}", agent_id);
    match state.store.list_plugin_settings(&plugin_name) {
        Ok(items) => axum::Json(serde_json::json!({ "items": items })).into_response(),
        Err(e) => {
            warn!(error = %e, "app storage list failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// POST /apps/{agent_id}/agents/invoke — run the app's agent and collect text.
pub async fn invoke_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    headers: axum::http::HeaderMap,
    axum::Json(body): axum::Json<InvokeRequest>,
) -> Response {
    if let Err(r) = validate_app_token(&state, &agent_id, &headers).await {
        return r;
    }
    let (target_agent_id, agent_name) = match validate_app_agent(&state, &agent_id, body.agent.as_deref()) {
        Ok(v) => v,
        Err(e) => return to_error_response(e).into_response(),
    };
    if let Err(r) = check_subagent_permission(&state, &agent_id, &target_agent_id).await {
        return r;
    }
    match run_agent_collect(&state, &target_agent_id, &agent_name, body).await {
        Ok((text, tools)) => axum::Json(InvokeResponse { text, tools }).into_response(),
        Err(e) => to_error_response(e).into_response(),
    }
}

/// POST /apps/{agent_id}/agents/stream — run the app's agent and stream SSE chunks.
pub async fn stream_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    headers: axum::http::HeaderMap,
    axum::Json(body): axum::Json<InvokeRequest>,
) -> Response {
    if let Err(r) = validate_app_token(&state, &agent_id, &headers).await {
        return r;
    }
    let (target_agent_id, agent_name) =
        match validate_app_agent(&state, &agent_id, body.agent.as_deref()) {
            Ok(v) => v,
            Err(e) => return to_error_response(e).into_response(),
        };
    if let Err(r) = check_subagent_permission(&state, &agent_id, &target_agent_id).await {
        return r;
    }
    let stream = run_agent_sse(state, target_agent_id, agent_name, body).await;
    Sse::new(stream).into_response()
}

/// POST /apps/{agent_id}/janus/complete — direct provider completion for apps.
pub async fn janus_complete(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    headers: axum::http::HeaderMap,
    axum::Json(body): axum::Json<JanusRequest>,
) -> Response {
    if let Err(r) = validate_app_token(&state, &agent_id, &headers).await {
        return r;
    }
    if let Err(e) = validate_app_agent(&state, &agent_id, None) {
        return to_error_response(e).into_response();
    }
    match run_janus_collect(&state, body).await {
        Ok((text, usage)) => {
            axum::Json(serde_json::json!({ "text": text, "usage": usage })).into_response()
        }
        Err(e) => to_error_response(e).into_response(),
    }
}

/// POST /apps/{agent_id}/janus/stream — direct provider SSE streaming for apps.
pub async fn janus_stream(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    headers: axum::http::HeaderMap,
    axum::Json(body): axum::Json<JanusRequest>,
) -> Response {
    if let Err(r) = validate_app_token(&state, &agent_id, &headers).await {
        return r;
    }
    if let Err(e) = validate_app_agent(&state, &agent_id, None) {
        return to_error_response(e).into_response();
    }
    let stream = run_janus_sse(state, body).await;
    Sse::new(stream).into_response()
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
    // Block access to internal sidecar endpoints — these are for Nebo's internal
    // use only (tool discovery, health checks). Never expose to HTTP clients.
    let clean = path.trim_start_matches('/');
    if clean == "_tools" || clean.starts_with("_") {
        return StatusCode::FORBIDDEN.into_response();
    }

    let agent = match state.store.get_agent(&agent_id) {
        Ok(Some(a)) if a.is_app.unwrap_or(0) != 0 => a,
        _ => return StatusCode::NOT_FOUND.into_response(),
    };

    // Find the Unix socket for this app's sidecar
    let sock_path = match sidecar_sock_path(&agent) {
        Some(p) => p,
        None => return (StatusCode::SERVICE_UNAVAILABLE, "app has no sidecar path").into_response(),
    };

    // Auto-launch sidecar on first request (like plugins do on-demand)
    if !sock_path.exists() {
        let mut lifecycles = state.app_lifecycles.write().await;
        if !lifecycles.contains_key(&agent_id) {
            if let Some(tool_dir) = super::agents::app_tool_dir(&agent) {
                let mut lifecycle = crate::app_lifecycle::AppLifecycle::new(
                    agent_id.clone(),
                    tool_dir,
                    state.hub.clone(),
                    state.tools.clone(),
                    state.skill_loader.clone(),
                    state.config.port,
                );
                match lifecycle.launch().await {
                    Ok(()) => {
                        lifecycles.insert(agent_id.clone(), lifecycle);
                    }
                    Err(e) => {
                        warn!(agent = %agent_id, error = %e, "auto-launch sidecar failed");
                        return (StatusCode::SERVICE_UNAVAILABLE, "sidecar launch failed").into_response();
                    }
                }
            } else {
                return (StatusCode::SERVICE_UNAVAILABLE, "app has no sidecar directory").into_response();
            }
        }
    }

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
    let channel = tonic::transport::Endpoint::from_static("http://[::]:50051")
        .connect_with_connector_lazy(tower::service_fn(move |_: tonic::transport::Uri| {
            let sock = sock_path.clone();
            async move {
                tokio::net::UnixStream::connect(sock)
                    .await
                    .map(hyper_util::rt::TokioIo::new)
            }
        }));

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
            response
                .body(Body::from(inner.body))
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
    // Prefer napp_path if set, otherwise derive from agent directory
    let dir = agent
        .napp_path
        .as_ref()
        .map(PathBuf::from)
        .or_else(|| super::agents::app_tool_dir(agent))?;
    Some(dir.join(format!("{}.sock", agent.id)))
}

fn validate_app_agent(
    state: &AppState,
    app_agent_id: &str,
    override_agent: Option<&str>,
) -> Result<(String, String), types::NeboError> {
    let app = state
        .store
        .get_agent(app_agent_id)?
        .ok_or(types::NeboError::NotFound)?;
    if app.is_app.unwrap_or(0) == 0 {
        return Err(types::NeboError::Unauthorized);
    }
    if let Some(target) = override_agent {
        let agent = state
            .store
            .get_agent(target)?
            .ok_or(types::NeboError::NotFound)?;
        return Ok((agent.id, agent.name));
    }
    Ok((app.id, app.name))
}

async fn run_agent_collect(
    state: &AppState,
    agent_id: &str,
    agent_name: &str,
    body: InvokeRequest,
) -> Result<(String, Vec<serde_json::Value>), types::NeboError> {
    let mut rx = start_app_agent_run(state, agent_id, agent_name, body).await?;
    let mut text = String::new();
    let mut tools = Vec::new();
    while let Some(event) = rx.recv().await {
        match event.event_type {
            ai::StreamEventType::Text => text.push_str(&event.text),
            ai::StreamEventType::ToolCall | ai::StreamEventType::ToolResult => {
                if let Some(tool_call) = event.tool_call {
                    if let Ok(value) = serde_json::to_value(tool_call) {
                        tools.push(value);
                    }
                }
            }
            ai::StreamEventType::Error => {
                if let Some(error) = event.error {
                    return Err(types::NeboError::Internal(error));
                }
            }
            _ => {}
        }
    }
    Ok((text, tools))
}

async fn run_agent_sse(
    state: AppState,
    agent_id: String,
    agent_name: String,
    body: InvokeRequest,
) -> impl Stream<Item = Result<Event, std::convert::Infallible>> {
    let (tx, rx) = tokio::sync::mpsc::channel(32);
    tokio::spawn(async move {
        match start_app_agent_run(&state, &agent_id, &agent_name, body).await {
            Ok(mut events) => {
                while let Some(event) = events.recv().await {
                    let data = serde_json::json!({
                        "text": event.text,
                        "done": false,
                        "type": format!("{:?}", event.event_type),
                    });
                    if tx
                        .send(Ok(Event::default().data(data.to_string())))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                let _ = tx
                    .send(Ok(Event::default().data(r#"{"text":"","done":true}"#)))
                    .await;
            }
            Err(e) => {
                let data = serde_json::json!({ "error": e.to_string(), "done": true });
                let _ = tx.send(Ok(Event::default().data(data.to_string()))).await;
            }
        }
    });
    futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    })
}

async fn start_app_agent_run(
    state: &AppState,
    agent_id: &str,
    agent_name: &str,
    body: InvokeRequest,
) -> Result<tokio::sync::mpsc::Receiver<ai::StreamEvent>, types::NeboError> {
    let session_key = format!("app:{}:api", agent_id);
    let cancel_token = CancellationToken::new();
    let entity_config = crate::entity_config::resolve_for_chat(&state.store, "agent", agent_id);
    let mention_context = body.data.map(|data| format!("App data context: {}", data));
    crate::chat_dispatch::run_chat_events(
        state,
        crate::chat_dispatch::ChatConfig {
            session_key,
            prompt: body.message,
            system: String::new(),
            user_id: String::new(),
            channel: "app".to_string(),
            origin: tools::Origin::App,
            agent_id: agent_id.to_string(),
            cancel_token,
            lane: types::constants::lanes::EVENTS.to_string(),
            comm_reply: None,
            entity_config,
            images: vec![],
            entity_name: agent_name.to_string(),
            origin_agent_id: None,
            mention_context,
            tool_scope: None, plan_mode: false,
            channel_ctx: None,
        },
    )
    .await
}

async fn run_janus_collect(
    state: &AppState,
    body: JanusRequest,
) -> Result<(String, Option<ai::UsageInfo>), types::NeboError> {
    let mut rx = start_janus_stream(state, body).await?;
    let mut text = String::new();
    let mut usage = None;
    while let Some(event) = rx.recv().await {
        if event.event_type == ai::StreamEventType::Text {
            text.push_str(&event.text);
        }
        if event.usage.is_some() {
            usage = event.usage;
        }
        if let Some(error) = event.error {
            return Err(types::NeboError::Internal(error));
        }
    }
    Ok((text, usage))
}

async fn run_janus_sse(
    state: AppState,
    body: JanusRequest,
) -> impl Stream<Item = Result<Event, std::convert::Infallible>> {
    let (tx, rx) = tokio::sync::mpsc::channel(32);
    tokio::spawn(async move {
        match start_janus_stream(&state, body).await {
            Ok(mut events) => {
                while let Some(event) = events.recv().await {
                    let data = serde_json::json!({
                        "text": event.text,
                        "done": false,
                    });
                    if tx
                        .send(Ok(Event::default().data(data.to_string())))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                let _ = tx.send(Ok(Event::default().data("[DONE]"))).await;
            }
            Err(e) => {
                let data = serde_json::json!({ "error": e.to_string(), "done": true });
                let _ = tx.send(Ok(Event::default().data(data.to_string()))).await;
            }
        }
    });
    futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    })
}

async fn start_janus_stream(
    state: &AppState,
    body: JanusRequest,
) -> Result<tokio::sync::mpsc::Receiver<ai::StreamEvent>, types::NeboError> {
    let providers = state.runner.providers();
    let providers = providers.read().await;
    let provider = providers
        .first()
        .cloned()
        .ok_or_else(|| types::NeboError::Internal("no providers available".into()))?;
    drop(providers);

    let req = ai::ChatRequest {
        messages: body.messages,
        tools: vec![],
        max_tokens: body.max_tokens.unwrap_or(2000),
        temperature: body.temperature.unwrap_or(0.7),
        system: body.system.unwrap_or_default(),
        static_system: String::new(),
        model: body.model.unwrap_or_default(),
        enable_thinking: false,
        metadata: None,
        cache_breakpoints: vec![],
        cancel_token: Some(CancellationToken::new()),
    };
    provider
        .stream(&req)
        .await
        .map_err(|e| types::NeboError::Internal(format!("provider stream failed: {e}")))
}

/// POST /apps/{agent_id}/http/proxy — CORS-free outbound HTTP proxy.
pub async fn http_proxy(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    headers: axum::http::HeaderMap,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> Response {
    if let Err(r) = validate_app_token(&state, &agent_id, &headers).await {
        return r;
    }
    let url = match body.get("url").and_then(|v| v.as_str()) {
        Some(u) => u.to_string(),
        None => return (StatusCode::BAD_REQUEST, "missing url field").into_response(),
    };

    // Enforce network: permission from manifest
    if let Err(r) = check_network_permission(&state, &agent_id, &url).await {
        return r;
    }

    let method = body
        .get("method")
        .and_then(|v| v.as_str())
        .unwrap_or("GET")
        .to_uppercase();

    let headers: Vec<(String, String)> = body
        .get("headers")
        .and_then(|h| h.as_object())
        .map(|obj| {
            obj.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                .collect()
        })
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
            let resp_headers: Vec<(String, String)> = resp
                .headers()
                .iter()
                .filter_map(|(k, v)| Some((k.to_string(), v.to_str().ok()?.to_string())))
                .collect();
            let body_bytes = resp.bytes().await.unwrap_or_default();
            axum::Json(serde_json::json!({
                "status": status,
                "headers": resp_headers.into_iter().collect::<std::collections::HashMap<_, _>>(),
                "body": String::from_utf8_lossy(&body_bytes),
            }))
            .into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY, format!("proxy error: {}", e)).into_response(),
    }
}

/// GET /apps/{agent_id}/identity — expose agent context for the app SDK.
pub async fn get_identity(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let agent = state
        .store
        .get_agent(&agent_id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    if agent.is_app.unwrap_or(0) == 0 {
        return Err(to_error_response(types::NeboError::Unauthorized));
    }

    // Parse frontmatter for model + skills
    let frontmatter_val: serde_json::Value = if !agent.frontmatter.is_empty() {
        serde_json::from_str(&agent.frontmatter).unwrap_or_default()
    } else {
        serde_json::Value::Null
    };
    let model = frontmatter_val
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let skills: Vec<&str> = frontmatter_val
        .get("skills")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|s| s.as_str()).collect())
        .unwrap_or_default();

    // Extract persona body (markdown after frontmatter)
    let (_yaml_str, persona_body) =
        napp::agent::split_frontmatter(&agent.agent_md).unwrap_or_default();

    // Compute display name
    let display_name = agent
        .app_window_config
        .as_ref()
        .and_then(|cfg_str| serde_json::from_str::<serde_json::Value>(cfg_str).ok())
        .and_then(|cfg| cfg.get("title").and_then(|t| t.as_str().map(|s| s.to_string())))
        .filter(|t| !t.is_empty())
        .or_else(|| {
            persona_body.lines().find_map(|line| {
                line.trim()
                    .strip_prefix("# ")
                    .map(|h| h.trim().to_string())
                    .filter(|h| !h.is_empty())
            })
        })
        .unwrap_or_else(|| agent.name.clone());

    // Parse input_values from DB
    let input_values: serde_json::Value =
        serde_json::from_str(&agent.input_values).unwrap_or(serde_json::json!({}));

    Ok(axum::Json(serde_json::json!({
        "id": agent.id,
        "name": agent.name,
        "displayName": display_name,
        "description": agent.description,
        "persona": persona_body,
        "model": model,
        "skills": skills,
        "inputValues": input_values,
    })))
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
