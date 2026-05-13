use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Voice pipeline routes (TTS and transcription).
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/voice/tts", axum::routing::post(handlers::voice::tts))
        .route(
            "/voice/transcribe",
            axum::routing::post(handlers::voice::transcribe),
        )
        .route("/voice/status", axum::routing::get(handlers::voice::status))
}
