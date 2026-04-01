use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Agent routes (CRUD, activation, workflows, stats).
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/agents", axum::routing::get(handlers::agents::list_agents))
        .route("/agents", axum::routing::post(handlers::agents::create_agent))
        .route("/agents/{id}", axum::routing::get(handlers::agents::get_agent))
        .route("/agents/{id}", axum::routing::put(handlers::agents::update_agent))
        .route("/agents/{id}", axum::routing::delete(handlers::agents::delete_agent))
        .route("/agents/{id}/toggle", axum::routing::post(handlers::agents::toggle_agent))
        .route("/agents/{id}/install-deps", axum::routing::post(handlers::agents::install_deps))
        .route("/agents/active", axum::routing::get(handlers::agents::list_active_agents))
        .route("/agents/event-sources", axum::routing::get(handlers::agents::list_event_sources))
        .route("/agents/{id}/activate", axum::routing::post(handlers::agents::activate_agent))
        .route("/agents/{id}/deactivate", axum::routing::post(handlers::agents::deactivate_agent))
        .route("/agents/{id}/duplicate", axum::routing::post(handlers::agents::duplicate_agent))
        .route("/agents/{id}/chat", axum::routing::post(handlers::agents::chat_with_agent))
        .route("/agents/{id}/workflows", axum::routing::get(handlers::agents::list_agent_workflows))
        .route("/agents/{id}/workflows", axum::routing::post(handlers::agents::create_agent_workflow))
        .route("/agents/{id}/workflows/{binding_name}", axum::routing::put(handlers::agents::update_agent_workflow))
        .route("/agents/{id}/workflows/{binding_name}", axum::routing::delete(handlers::agents::delete_agent_workflow))
        .route("/agents/{id}/workflows/{binding_name}/toggle", axum::routing::post(handlers::agents::toggle_agent_workflow))
        .route("/agents/{id}/inputs", axum::routing::put(handlers::agents::update_agent_inputs))
        .route("/agents/{id}/setup", axum::routing::post(handlers::agents::trigger_agent_setup))
        .route("/agents/{id}/reload", axum::routing::post(handlers::agents::reload_agent))
        .route("/agents/{id}/check-update", axum::routing::post(handlers::agents::check_agent_update))
        .route("/agents/{id}/apply-update", axum::routing::post(handlers::agents::apply_agent_update))
        .route("/agents/{id}/stats", axum::routing::get(handlers::agents::agent_stats))
        .route("/agents/{id}/runs", axum::routing::get(handlers::agents::list_agent_runs))
}
