pub mod agent;
pub mod auth;
pub mod chat;
pub mod entity_config;
pub mod files;
pub mod integrations;
pub mod mcp_server;
pub mod memory;
pub mod neboloop;
pub mod notification;
pub mod plugins;
pub mod provider;
pub mod agents;
pub mod setup;
pub mod skills;
pub mod store;
pub mod tasks;
pub mod user;
pub mod workflows;
pub mod ws;
pub mod commander;

use axum::http::StatusCode;
use axum::response::Json;
use types::api::ErrorResponse;
use types::NeboError;

/// Convert a NeboError into an Axum error response tuple.
pub fn to_error_response(e: NeboError) -> (StatusCode, Json<ErrorResponse>) {
    let status = StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    if status.is_server_error() {
        tracing::error!(status = %status, error = %e, "handler error");
    }
    (
        status,
        Json(ErrorResponse {
            error: e.to_string(),
        }),
    )
}

/// Shorthand result type for handlers.
pub type HandlerResult<T> = Result<Json<T>, (StatusCode, Json<ErrorResponse>)>;
