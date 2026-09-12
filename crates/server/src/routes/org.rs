use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Org routes: install an org folder onto this Nebo (R12).
pub fn routes() -> Router<AppState> {
    Router::new().route("/org/install", axum::routing::post(handlers::org::install_org))
}
