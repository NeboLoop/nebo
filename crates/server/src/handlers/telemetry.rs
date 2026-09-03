use axum::Json;
use axum::http::HeaderMap;
use axum::http::header::USER_AGENT;
use serde::Deserialize;
use tracing::info;

use super::HandlerResult;

/// POST /api/v1/client/events. One line in the server log per client-side
/// connection event (socket open, close code, visibility change, a read that
/// stalled), so a phone's side of a dropped session shows up next to the
/// server's own lines. Nothing is stored.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientEventRequest {
    pub event: String,
    #[serde(default)]
    pub page: String,
    #[serde(default)]
    pub detail: String,
    pub duration_ms: Option<u64>,
    pub code: Option<i64>,
}

pub async fn client_event(
    headers: HeaderMap,
    Json(body): Json<ClientEventRequest>,
) -> HandlerResult<serde_json::Value> {
    let ua = headers
        .get(USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    info!(
        target: "client",
        event = %body.event,
        page = %body.page,
        detail = %body.detail,
        duration_ms = body.duration_ms,
        code = body.code,
        ua,
        "client event"
    );
    Ok(Json(serde_json::json!({ "ok": true })))
}
