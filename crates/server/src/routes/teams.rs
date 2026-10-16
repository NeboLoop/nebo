use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Team routes. `/teams` is the contract; `/workrooms` is the old name and
/// stays as an alias of the same handlers (the `/teams` routes come first so
/// the generated client is emitted against them).
pub fn routes() -> Router<AppState> {
    Router::new()
        // Hand-offs between employees: the trace of who passed what to whom.
        .route("/handoffs", axum::routing::get(handlers::handoffs::list_handoffs))
        .route("/handoffs/{id}", axum::routing::get(handlers::handoffs::get_handoff))
        .route("/handoffs/{id}/stop", axum::routing::post(handlers::handoffs::stop_handoff))
        .route(
            "/teams",
            axum::routing::get(handlers::teams::list_teams).post(handlers::teams::open_team),
        )
        // Before `/teams/{teamId}` so the literal path is not swallowed by it.
        .route(
            "/teams/other-computers",
            axum::routing::get(handlers::teams::other_computers),
        )
        .route(
            "/teams/{teamId}",
            axum::routing::put(handlers::teams::edit_team)
                .delete(handlers::teams::remove_team),
        )
        .route(
            "/teams/{teamId}/messages",
            axum::routing::get(handlers::teams::get_team_messages)
                .post(handlers::teams::send_team_message),
        )
        .route(
            "/teams/{teamId}/stop",
            axum::routing::post(handlers::teams::stop_team_work),
        )
        .route(
            "/teams/{teamId}/working",
            axum::routing::get(handlers::teams::team_working),
        )
        .route(
            "/teams/{teamId}/read",
            axum::routing::put(handlers::teams::mark_team_read),
        )
        // A team's projects: each one piece of work its lead owns, with its
        // own thread (`team::post` with a project).
        .route(
            "/teams/{teamId}/projects",
            axum::routing::get(handlers::teams::list_projects).post(handlers::teams::create_project),
        )
        .route(
            "/projects/{id}/messages",
            axum::routing::get(handlers::teams::get_project_messages)
                .post(handlers::teams::send_project_message),
        )
        .route("/projects/{id}/read", axum::routing::put(handlers::teams::mark_project_read))
        .route("/projects/{id}/working", axum::routing::get(handlers::teams::project_working))
        .route("/projects/{id}/stop", axum::routing::post(handlers::teams::stop_project_work))
        // ── Aliases: the old workroom routes ──
        .route(
            "/workrooms",
            axum::routing::get(handlers::teams::list_teams).post(handlers::teams::open_team),
        )
        .route(
            "/workrooms/{teamId}",
            axum::routing::delete(handlers::teams::remove_team),
        )
        .route(
            "/workrooms/{teamId}/messages",
            axum::routing::get(handlers::teams::get_team_messages)
                .post(handlers::teams::send_team_message),
        )
        .route(
            "/workrooms/{teamId}/send",
            axum::routing::post(handlers::teams::send_team_message),
        )
}
