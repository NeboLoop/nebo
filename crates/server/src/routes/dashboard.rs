use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// The workforce dashboard.
pub fn routes() -> Router<AppState> {
    Router::new().route("/dashboard", axum::routing::get(handlers::dashboard::dashboard))
}
