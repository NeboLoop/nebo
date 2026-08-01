//! Migration importer endpoints: detect installs, dry-run scan, apply.
//!
//! Serves both surfaces that trigger a migration — the Settings → Import page
//! and the onboarding "we found your setup" card — so the two can't drift.

use std::path::PathBuf;

use axum::Json;
use axum::extract::State;

use crate::state::AppState;

use super::{HandlerResult, to_error_response};

/// One probed install location.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DetectedInstall {
    source: &'static str,
    path: String,
    /// False while a system's apply path hasn't shipped (OpenClaw), so the UI
    /// can show it as "coming soon" instead of offering a broken import.
    importable: bool,
}

/// Default install locations per system, honoring each tool's env overrides.
fn candidate_roots() -> Vec<(crate::import::SourceKind, PathBuf)> {
    use crate::import::SourceKind;
    let mut v = Vec::new();
    if let Ok(h) = std::env::var("HERMES_HOME") {
        v.push((SourceKind::Hermes, PathBuf::from(h)));
    } else if let Some(home) = dirs::home_dir() {
        v.push((SourceKind::Hermes, home.join(".hermes")));
    }
    let openclaw = std::env::var("OPENCLAW_HOME")
        .or_else(|_| std::env::var("OPENCLAW_STATE_DIR"))
        .map(PathBuf::from)
        .ok()
        .or_else(|| dirs::home_dir().map(|h| h.join(".openclaw")));
    if let Some(p) = openclaw {
        v.push((SourceKind::OpenClaw, p));
    }
    v
}

/// GET /api/v1/import/detect — probe the default install locations and report
/// what's actually there (fingerprinted, not just "directory exists").
pub async fn detect_installs(State(_state): State<AppState>) -> HandlerResult<serde_json::Value> {
    let mut found = Vec::new();
    for (expected, path) in candidate_roots() {
        if crate::import::detect(&path) == Some(expected) {
            found.push(DetectedInstall {
                source: match expected {
                    crate::import::SourceKind::Hermes => "hermes",
                    crate::import::SourceKind::OpenClaw => "openclaw",
                },
                path: path.display().to_string(),
                importable: matches!(expected, crate::import::SourceKind::Hermes),
            });
        }
    }
    Ok(Json(serde_json::json!({ "installs": found })))
}

/// Pull and validate the `path` field shared by scan and apply.
fn body_path(body: &serde_json::Value) -> Result<PathBuf, types::NeboError> {
    let path = body["path"]
        .as_str()
        .ok_or_else(|| types::NeboError::Validation("path required".into()))?;
    let path = PathBuf::from(path);
    if !path.is_dir() {
        return Err(types::NeboError::Validation(format!(
            "{} is not a directory",
            path.display()
        )));
    }
    Ok(path)
}

/// POST /api/v1/import/scan — dry-run. Walks the install read-only and returns
/// the manifest of what an import would do. Never writes anything.
pub async fn scan_install(
    State(_state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let path = body_path(&body).map_err(to_error_response)?;
    let manifest = crate::import::scan(&path).map_err(to_error_response)?;
    let needs_confirmation = manifest.needs_confirmation();
    Ok(Json(serde_json::json!({
        "manifest": manifest,
        "needsConfirmation": needs_confirmation,
    })))
}

/// POST /api/v1/import/apply — perform the import. The frontend calls this
/// only after the user approved the scanned manifest.
pub async fn apply_install(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let path = body_path(&body).map_err(to_error_response)?;
    let outcome = crate::import::apply(&state, &path)
        .await
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({ "outcome": outcome })))
}
