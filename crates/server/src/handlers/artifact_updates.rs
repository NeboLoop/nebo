use axum::extract::{Path, State};
use axum::Json;

use db::models::ArtifactUpdateSettings;

use super::{HandlerResult, to_error_response};
use crate::state::AppState;

/// POST /artifacts/check-updates — trigger a background update check.
pub async fn check_updates(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let s = state.clone();
    tokio::spawn(async move {
        if let Err(e) = crate::artifact_updates::check_all(&s).await {
            tracing::warn!("manual artifact update check failed: {e}");
        }
    });
    Ok(Json(serde_json::json!({ "status": "checking" })))
}

/// GET /artifacts/updates — list pending updates.
pub async fn list_updates(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let updates = state
        .store
        .list_artifacts_with_updates()
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({ "updates": updates })))
}

/// POST /artifacts/:id/apply-update — apply a specific pending update.
pub async fn apply_update(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    // Find the artifact in pending updates
    let pending = state
        .store
        .list_artifacts_with_updates()
        .map_err(to_error_response)?;
    let artifact = pending
        .iter()
        .find(|a| a.artifact_id == id)
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?
        .clone();

    // Atomically claim to prevent double-apply
    let claimed = state
        .store
        .claim_artifact_update(&artifact.artifact_id, &artifact.artifact_type)
        .map_err(to_error_response)?;
    if !claimed {
        return Err(to_error_response(types::NeboError::Validation(
            "update already being applied".into(),
        )));
    }

    // Apply in background through the SINGLE shared apply core (same path the
    // auto-update loop uses) so behavior, history logging, and broadcasts match.
    let s = state.clone();
    let art = artifact.clone();
    tokio::spawn(async move {
        let api = match crate::codes::build_api_client(&s) {
            Ok(api) => api,
            Err(e) => {
                // Couldn't reach the marketplace — unclaim + record + broadcast.
                let _ = s.store.unclaim_artifact_update(&art.artifact_id, &art.artifact_type);
                let _ = s.store.record_artifact_update_history(
                    &art.artifact_id,
                    &art.artifact_type,
                    "",
                    &art.local_version,
                    &art.remote_version,
                    "failed",
                    &e.to_string(),
                );
                s.hub.broadcast(
                    "artifact_update_failed",
                    serde_json::json!({
                        "id": art.artifact_id,
                        "type": art.artifact_type,
                        "error": e.to_string(),
                    }),
                );
                return;
            }
        };
        crate::artifact_updates::apply_claimed_update(&s, &api, &art).await;
    });

    Ok(Json(serde_json::json!({ "status": "applying" })))
}

/// GET /artifacts/update-settings — read artifact update preferences.
pub async fn get_update_settings(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let settings = state
        .store
        .get_artifact_update_settings()
        .map_err(to_error_response)?;
    Ok(Json(serde_json::to_value(settings).unwrap_or_default()))
}

/// PUT /artifacts/update-settings — update artifact update preferences.
pub async fn set_update_settings(
    State(state): State<AppState>,
    Json(body): Json<ArtifactUpdateSettings>,
) -> HandlerResult<serde_json::Value> {
    state
        .store
        .set_artifact_update_settings(&body)
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// PUT /artifacts/:id/auto-update — toggle per-artifact auto-update.
pub async fn set_artifact_auto_update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let enabled = body["enabled"].as_bool().unwrap_or(true);
    // We need the artifact_type — check all types
    for artifact_type in &["agent", "skill", "plugin", "app"] {
        let _ = state.store.set_artifact_auto_update(&id, artifact_type, enabled);
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /artifacts/update-history — recent applied/failed upgrades.
pub async fn list_update_history(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let history = state
        .store
        .list_artifact_update_history(100)
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({ "history": history })))
}
