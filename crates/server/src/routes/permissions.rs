use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Permission routes: the asks waiting on the owner and their answers; the
/// owner's Permissions pages (one employee's, the company defaults) and the
/// activity with why each action was allowed.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/permissions/asks", axum::routing::get(handlers::permissions::list_permission_asks))
        .route("/permissions/asks/{id}", axum::routing::get(handlers::permissions::get_permission_ask))
        .route(
            "/permissions/asks/{id}/answer",
            axum::routing::post(handlers::permissions::answer_permission_ask),
        )
        // The shipped phone's approvals (see the handlers).
        .route(
            "/agents/workflow-runs/{run_id}/approval",
            axum::routing::get(handlers::permissions::get_workflow_run_approval),
        )
        .route(
            "/agents/workflow-runs/{run_id}/approval",
            axum::routing::post(handlers::permissions::answer_workflow_run_approval),
        )
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
