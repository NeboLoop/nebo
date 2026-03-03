use std::sync::Arc;

use config::Config;
use db::Store;
use auth::AuthService;

/// Shared application state passed to all handlers via Axum extractors.
#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub store: Arc<Store>,
    pub auth: Arc<AuthService>,
}
