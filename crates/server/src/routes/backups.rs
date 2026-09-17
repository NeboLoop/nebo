use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// The snapshot ring.
pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/backups",
        axum::routing::get(handlers::backups::list_backups).post(handlers::backups::take_backup),
    )
}
