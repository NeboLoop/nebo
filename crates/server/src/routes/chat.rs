use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Chat and message routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/chats", axum::routing::get(handlers::chat::list_chats))
        .route("/chats", axum::routing::post(handlers::chat::create_chat))
        .route("/chats/days", axum::routing::get(handlers::chat::list_chat_days))
        .route("/chats/history/{day}", axum::routing::get(handlers::chat::get_chat_history_by_day))
        .route("/chats/companion", axum::routing::get(handlers::chat::get_companion_chat))
        .route("/chats/companion/new", axum::routing::post(handlers::chat::create_companion_chat))
        .route("/chats/search", axum::routing::get(handlers::chat::search_messages))
        .route("/chats/message", axum::routing::post(handlers::chat::send_message))
        .route("/chats/{id}", axum::routing::get(handlers::chat::get_chat))
        .route("/chats/{id}", axum::routing::put(handlers::chat::update_chat))
        .route("/chats/{id}", axum::routing::delete(handlers::chat::delete_chat))
        .route("/chats/{id}/messages", axum::routing::get(handlers::chat::get_chat_messages))
        .route("/chats/{chat_id}/tool-output/{tool_call_id}", axum::routing::get(handlers::chat::get_tool_output))
        .route("/chats/messages/{id}/edit", axum::routing::post(handlers::chat::edit_message))
}
