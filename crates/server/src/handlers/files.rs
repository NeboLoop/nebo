use axum::extract::{Path, State};
use axum::response::Json;

use crate::state::AppState;
use super::{to_error_response, HandlerResult};

/// POST /api/v1/files/browse
pub async fn browse(
    State(_state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let path = body["path"].as_str().unwrap_or("~");

    let expanded = shellexpand::tilde(path).to_string();
    let dir = std::path::Path::new(&expanded);

    if !dir.exists() {
        return Err(to_error_response(types::NeboError::NotFound));
    }

    if !dir.is_dir() {
        return Err(to_error_response(types::NeboError::Validation(
            "path is not a directory".into(),
        )));
    }

    let mut entries = Vec::new();
    if let Ok(read_dir) = std::fs::read_dir(dir) {
        for entry in read_dir.flatten() {
            let metadata = entry.metadata().ok();
            let is_dir = metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);
            let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);

            entries.push(serde_json::json!({
                "name": entry.file_name().to_string_lossy(),
                "path": entry.path().to_string_lossy(),
                "isDir": is_dir,
                "size": size,
            }));
        }
    }

    // Sort: directories first, then alphabetical
    entries.sort_by(|a, b| {
        let a_dir = a["isDir"].as_bool().unwrap_or(false);
        let b_dir = b["isDir"].as_bool().unwrap_or(false);
        match (a_dir, b_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => {
                let a_name = a["name"].as_str().unwrap_or("");
                let b_name = b["name"].as_str().unwrap_or("");
                a_name.to_lowercase().cmp(&b_name.to_lowercase())
            }
        }
    });

    Ok(Json(serde_json::json!({
        "path": expanded,
        "entries": entries,
    })))
}

/// POST /api/v1/files/pick — Open native file dialog and return selected paths
pub async fn pick_files() -> HandlerResult<serde_json::Value> {
    let result = tokio::task::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("Select files")
            .pick_files()
    })
    .await
    .map_err(|e| to_error_response(types::NeboError::Internal(e.to_string())))?;

    let paths: Vec<String> = result
        .unwrap_or_default()
        .iter()
        .filter_map(|p| p.to_str())
        .map(|s| s.to_string())
        .collect();

    Ok(Json(serde_json::json!({ "paths": paths })))
}

/// GET /api/v1/files/*path
pub async fn serve_file(
    State(_state): State<AppState>,
    Path(file_path): Path<String>,
) -> Result<axum::response::Response, (axum::http::StatusCode, Json<types::api::ErrorResponse>)> {
    let data_dir = config::data_dir().map_err(to_error_response)?;
    let full_path = data_dir.join("files").join(&file_path);

    if !full_path.exists() {
        return Err(to_error_response(types::NeboError::NotFound));
    }

    let bytes = tokio::fs::read(&full_path)
        .await
        .map_err(|e| to_error_response(types::NeboError::Io(e)))?;

    // Guess content type from extension
    let content_type = match full_path.extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("pdf") => "application/pdf",
        Some("json") => "application/json",
        Some("txt") | Some("log") => "text/plain",
        Some("html") => "text/html",
        Some("css") => "text/css",
        Some("js") => "application/javascript",
        _ => "application/octet-stream",
    };

    axum::response::Response::builder()
        .header("content-type", content_type)
        .body(axum::body::Body::from(bytes))
        .map_err(|e| to_error_response(types::NeboError::Internal(e.to_string())))
}
