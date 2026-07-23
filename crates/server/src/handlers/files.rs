use axum::extract::{Multipart, Path, State};
use axum::response::Json;

use super::{HandlerResult, to_error_response};
use crate::state::AppState;

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

/// POST /api/v1/files/pick-folder — Open native folder dialog and return selected path
pub async fn pick_folder() -> HandlerResult<serde_json::Value> {
    let result = tokio::task::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("Select folder")
            .pick_folder()
    })
    .await
    .map_err(|e| to_error_response(types::NeboError::Internal(e.to_string())))?;

    let path = result.and_then(|p| p.to_str().map(|s| s.to_string()));

    Ok(Json(serde_json::json!({ "path": path })))
}

/// POST /api/v1/files/upload — Proxy file upload to NeboAI API
pub async fn upload_file(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> HandlerResult<serde_json::Value> {
    let api = crate::codes::build_api_client(&state).map_err(to_error_response)?;

    let mut filename = String::new();
    let mut mime_type = String::new();
    let mut data: Vec<u8> = Vec::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| to_error_response(types::NeboError::Validation(e.to_string())))?
    {
        if field.name() == Some("file") {
            filename = field
                .file_name()
                .unwrap_or("upload")
                .to_string();
            mime_type = field
                .content_type()
                .unwrap_or("application/octet-stream")
                .to_string();
            data = field
                .bytes()
                .await
                .map_err(|e| to_error_response(types::NeboError::Internal(e.to_string())))?
                .to_vec();
        }
    }

    if data.is_empty() {
        return Err(to_error_response(types::NeboError::Validation(
            "no file provided".into(),
        )));
    }

    let attachment = api
        .upload_file(&filename, &mime_type, data)
        .await
        .map_err(|e| to_error_response(types::NeboError::Internal(e.to_string())))?;

    Ok(Json(serde_json::to_value(attachment).unwrap_or_default()))
}

