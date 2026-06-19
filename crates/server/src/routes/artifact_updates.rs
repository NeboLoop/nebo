use axum::Router;

use crate::handlers;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/artifacts/check-updates",
            axum::routing::post(handlers::artifact_updates::check_updates),
        )
        .route(
            "/artifacts/updates",
            axum::routing::get(handlers::artifact_updates::list_updates),
        )
        .route(
            "/artifacts/{id}/apply-update",
            axum::routing::post(handlers::artifact_updates::apply_update),
        )
        .route(
            "/artifacts/{id}/auto-update",
            axum::routing::put(handlers::artifact_updates::set_artifact_auto_update),
        )
        .route(
            "/artifacts/update-settings",
            axum::routing::get(handlers::artifact_updates::get_update_settings)
                .put(handlers::artifact_updates::set_update_settings),
        )
        .route(
            "/artifacts/update-history",
            axum::routing::get(handlers::artifact_updates::list_update_history),
        )
}
