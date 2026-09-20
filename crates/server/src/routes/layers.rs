use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// The layers screen (Playbook PRD R15): the owner reads and writes the
/// industry, franchise and company packs, and says when the workforce learns
/// them. Every write here parks; `/layers/apply` is the owner saying now.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/layers", axum::routing::get(handlers::layers::list_layers))
        .route("/layers/seats", axum::routing::get(handlers::layers::list_layer_seats))
        .route("/layers/apply", axum::routing::post(handlers::layers::apply_layers))
        .route(
            "/layers/upload",
            axum::routing::post(handlers::layers::upload_layer_pack)
                // A real pack is a zip, and zips are not small: the same
                // ceiling the file door uses, for the same reason.
                .layer(axum::extract::DefaultBodyLimit::max(crate::routes::files::MAX_UPLOAD_BYTES)),
        )
        .route(
            "/layers/{slug}/files",
            axum::routing::get(handlers::layers::list_layer_files),
        )
        .route(
            "/layers/{slug}/file",
            axum::routing::get(handlers::layers::read_layer_file)
                .put(handlers::layers::write_layer_file)
                .delete(handlers::layers::delete_layer_file),
        )
}
