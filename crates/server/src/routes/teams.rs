use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Team routes. `/teams` is the contract; `/workrooms` is the old name and
/// stays as an alias of the same handlers (the `/teams` routes come first so
/// the generated client is emitted against them).
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/teams",
            axum::routing::get(handlers::teams::list_teams).post(handlers::teams::open_team),
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
