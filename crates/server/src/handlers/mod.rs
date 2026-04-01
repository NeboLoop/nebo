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
use axum::response::{IntoResponse, Json, Response};
use types::api::ErrorResponse;
use types::NeboError;

/// Newtype wrapper that implements `IntoResponse` for `NeboError`.
///
/// Due to the orphan rule, we cannot implement a foreign trait (`IntoResponse`)
/// on a foreign type (`NeboError`) in this crate. This wrapper bridges the gap.
///
/// Server errors (5xx) are logged automatically.
pub struct ApiError(pub NeboError);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.0.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        if status.is_server_error() {
            tracing::error!(status = %status, error = %self.0, "handler error");
        }
        (
            status,
            Json(ErrorResponse {
                error: self.0.to_string(),
            }),
        )
            .into_response()
    }
}

impl From<NeboError> for ApiError {
    fn from(e: NeboError) -> Self {
        ApiError(e)
    }
}

/// Convert a NeboError into an Axum error response tuple.
///
/// Existing handlers use this with `.map_err(to_error_response)`.
/// New handlers should prefer returning `ApiResult<T>` with `?` operator instead.
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

/// Shorthand result type used by existing handlers with `to_error_response()`.
pub type HandlerResult<T> = Result<Json<T>, (StatusCode, Json<ErrorResponse>)>;

/// Ergonomic result type for new handlers — `NeboError` converts to an HTTP
/// error response automatically via `ApiError`'s `IntoResponse` implementation.
///
/// Handlers returning this type can use the `?` operator on any `Result<_, NeboError>`
/// directly, without needing `.map_err(to_error_response)`.
pub type ApiResult<T> = Result<Json<T>, ApiError>;
