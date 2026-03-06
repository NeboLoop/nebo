use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

use crate::VERSION;

#[derive(Embed)]
#[folder = "../../app/build/"]
struct Frontend;

/// Serve SPA assets with fallback to index.html for client-side routing.
pub async fn spa_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    // Try exact file first
    if let Some(file) = Frontend::get(path) {
        return file_response(path, file);
    }

    // Fallback to index.html for SPA routing
    match Frontend::get("index.html") {
        Some(file) => file_response("index.html", file),
        None => (StatusCode::NOT_FOUND, "frontend not built").into_response(),
    }
}

/// GET /server.json — dynamic server info for the frontend.
pub async fn server_json() -> impl IntoResponse {
    axum::response::Json(serde_json::json!({
        "host": "localhost",
        "port": std::env::var("PORT").unwrap_or_else(|_| "27895".into()),
        "version": VERSION,
    }))
}

fn file_response(path: &str, file: rust_embed::EmbeddedFile) -> Response {
    let mime = mime_from_path(path);
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, cache_control(path).into()),
        ],
        file.data,
    )
        .into_response()
}

fn mime_from_path(path: &str) -> String {
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "webmanifest" => "application/manifest+json",
        "webp" => "image/webp",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "wasm" => "application/wasm",
        "map" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        "xml" => "application/xml",
        _ => "application/octet-stream",
    }
    .to_string()
}

fn cache_control(path: &str) -> &'static str {
    // Immutable hashed assets get long cache
    if path.starts_with("_app/immutable/") {
        "public, max-age=31536000, immutable"
    } else if path == "index.html" || path == "200.html" {
        "no-cache"
    } else {
        "public, max-age=3600"
    }
}
