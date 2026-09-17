//! Backups — the snapshot ring over the ONE copy primitive (`db::backup`).
//! Listing is the ring's table; "back up now" takes a verified snapshot.

use axum::extract::State;
use axum::Json;
use serde::Serialize;

use crate::handlers::{to_error_response, HandlerResult};
use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupsResponse {
    pub backups: Vec<db::Backup>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupResponse {
    pub backup: db::Backup,
}

/// GET /backups — newest first, every copy that opens whole.
pub async fn list_backups(State(state): State<AppState>) -> HandlerResult<BackupsResponse> {
    let backups = state.store.list_backups().map_err(to_error_response)?;
    Ok(Json(BackupsResponse { backups }))
}

/// POST /backups — take a snapshot now. Verified before it is returned.
pub async fn take_backup(State(state): State<AppState>) -> HandlerResult<BackupResponse> {
    let store = state.store.clone();
    let backup = tokio::task::spawn_blocking(move || store.snapshot("manual"))
        .await
        .map_err(|e| to_error_response(types::NeboError::Internal(e.to_string())))?
        .map_err(to_error_response)?;
    Ok(Json(BackupResponse { backup }))
}
