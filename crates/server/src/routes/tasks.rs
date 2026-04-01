use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Scheduled task routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/tasks", axum::routing::get(handlers::tasks::list_tasks))
        .route("/tasks", axum::routing::post(handlers::tasks::create_task))
        .route("/tasks/{name}", axum::routing::get(handlers::tasks::get_task))
        .route("/tasks/{name}", axum::routing::put(handlers::tasks::update_task))
        .route("/tasks/{name}", axum::routing::delete(handlers::tasks::delete_task))
        .route("/tasks/{name}/toggle", axum::routing::post(handlers::tasks::toggle_task))
        .route("/tasks/{name}/run", axum::routing::post(handlers::tasks::run_task))
        .route("/tasks/{name}/history", axum::routing::get(handlers::tasks::list_task_history))
}
