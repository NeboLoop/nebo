use axum::extract::DefaultBodyLimit;
use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// What one upload may weigh, matching the edge (`edgelb/main.go`
/// MaxRequestBodySize) so a file the edge accepts is not refused here.
///
/// Without this, axum's own 2 MiB default applied: a photo or an hour of
/// audio came back as `500 Error parsing 'multipart/form-data' request`,
/// which reads like a malformed request rather than a file that is too big.
pub const MAX_UPLOAD_BYTES: usize = 100 * 1024 * 1024;

/// File serving and picker routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/files/browse",
            axum::routing::post(handlers::files::browse),
        )
        .route(
            "/files/pick",
            axum::routing::post(handlers::files::pick_files),
        )
        .route(
            "/files/pick-folder",
            axum::routing::post(handlers::files::pick_folder),
        )
        .route(
            "/files/upload",
            axum::routing::post(handlers::files::upload_file)
                .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)),
        )
        .route(
            "/files/{*path}",
            axum::routing::get(handlers::files::serve_file),
        )
        .route(
            "/work/documents",
            axum::routing::get(handlers::files::list_work_documents),
        )
        .route(
            "/comm-files/{id}",
            axum::routing::get(handlers::files::serve_comm_file),
        )
}
