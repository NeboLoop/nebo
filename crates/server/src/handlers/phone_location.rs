use super::{HandlerResult, to_error_response};
use axum::response::Json;

// Like the other local bot endpoints, remote access is authenticated by the
// owner-scoped hub tunnel. The payload is never logged or written to disk.
pub async fn update(
    Json(body): Json<agent::phone_location::PhoneLocation>,
) -> HandlerResult<serde_json::Value> {
    agent::phone_location::update(body)
        .map_err(|e| to_error_response(types::NeboError::Validation(e.into())))?;
    Ok(Json(serde_json::json!({"accepted": true})))
}
