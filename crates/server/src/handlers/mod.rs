pub mod agent;
pub mod auth;
pub mod chat;
pub mod files;
pub mod integrations;
pub mod mcp_server;
pub mod memory;
pub mod neboloop;
pub mod notification;
pub mod provider;
pub mod roles;
pub mod setup;
pub mod skills;
pub mod store;
pub mod tasks;
pub mod user;
pub mod workflows;
pub mod ws;

use axum::http::StatusCode;
use axum::response::Json;
use types::api::ErrorResponse;
use types::NeboError;

/// Convert a NeboError into an Axum error response tuple.
pub fn to_error_response(e: NeboError) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(ErrorResponse {
            error: e.to_string(),
        }),
    )
}

/// Shorthand result type for handlers.
pub type HandlerResult<T> = Result<Json<T>, (StatusCode, Json<ErrorResponse>)>;
