use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// The owner's Permissions pages: one employee's, the company defaults, and
/// the activity with why each action was allowed.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/agents/{id}/permissions",
            axum::routing::get(handlers::permissions::get_agent_permissions),
        )
        .route(
            "/agents/{id}/permissions",
            axum::routing::put(handlers::permissions::update_agent_permissions),
        )
        .route(
            "/agents/{id}/permissions/items/{rule_id}",
            axum::routing::delete(handlers::permissions::remove_agent_permission),
        )
        .route(
            "/permissions/company",
            axum::routing::get(handlers::permissions::get_company_permissions),
        )
        .route(
            "/permissions/company",
            axum::routing::put(handlers::permissions::update_company_permissions),
        )
        .route(
            "/permissions/company/items/{rule_id}",
            axum::routing::delete(handlers::permissions::remove_company_permission),
        )
        .route(
            "/permissions/activity",
            axum::routing::get(handlers::permissions::list_permission_activity),
        )
}
