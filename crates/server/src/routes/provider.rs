use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Provider and model routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/providers", axum::routing::get(handlers::provider::list_providers))
        .route("/providers", axum::routing::post(handlers::provider::create_provider))
        .route("/providers/{id}", axum::routing::get(handlers::provider::get_provider))
        .route("/providers/{id}", axum::routing::put(handlers::provider::update_provider))
        .route("/providers/{id}", axum::routing::delete(handlers::provider::delete_provider))
        .route("/providers/{id}/test", axum::routing::post(handlers::provider::test_provider))
        .route("/models", axum::routing::get(handlers::provider::list_models))
        .route("/models/config", axum::routing::put(handlers::provider::update_model_config))
        .route("/models/task-routing", axum::routing::put(handlers::provider::update_task_routing))
        .route("/models/cli/{cliId}", axum::routing::put(handlers::provider::update_cli_provider))
        .route("/models/{provider}/{modelId}", axum::routing::put(handlers::provider::update_model))
        .route("/local-models/status", axum::routing::get(handlers::provider::local_models_status))
}
