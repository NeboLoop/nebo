use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Public user routes (no JWT required — single-user local app).
pub fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/user/me/profile", axum::routing::get(handlers::user::get_profile))
        .route("/user/me/profile", axum::routing::put(handlers::user::update_profile))
        .route("/user/me/preferences", axum::routing::get(handlers::user::get_preferences))
        .route("/user/me/preferences", axum::routing::put(handlers::user::update_preferences))
        .route("/user/me/permissions", axum::routing::get(handlers::user::get_permissions))
        .route("/user/me/permissions", axum::routing::put(handlers::user::update_permissions))
        .route("/user/me/accept-terms", axum::routing::post(handlers::user::accept_terms))
}

/// Protected user routes (JWT required).
pub fn protected_routes() -> Router<AppState> {
    Router::new()
        .route("/user/me", axum::routing::get(handlers::user::get_current_user))
        .route("/user/me", axum::routing::put(handlers::user::update_current_user))
        .route("/user/me", axum::routing::delete(handlers::user::delete_account))
        .route("/user/me/change-password", axum::routing::post(handlers::user::change_password))
}