/// GET /api/v1/comm-files/{id} — stream a loop attachment through the bot's
/// own credentials. The loop's `/api/v1/files/{id}` is auth-gated, and an
/// `<img>`/`<video>`/`<audio>` tag can't attach a bearer token — so uploaded
/// media in chat renders through this proxy (desktop and tunnel alike).
/// `?mime=` sets the response type but only media prefixes are honored —
/// anything else (e.g. text/html) is forced to octet-stream so a crafted
/// query can't turn the proxy into an XSS vector.
pub async fn serve_comm_file(
    State(state): State<AppState>,
    Path(file_id): Path<String>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<axum::response::Response, (axum::http::StatusCode, Json<types::api::ErrorResponse>)> {
    // The id is interpolated into the loop URL — restrict to uuid characters
    // so it can't traverse into other loop endpoints.
    if file_id.is_empty()
        || !file_id
            .chars()
            .all(|c| c.is_ascii_hexdigit() || c == '-')
    {
        return Err(to_error_response(types::NeboError::Validation(
            "invalid file id".into(),
        )));
    }
    let api = crate::codes::build_api_client(&state).map_err(to_error_response)?;
    let bytes = api
        .download_file(&file_id)
        .await
        .map_err(|e| to_error_response(types::NeboError::Internal(e.to_string())))?;

    let mime = params
        .get("mime")
        .filter(|m| {
            // svg is script-capable — never serve it inline from this origin.
            !m.starts_with("image/svg")
                && (m.starts_with("image/") || m.starts_with("video/") || m.starts_with("audio/"))
        })
        .cloned()
        .unwrap_or_else(|| "application/octet-stream".to_string());

    Ok(axum::response::Response::builder()
        .header("Content-Type", mime)
        .header("Cache-Control", "private, max-age=3600")
        .body(axum::body::Body::from(bytes))
        .unwrap_or_default())
}

/// GET /api/v1/files/*path
///
/// `?preview=pdf` on a presentation file serves an on-demand PDF rendering
/// (generated via the nebo-office plugin, cached next to the source) so the
/// Work panel can show decks through its existing PDF viewer.
pub async fn serve_file(
    State(state): State<AppState>,
    Path(file_path): Path<String>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<axum::response::Response, (axum::http::StatusCode, Json<types::api::ErrorResponse>)> {
    let data_dir = config::data_dir().map_err(to_error_response)?;
    let files_root = data_dir.join("files");
    let full_path = files_root.join(&file_path);

    if !full_path.exists() {
        return Err(to_error_response(types::NeboError::NotFound));
    }

    // Path-traversal guard: resolve `..` and symlinks, then confirm the target is still
    // inside the files/ sandbox. Without this, `GET /api/v1/files/../../<anything>` (or a
    // symlink) escapes the root and serves arbitrary files. 404 (not 403) to avoid leaking
    // which paths exist.
    let canonical = tokio::fs::canonicalize(&full_path)
        .await
        .map_err(|_| to_error_response(types::NeboError::NotFound))?;
    let canonical_root = tokio::fs::canonicalize(&files_root)
        .await
        .map_err(|_| to_error_response(types::NeboError::NotFound))?;
    if !canonical.starts_with(&canonical_root) {
        return Err(to_error_response(types::NeboError::NotFound));
    }

    if params.get("preview").map(String::as_str) == Some("pdf")
        && matches!(
            canonical.extension().and_then(|e| e.to_str()),
            Some("pptx" | "ppt")
        )
    {
        return serve_pptx_preview(&state, &canonical, &canonical_root).await;
    }

    let bytes = tokio::fs::read(&canonical)
        .await
        .map_err(|e| to_error_response(types::NeboError::Io(e)))?;

    // Guess content type from extension. Text types carry charset=utf-8:
    // agent-written files are UTF-8, and without an explicit charset the
    // browser falls back to Windows-1252 — em-dashes and emoji render as
    // mojibake (â€", ðŸ"š) in the Work panel iframe.
    let content_type = match canonical.extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("webp") => "image/webp",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mov") => "video/quicktime",
        Some("pdf") => "application/pdf",
        Some("json") => "application/json; charset=utf-8",
        Some("txt") | Some("log") | Some("md") | Some("markdown") | Some("csv") | Some("typ") => {
            "text/plain; charset=utf-8"
        }
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "application/javascript; charset=utf-8",
        _ => "application/octet-stream",
    };

    axum::response::Response::builder()
        .header("content-type", content_type)
        .body(axum::body::Body::from(bytes))
        .map_err(|e| to_error_response(types::NeboError::Internal(e.to_string())))
}

/// Render a .pptx to a cached PDF via the nebo-office plugin and return the
/// cache path. Results cache under `files/.previews/<name>.pdf` and
/// regenerate when the source is newer. The ONE conversion implementation —
/// used by the preview endpoint and by outbound comm artifact uploads.
pub(crate) async fn ensure_pptx_preview(
    plugin_store: &napp::plugin::PluginStore,
    source: &std::path::Path,
    files_root: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    let name = source
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "invalid source file name".to_string())?;
    let previews_dir = files_root.join(".previews");
    let cache = previews_dir.join(format!("{name}.pdf"));

    let src_mtime = tokio::fs::metadata(source)
        .await
        .and_then(|m| m.modified())
        .map_err(|e| format!("stat source: {e}"))?;
    let cache_fresh = match tokio::fs::metadata(&cache).await.and_then(|m| m.modified()) {
        Ok(t) => t >= src_mtime,
        Err(_) => false,
    };

    if !cache_fresh {
        let Some(bin) = plugin_store.resolve("nebo-office", "*") else {
            return Err("the nebo-office plugin is not installed".into());
        };
        tokio::fs::create_dir_all(&previews_dir)
            .await
            .map_err(|e| format!("create previews dir: {e}"))?;
        let output = tokio::process::Command::new(&bin)
            .arg("pdf")
            .arg("convert")
            .arg(source)
            .arg("-o")
            .arg(&cache)
            .arg("--bin")
            .arg(&bin)
            .output()
            .await
            .map_err(|e| format!("run nebo-office: {e}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("conversion failed: {}", stderr.trim()));
        }
    }
    Ok(cache)
}

/// Serve the on-demand pptx→PDF preview. 503 when the plugin is missing or
/// conversion fails — the viewer falls back to its download card.
async fn serve_pptx_preview(
    state: &AppState,
    source: &std::path::Path,
    files_root: &std::path::Path,
) -> Result<axum::response::Response, (axum::http::StatusCode, Json<types::api::ErrorResponse>)> {
    let cache = ensure_pptx_preview(&state.plugin_store, source, files_root)
        .await
        .map_err(|e| {
            (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                Json(types::api::ErrorResponse {
                    error: format!("preview unavailable: {e}"),
                }),
            )
        })?;

    let bytes = tokio::fs::read(&cache)
        .await
        .map_err(|e| to_error_response(types::NeboError::Io(e)))?;
    axum::response::Response::builder()
        .header("content-type", "application/pdf")
        .body(axum::body::Body::from(bytes))
        .map_err(|e| to_error_response(types::NeboError::Internal(e.to_string())))
}
