use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Migration importer routes (detect / dry-run scan / apply).
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/import/detect",
            axum::routing::get(handlers::import::detect_installs),
        )
        .route(
            "/import/scan",
            axum::routing::post(handlers::import::scan_install),
        )
        .route(
            "/import/apply",
            axum::routing::post(handlers::import::apply_install),
        )
}
