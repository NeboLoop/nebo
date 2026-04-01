use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Agent routes (sessions, settings, profile, status, advisors, channels, WS).
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/agent/sessions", axum::routing::get(handlers::agent::list_sessions))
        .route("/agent/sessions/{id}", axum::routing::delete(handlers::agent::delete_session))
        .route("/agent/sessions/{id}/messages", axum::routing::get(handlers::agent::get_session_messages))
        .route("/agent/settings", axum::routing::get(handlers::agent::get_settings))
        .route("/agent/settings", axum::routing::put(handlers::agent::update_settings))
        .route("/agent/profile", axum::routing::get(handlers::agent::get_profile))
        .route("/agent/profile", axum::routing::put(handlers::agent::update_profile))
        .route("/agent/status", axum::routing::get(handlers::agent::get_status))
        .route("/agent/system-info", axum::routing::get(handlers::agent::get_system_info))
        .route("/agent/personality-presets", axum::routing::get(handlers::agent::list_personality_presets))
        .route("/agent/heartbeat", axum::routing::get(handlers::agent::get_heartbeat))
        .route("/agent/heartbeat", axum::routing::put(handlers::agent::update_heartbeat))
        .route("/agent/lanes", axum::routing::get(handlers::agent::get_lanes))
        .route("/agent/advisors", axum::routing::get(handlers::agent::list_advisors))
        .route("/agent/advisors", axum::routing::post(handlers::agent::create_advisor))
        .route("/agent/advisors/{name}", axum::routing::get(handlers::agent::get_advisor))
        .route("/agent/advisors/{name}", axum::routing::put(handlers::agent::update_advisor))
        .route("/agent/advisors/{name}", axum::routing::delete(handlers::agent::delete_advisor))
        .route("/agent/channels/{channelId}/messages", axum::routing::get(handlers::agent::get_channel_messages))
        .route("/agent/channels/{channelId}/send", axum::routing::post(handlers::agent::send_channel_message))
        .route("/agent/ws", axum::routing::get(handlers::ws::agent_ws_handler))
}
