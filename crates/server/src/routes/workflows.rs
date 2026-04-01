use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Workflow routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/workflows", axum::routing::get(handlers::workflows::list_workflows))
        .route("/workflows", axum::routing::post(handlers::workflows::create_workflow))
        .route("/workflows/{id}", axum::routing::get(handlers::workflows::get_workflow))
        .route("/workflows/{id}", axum::routing::put(handlers::workflows::update_workflow))
        .route("/workflows/{id}", axum::routing::delete(handlers::workflows::delete_workflow))
        .route("/workflows/{id}/toggle", axum::routing::post(handlers::workflows::toggle_workflow))
        .route("/workflows/{id}/run", axum::routing::post(handlers::workflows::run_workflow))
        .route("/workflows/{id}/runs", axum::routing::get(handlers::workflows::list_runs))
        .route("/workflows/{id}/runs/{runId}", axum::routing::get(handlers::workflows::get_run))
        .route("/workflows/{id}/runs/{runId}/cancel", axum::routing::post(handlers::workflows::cancel_run))
        .route("/workflows/{id}/bindings", axum::routing::get(handlers::workflows::list_bindings))
        .route("/workflows/{id}/bindings", axum::routing::put(handlers::workflows::update_bindings))
}
