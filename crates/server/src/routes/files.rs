use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// File serving and picker routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/files/browse", axum::routing::post(handlers::files::browse))
        .route("/files/pick", axum::routing::post(handlers::files::pick_files))
        .route("/files/pick-folder", axum::routing::post(handlers::files::pick_folder))
        .route("/files/{*path}", axum::routing::get(handlers::files::serve_file))
}
