//! The owner's phone reports its position for the employees the owner
//! shares it with (`agent::phone_location`). Reached from the phone through
//! the owner's tunnel; the reading is held in memory only and never logged.

use axum::extract::State;
use axum::response::Json;

use super::{HandlerResult, to_error_response};
use crate::state::AppState;

/// PUT /phone/location — the phone's latest reading and who may see it; an
/// empty recipient list revokes.
pub async fn update(
    State(state): State<AppState>,
    Json(reading): Json<agent::phone_location::PhoneReading>,
) -> HandlerResult<serde_json::Value> {
    state
        .harness
        .phone_locations()
        .update(reading, chrono::Utc::now().timestamp())
        .map_err(|e| to_error_response(types::NeboError::Validation(e.to_string())))?;
    Ok(Json(serde_json::json!({ "accepted": true })))
}
