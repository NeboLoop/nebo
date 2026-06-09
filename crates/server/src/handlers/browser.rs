use axum::extract::State;
use axum::response::Json;

use super::HandlerResult;
use crate::state::AppState;

/// GET /api/v1/browser/status — backend state for the Settings → Browser panel.
///
/// Reports whether the Chrome/Brave extension is connected and whether the
/// built-in browser (Obscura tier-2) is available as a fallback. The panel uses
/// this to show connection state and an Install CTA.
pub async fn browser_status(State(state): State<AppState>) -> HandlerResult<serde_json::Value> {
    let extension_connected = state.extension_bridge.is_connected();
    let data_dir = config::data_dir()
        .map(|d| d.to_string_lossy().to_string())
        .unwrap_or_default();
    let built_in_available = browser::cdp_bridge::find_obscura(&data_dir).is_some();

    Ok(Json(serde_json::json!({
        "extensionConnected": extension_connected,
        "builtInAvailable": built_in_available,
    })))
}
