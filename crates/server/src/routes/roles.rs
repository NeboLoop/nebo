use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Role routes (CRUD, activation, workflows, stats).
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/roles", axum::routing::get(handlers::roles::list_roles))
        .route("/roles", axum::routing::post(handlers::roles::create_role))
        .route("/roles/{id}", axum::routing::get(handlers::roles::get_role))
        .route("/roles/{id}", axum::routing::put(handlers::roles::update_role))
        .route("/roles/{id}", axum::routing::delete(handlers::roles::delete_role))
        .route("/roles/{id}/toggle", axum::routing::post(handlers::roles::toggle_role))
        .route("/roles/{id}/install-deps", axum::routing::post(handlers::roles::install_deps))
        .route("/roles/active", axum::routing::get(handlers::roles::list_active_roles))
        .route("/roles/event-sources", axum::routing::get(handlers::roles::list_event_sources))
        .route("/roles/{id}/activate", axum::routing::post(handlers::roles::activate_role))
        .route("/roles/{id}/deactivate", axum::routing::post(handlers::roles::deactivate_role))
        .route("/roles/{id}/duplicate", axum::routing::post(handlers::roles::duplicate_role))
        .route("/roles/{id}/chat", axum::routing::post(handlers::roles::chat_with_role))
        .route("/roles/{id}/workflows", axum::routing::get(handlers::roles::list_role_workflows))
        .route("/roles/{id}/workflows", axum::routing::post(handlers::roles::create_role_workflow))
        .route("/roles/{id}/workflows/{binding_name}", axum::routing::put(handlers::roles::update_role_workflow))
        .route("/roles/{id}/workflows/{binding_name}", axum::routing::delete(handlers::roles::delete_role_workflow))
        .route("/roles/{id}/workflows/{binding_name}/toggle", axum::routing::post(handlers::roles::toggle_role_workflow))
        .route("/roles/{id}/inputs", axum::routing::put(handlers::roles::update_role_inputs))
        .route("/roles/{id}/setup", axum::routing::post(handlers::roles::trigger_role_setup))
        .route("/roles/{id}/reload", axum::routing::post(handlers::roles::reload_role))
        .route("/roles/{id}/check-update", axum::routing::post(handlers::roles::check_role_update))
        .route("/roles/{id}/apply-update", axum::routing::post(handlers::roles::apply_role_update))
        .route("/roles/{id}/stats", axum::routing::get(handlers::roles::role_stats))
        .route("/roles/{id}/runs", axum::routing::get(handlers::roles::list_role_runs))
}
