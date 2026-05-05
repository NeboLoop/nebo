use axum::Router;

use crate::handlers;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/plugins", axum::routing::get(handlers::plugins::list_plugins))
        .route("/plugins/events", axum::routing::get(handlers::plugins::list_all_plugin_events))
        .route("/plugins/{slug}", axum::routing::delete(handlers::plugins::remove_plugin))
        .route("/plugins/{slug}/events", axum::routing::get(handlers::plugins::list_plugin_events))
        .route("/plugins/{slug}/auth/login", axum::routing::post(handlers::plugins::auth_login))
        .route("/plugins/{slug}/auth/logout", axum::routing::post(handlers::plugins::auth_logout))
        .route("/plugins/{slug}/auth/status", axum::routing::get(handlers::plugins::auth_status))
        .route("/plugins/{slug}/config", axum::routing::get(handlers::plugins::get_plugin_config).put(handlers::plugins::set_plugin_config))
        .route("/plugins/{slug}/diagnostics", axum::routing::get(handlers::plugins::get_diagnostics))
        .route("/plugins/{slug}/api/{*path}", axum::routing::any(handlers::plugins::proxy_plugin_route))
}
