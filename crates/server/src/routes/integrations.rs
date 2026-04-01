use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Integration (MCP) routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/integrations", axum::routing::get(handlers::integrations::list_integrations))
        .route("/integrations", axum::routing::post(handlers::integrations::create_integration))
        .route("/integrations/registry", axum::routing::get(handlers::integrations::list_registry))
        .route("/mcp/servers", axum::routing::get(handlers::integrations::list_registry))
        .route("/integrations/tools", axum::routing::get(handlers::integrations::list_tools))
        .route("/integrations/{id}", axum::routing::get(handlers::integrations::get_integration))
        .route("/integrations/{id}", axum::routing::put(handlers::integrations::update_integration))
        .route("/integrations/{id}", axum::routing::delete(handlers::integrations::delete_integration))
        .route("/integrations/{id}/test", axum::routing::post(handlers::integrations::test_integration))
        .route("/integrations/{id}/connect", axum::routing::post(handlers::integrations::connect_integration))
        .route("/integrations/{id}/oauth-url", axum::routing::get(handlers::integrations::get_oauth_url))
        .route("/integrations/oauth/callback", axum::routing::get(handlers::integrations::oauth_callback))
}
