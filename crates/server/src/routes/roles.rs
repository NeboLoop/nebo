use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Agent routes (CRUD, activation, workflows, stats).
/// Note: URL paths kept as /roles for backwards compatibility with frontend.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/roles", axum::routing::get(handlers::agents::list_agents))
        .route("/roles", axum::routing::post(handlers::agents::create_agent))
        .route("/roles/{id}", axum::routing::get(handlers::agents::get_agent))
        .route("/roles/{id}", axum::routing::put(handlers::agents::update_agent))
        .route("/roles/{id}", axum::routing::delete(handlers::agents::delete_agent))
        .route("/roles/{id}/toggle", axum::routing::post(handlers::agents::toggle_agent))
        .route("/roles/{id}/install-deps", axum::routing::post(handlers::agents::install_deps))
        .route("/roles/active", axum::routing::get(handlers::agents::list_active_agents))
        .route("/roles/event-sources", axum::routing::get(handlers::agents::list_event_sources))
        .route("/roles/{id}/activate", axum::routing::post(handlers::agents::activate_agent))
        .route("/roles/{id}/deactivate", axum::routing::post(handlers::agents::deactivate_agent))
        .route("/roles/{id}/duplicate", axum::routing::post(handlers::agents::duplicate_agent))
        .route("/roles/{id}/chat", axum::routing::post(handlers::agents::chat_with_agent))
        .route("/roles/{id}/workflows", axum::routing::get(handlers::agents::list_agent_workflows))
        .route("/roles/{id}/workflows", axum::routing::post(handlers::agents::create_agent_workflow))
        .route("/roles/{id}/workflows/{binding_name}", axum::routing::put(handlers::agents::update_agent_workflow))
        .route("/roles/{id}/workflows/{binding_name}", axum::routing::delete(handlers::agents::delete_agent_workflow))
        .route("/roles/{id}/workflows/{binding_name}/toggle", axum::routing::post(handlers::agents::toggle_agent_workflow))
        .route("/roles/{id}/inputs", axum::routing::put(handlers::agents::update_agent_inputs))
        .route("/roles/{id}/setup", axum::routing::post(handlers::agents::trigger_agent_setup))
        .route("/roles/{id}/reload", axum::routing::post(handlers::agents::reload_agent))
        .route("/roles/{id}/check-update", axum::routing::post(handlers::agents::check_agent_update))
        .route("/roles/{id}/apply-update", axum::routing::post(handlers::agents::apply_agent_update))
        .route("/roles/{id}/stats", axum::routing::get(handlers::agents::agent_stats))
        .route("/roles/{id}/runs", axum::routing::get(handlers::agents::list_agent_runs))
}
