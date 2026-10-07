use axum::extract::{Multipart, Path, State};
use axum::response::{IntoResponse, Json};

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
    // Android has no native desktop file dialog (and no rfd backend). The web UI
    // falls back to its own <input type="file"> upload path when picking fails.
    #[cfg(target_os = "android")]
    {
        let paths: Vec<String> = Vec::new();
        return Ok(Json(serde_json::json!({ "paths": paths })));
    }

    #[cfg(not(target_os = "android"))]
    {
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
}

/// POST /api/v1/files/pick-folder — Open native folder dialog and return selected path
pub async fn pick_folder() -> HandlerResult<serde_json::Value> {
    // See pick_files — no dialog backend on Android.
    #[cfg(target_os = "android")]
    {
        let path: Option<String> = None;
        return Ok(Json(serde_json::json!({ "path": path })));
    }

    #[cfg(not(target_os = "android"))]
    {
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
}

/// POST /api/v1/files/upload — store an attachment.
///
/// The bytes are written to this machine first and only then offered to the
/// loop. Attaching a file is a local act: it has to keep working while signed
/// out, and a file the user can see in the composer must not be lost because
/// an upload failed. The loop copy is what makes an attachment shareable with
/// other bots, so it is still attempted — just never as the only copy.
/// A multipart failure says which of the two things went wrong. Axum answers a
/// body over the limit with the same opaque "Error parsing `multipart/form-data`
/// request" it uses for a malformed one, so a file that is merely too big reads
/// as a broken request; `MultipartError::status()` tells them apart.
pub(crate) fn too_big_or_bad(
    max_upload_bytes: usize,
    e: axum::extract::multipart::MultipartError,
) -> (axum::http::StatusCode, axum::Json<crate::handlers::ErrorResponse>) {
    if e.status() == axum::http::StatusCode::PAYLOAD_TOO_LARGE {
        return to_error_response(types::NeboError::Validation(format!(
            "That file is larger than {} MB, which is the most one upload may carry.",
            max_upload_bytes / (1024 * 1024)
        )));
    }
    to_error_response(types::NeboError::Validation(e.to_string()))
}

/// A hub refusal an app must hear as the hub said it, or `None` for one the
/// bot rides out on its own. Only a full account is passed on (413, code
/// `storage_full`): its body is the hub's own answer, so the apps show its
/// sentence and know not to offer the same upload again.
pub(crate) fn hub_upload_refusal(e: &comm::CommError) -> Option<axum::response::Response> {
    let comm::CommError::Http { status: 413, body } = e else {
        return None;
    };
    let parsed: serde_json::Value = serde_json::from_str(body).ok()?;
    if parsed.get("code").and_then(|c| c.as_str()) != Some("storage_full") {
        return None;
    }
    Some((axum::http::StatusCode::PAYLOAD_TOO_LARGE, Json(parsed)).into_response())
}

/// What a hub refusal of a file read becomes here. A file the hub removed
/// after its keeping period (410) is passed on with the hub's sentence, so
/// the apps can say so instead of offering a retry; anything else stays an
/// internal error.
pub(crate) fn hub_file_error(
    e: comm::CommError,
) -> (axum::http::StatusCode, Json<types::api::ErrorResponse>) {
    if let comm::CommError::Http { status: 410, body } = &e {
        let error = serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|v| v.get("error").and_then(|m| m.as_str()).map(str::to_string))
            .unwrap_or_else(|| "This file was removed.".to_string());
        return (
            axum::http::StatusCode::GONE,
            Json(types::api::ErrorResponse { error }),
        );
    }
    to_error_response(types::NeboError::Internal(e.to_string()))
}

pub async fn upload_file(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<axum::response::Response, (axum::http::StatusCode, Json<types::api::ErrorResponse>)> {
    let mut filename = String::new();
    let mut mime_type = String::new();
    let mut data: Vec<u8> = Vec::new();
    // Where the file landed. Optional: a client that has no conversation
    // behind the upload sends neither, and the arrival is announced without
    // them rather than not at all.
    let mut agent_id = String::new();
    let mut chat_id = String::new();

    // The same ceiling the door was built with, so the sentence names the
    // number that actually refused the file.
    let max_upload_bytes = state.config.runtime.max_upload_bytes();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| too_big_or_bad(max_upload_bytes, e))?
    {
        match field.name().unwrap_or_default().to_string().as_str() {
            "file" => {
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
                    .map_err(|e| too_big_or_bad(max_upload_bytes, e))?
                    .to_vec();
            }
            "agentId" => {
                agent_id = field
                    .text()
                    .await
                    .map_err(|e| too_big_or_bad(max_upload_bytes, e))?
            }
            "chatId" => {
                chat_id = field
                    .text()
                    .await
                    .map_err(|e| too_big_or_bad(max_upload_bytes, e))?
            }
            _ => {}
        }
    }

    if data.is_empty() {
        return Err(to_error_response(types::NeboError::Validation(
            "no file provided".into(),
        )));
    }

    let size = data.len() as u64;
    let file_id = uuid::Uuid::new_v4().to_string();

    let dir = agent::uploads::dir().ok_or_else(|| {
        to_error_response(types::NeboError::Internal(
            "cannot open the uploads directory".into(),
        ))
    })?;
    let path = dir.join(agent::uploads::file_name(&file_id, &filename));
    std::fs::write(&path, &data)
        .map_err(|e| to_error_response(types::NeboError::Internal(e.to_string())))?;

    // The attachment as it stands: the local copy, until the loop copy below
    // re-keys it to the id the loop gave it.
    let mut landed = comm::wire::Attachment {
        url: format!("/api/v1/comm-files/{}", file_id),
        file_id,
        filename: filename.clone(),
        mime_type: mime_type.clone(),
        size,
        thumbnail_url: None,
        width: None,
        height: None,
        duration: None,
    };

    // Best-effort loop copy. Failing here costs sharing with other bots, not the
    // attachment itself, so it is logged rather than returned as an error.
    match crate::codes::build_api_client(&state) {
        Ok(api) => match api.upload_file(&filename, &mime_type, data, &[]).await {
            Ok(mut attachment) => {
                attachment.mime_type = landed_mime(&mime_type, attachment.mime_type);
                // Re-key the local copy to the loop's id so lookups by that id
                // find it here instead of downloading what we already hold.
                let renamed = dir.join(agent::uploads::file_name(&attachment.file_id, &filename));
                if let Err(e) = std::fs::rename(&path, &renamed) {
                    tracing::warn!(error = %e, "could not re-key local attachment copy");
                }
                landed = attachment;
            }
            Err(e) => {
                // A full account is the one refusal the upload itself fails
                // on: the file goes no further, here or there.
                if let Some(refusal) = hub_upload_refusal(&e) {
                    let _ = std::fs::remove_file(&path);
                    return Ok(refusal);
                }
                tracing::warn!(error = %e, "loop upload failed; attachment is local-only")
            }
        },
        Err(e) => tracing::debug!(error = %e, "not signed in; attachment is local-only"),
    }

    // The file is on this machine and named: announce the arrival so a flow
    // waiting on this kind of attachment starts on its own. Announced with the
    // id the caller is about to get back, whichever copy that is, and after
    // the bytes are on disk — nothing subscribes to a file that is not there.
    crate::attachments::announce(
        &state,
        &landed,
        Some(agent_id.as_str()).filter(|s| !s.is_empty()),
        Some(chat_id.as_str()).filter(|s| !s.is_empty()),
    );

    Ok(Json(serde_json::to_value(&landed).unwrap_or_default()).into_response())
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
    // Local copy first — an attachment uploaded on this machine renders while
    // signed out, and never costs a round trip to fetch what is already here.
    let bytes = match agent::uploads::by_id(&file_id) {
        Some(path) => std::fs::read(&path)
            .map_err(|e| to_error_response(types::NeboError::Internal(e.to_string())))?,
        None => {
            let api = crate::codes::build_api_client(&state).map_err(to_error_response)?;
            api.download_file(&file_id).await.map_err(hub_file_error)?
        }
    };

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

/// GET /api/v1/work/documents — the account-wide document index (container +
/// latest version + owning chat), newest first. The Work panel stays a
/// per-thread view; this is the cross-chat list the web Library pulls through
/// the tunnel. `?limit=&offset=` paginate (default 100).
pub async fn list_work_documents(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> HandlerResult<serde_json::Value> {
    // ?id= — the standalone /work/<id> viewer's single-document lookup.
    if let Some(id) = params.get("id") {
        let doc = state
            .store
            .get_work_document_listing(id)
            .map_err(to_error_response)?;
        let documents: Vec<db::WorkDocumentListing> = doc.into_iter().collect();
        return Ok(Json(serde_json::json!({ "documents": documents })));
    }
    let limit = params
        .get("limit")
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v > 0 && *v <= 500)
        .unwrap_or(100);
    let offset = params
        .get("offset")
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v >= 0)
        .unwrap_or(0);
    let documents = state
        .store
        .list_work_documents(limit, offset)
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({ "documents": documents })))
}

/// GET /api/v1/files/*path
///
/// `?preview=pdf` on an office document serves an on-demand PDF rendering
/// (generated via the nebo-office plugin, cached next to the source) so the
/// Work panel can show decks and Word files through its existing PDF viewer.
///
/// The file streams from disk and answers a byte range (`stream_file`): a
/// video or a song plays and seeks in WebKit and the phone's player, which
/// ask for ranges and give up on a server that ignores them.
pub async fn serve_file(
    State(state): State<AppState>,
    Path(file_path): Path<String>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
    request: axum::extract::Request,
) -> Result<axum::response::Response, (axum::http::StatusCode, Json<types::api::ErrorResponse>)> {
    let data_dir = config::data_dir().map_err(to_error_response)?;
    let files_root = data_dir.join("files");
    let (canonical, canonical_root) = within_files_root(&files_root, &files_root.join(&file_path))
        .await
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    if params.get("preview").map(String::as_str) == Some("pdf")
        && canonical
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(pdf_previewable)
    {
        return serve_pdf_preview(&state, &canonical, &canonical_root, request).await;
    }

    Ok(stream_file(&canonical, content_type_for(&canonical), request).await)
}

/// The ONE containment check for anything read out of `<data_dir>/files`:
/// resolve `..` and symlinks, then confirm the target is still inside the
/// root. Without it, `GET /api/v1/files/../../<anything>` (or a symlink, or a
/// path a reply mentions) escapes the root and serves arbitrary files.
/// Answers the canonical target and the canonical root; None when either does
/// not exist or the target lies outside — the callers answer 404 (not 403) so
/// nothing learns which paths exist.
pub(crate) async fn within_files_root(
    files_root: &std::path::Path,
    path: &std::path::Path,
) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let canonical = tokio::fs::canonicalize(path).await.ok()?;
    let canonical_root = tokio::fs::canonicalize(files_root).await.ok()?;
    canonical
        .starts_with(&canonical_root)
        .then_some((canonical, canonical_root))
}

/// A file a reply names by its path, found in the bot's files.
#[derive(Debug, PartialEq, serde::Serialize)]
pub struct LocatedFile {
    /// Where the one file route serves it: `/api/v1/files/<path in files>`.
    pub url: String,
    pub filename: String,
    /// A folder: there is nothing to open.
    pub directory: bool,
}

/// Find the file a path in a reply names — absolute, or `~/` under `home` —
/// inside `files_root`, through [`within_files_root`]. None when it is
/// missing, is the root itself, or lies anywhere outside the root.
pub(crate) async fn locate_mention(
    files_root: &std::path::Path,
    home: Option<&std::path::Path>,
    mention: &str,
) -> Option<LocatedFile> {
    let path = match mention.strip_prefix("~/") {
        Some(rest) => home?.join(rest),
        None => std::path::PathBuf::from(mention),
    };
    if !path.is_absolute() {
        return None;
    }
    let (target, root) = within_files_root(files_root, &path).await?;
    let rel = target.strip_prefix(&root).ok()?;
    if rel.as_os_str().is_empty() {
        return None;
    }
    let filename = target.file_name()?.to_string_lossy().into_owned();
    if tokio::fs::metadata(&target).await.ok()?.is_dir() {
        return Some(LocatedFile {
            url: String::new(),
            filename,
            directory: true,
        });
    }
    let encoded: Vec<String> = rel
        .components()
        .map(|c| urlencoding::encode(&c.as_os_str().to_string_lossy()).into_owned())
        .collect();
    Some(LocatedFile {
        url: format!("/api/v1/files/{}", encoded.join("/")),
        filename,
        directory: false,
    })
}

#[derive(serde::Deserialize)]
pub struct LocateQuery {
    /// The path as the reply wrote it: absolute, or `~/…`.
    pub path: String,
}

/// GET /api/v1/work/locate?path= — the file a reply mentions by its path
/// (`/data/files/BUG/report.md`), as the `/api/v1/files/` URL the Work viewer
/// opens. Only a file inside this bot's files root is found; anything else —
/// missing, outside the root, or escaping it through `..` or a symlink — is
/// 404, the same answer, so the lookup cannot probe the disk.
pub async fn locate_work_file(
    axum::extract::Query(q): axum::extract::Query<LocateQuery>,
) -> HandlerResult<LocatedFile> {
    let files_root = config::data_dir().map_err(to_error_response)?.join("files");
    let found = locate_mention(&files_root, dirs::home_dir().as_deref(), &q.path)
        .await
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;
    Ok(Json(found))
}

#[derive(serde::Deserialize)]
pub struct HistoryQuery {
    /// The file's place in the workspace (`reports/q3.xlsx`), or its
    /// absolute path inside it.
    pub path: String,
}

/// An earlier content as the app reads it: the entry, with the URL its kept
/// bytes are served at.
fn history_json(e: &db::FileHistoryEntry) -> serde_json::Value {
    let mut v = serde_json::to_value(e).unwrap_or_default();
    v["url"] = serde_json::Value::String(tools::workspace_history::blob_url(&e.hash, &e.ext));
    v
}

/// GET /api/v1/work/history?path= — a workspace file's earlier contents,
/// newest first (`tools::workspace_history`). A path outside the workspace
/// is 404.
pub async fn file_history(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<HistoryQuery>,
) -> HandlerResult<serde_json::Value> {
    let files_root = config::data_dir().map_err(to_error_response)?.join("files");
    let rel = tools::workspace_history::workspace_path(&files_root, &q.path)
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;
    let entries = tools::workspace_history::history(&state.store, &rel)
        .map_err(|e| to_error_response(types::NeboError::Internal(e)))?;
    let entries: Vec<serde_json::Value> = entries.iter().map(history_json).collect();
    Ok(Json(serde_json::json!({ "path": rel, "entries": entries })))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreHistoryRequest {
    /// The earlier content to put back.
    pub id: i64,
    /// Who the "Restored …" message is from, and the conversation it lands
    /// in, when the file is a work document there (as `restore_version`
    /// takes them).
    #[serde(default)]
    pub agent_id: String,
    pub session_id: Option<String>,
}

/// POST /api/v1/work/history/restore — put an earlier content back. The
/// file's current content is kept as history first, so the restore can be
/// undone the same way. A file that is a work document of the conversation
/// that changed it gets the restored content as its next version.
pub async fn restore_file_history(
    State(state): State<AppState>,
    Json(req): Json<RestoreHistoryRequest>,
) -> HandlerResult<serde_json::Value> {
    let files_root = config::data_dir().map_err(to_error_response)?.join("files");
    let store = state.store.clone();
    let id = req.id;
    let restored = tokio::task::spawn_blocking(move || tools::workspace_history::restore(&store, &files_root, id, None))
        .await
        .map_err(|e| to_error_response(types::NeboError::Internal(format!("restore task: {e}"))))?
        .map_err(|e| to_error_response(types::NeboError::Validation(e)))?;
    if let Some((doc, version)) = &restored.work {
        let content = format!("Restored {} to its earlier content", doc.filename);
        crate::chat_dispatch::announce_work_version(&state, doc, version, &content, &req.agent_id, req.session_id.as_deref());
    }
    Ok(Json(serde_json::json!({
        "path": restored.to.path,
        "restored": history_json(&restored.to),
        "saved": restored.saved.as_ref().map(history_json),
    })))
}

/// The Content-Type a served file goes out with, by extension.
pub(crate) fn content_type_for(path: &std::path::Path) -> &'static str {
    // Guess content type from extension. Text types carry charset=utf-8:
    // agent-written files are UTF-8, and without an explicit charset the
    // browser falls back to Windows-1252 — em-dashes and emoji render as
    // mojibake (â€", ðŸ"š) in the Work panel iframe.
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("webp") => "image/webp",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mov") => "video/quicktime",
        Some("m4v") => "video/mp4",
        Some("mp3") => "audio/mpeg",
        Some("m4a") => "audio/mp4",
        Some("wav") => "audio/wav",
        Some("ogg") => "audio/ogg",
        Some("aac") => "audio/aac",
        Some("flac") => "audio/flac",
        Some("pdf") => "application/pdf",
        Some("json") => "application/json; charset=utf-8",
        Some("txt") | Some("log") | Some("md") | Some("markdown") | Some("csv") | Some("typ") => {
            "text/plain; charset=utf-8"
        }
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "application/javascript; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Serve one file from disk: streamed in chunks, never read whole into
/// memory, with `Accept-Ranges: bytes` and a single `Range: bytes=a-b`
/// answered `206` with its `Content-Range` (more than one range is `416`).
/// `content_type` replaces the guessed type on a success, so the one
/// extension table above decides it.
pub(crate) async fn stream_file(
    path: &std::path::Path,
    content_type: &'static str,
    request: axum::extract::Request,
) -> axum::response::Response {
    use tower::Service;
    // ServeFile is always ready, and its error is `Infallible`: a failed
    // read is already a 404 or 500 response.
    let mut response = match tower_http::services::ServeFile::new(path)
        .call(request)
        .await
    {
        Ok(response) => response.map(axum::body::Body::new),
        Err(never) => match never {},
    };
    if response.status().is_success() {
        response.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static(content_type),
        );
    }
    response
}

/// The office formats that get a PDF preview: decks, which no browser can
/// show, and Word files, which the phone cannot show either (the web renders
/// those in-page and never asks). ONE gate, shared by the on-demand preview
/// endpoint and the outbound sibling upload, so a format the phone expects a
/// preview for is a format the server actually converts.
pub(crate) fn pdf_previewable(ext: &str) -> bool {
    matches!(ext, "pptx" | "ppt" | "docx" | "doc")
}

/// Render an office document to a cached PDF via the nebo-office plugin and
/// return the cache path. Results cache under `files/.previews/<path>.pdf`,
/// `<path>` the source's place under `files/` (two shared decks of one name
/// sit in different folders, so each keeps its own preview), and regenerate
/// when the source is newer. The ONE conversion implementation — used by the
/// preview endpoint and by outbound comm artifact uploads.
pub(crate) async fn ensure_pdf_preview(
    plugin_store: &napp::plugin::PluginStore,
    source: &std::path::Path,
    files_root: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    let name = source
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "invalid source file name".to_string())?;
    let rel = source
        .strip_prefix(files_root)
        .map(|r| r.to_string_lossy().into_owned())
        .unwrap_or_else(|_| name.to_string());
    let cache = files_root.join(".previews").join(format!("{rel}.pdf"));
    let previews_dir = cache.parent().unwrap_or(files_root).to_path_buf();

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
        let output = command::new::<tokio::process::Command>(&bin, command::Console::Hidden)
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

/// Serve the on-demand office→PDF preview. 503 when the plugin is missing or
/// conversion fails — the viewer falls back to its download card.
async fn serve_pdf_preview(
    state: &AppState,
    source: &std::path::Path,
    files_root: &std::path::Path,
    request: axum::extract::Request,
) -> Result<axum::response::Response, (axum::http::StatusCode, Json<types::api::ErrorResponse>)> {
    let cache = ensure_pdf_preview(&state.plugin_store, source, files_root)
        .await
        .map_err(|e| {
            (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                Json(types::api::ErrorResponse {
                    error: format!("preview unavailable: {e}"),
                }),
            )
        })?;

    Ok(stream_file(&cache, "application/pdf", request).await)
}

/// The type an uploaded file keeps once the loop has its copy. The loop types
/// a file by its extension alone, and to it a `.webm` is video. A recording
/// the client declared as audio stays audio, so the employee hears it and the
/// bubble plays it, rather than both treating it as a silent video.
fn landed_mime(declared: &str, from_loop: String) -> String {
    if declared.starts_with("audio/") && !from_loop.starts_with("audio/") {
        declared.to_string()
    } else {
        from_loop
    }
}

#[cfg(test)]
mod landed_mime_tests {
    use super::landed_mime;

    #[test]
    fn a_recording_declared_as_audio_stays_audio() {
        assert_eq!(landed_mime("audio/webm", "video/webm".into()), "audio/webm");
        assert_eq!(landed_mime("audio/mp4", "audio/mp4".into()), "audio/mp4");
        assert_eq!(landed_mime("audio/x-m4a", "audio/mp4".into()), "audio/mp4");
        assert_eq!(landed_mime("video/webm", "video/webm".into()), "video/webm");
        assert_eq!(landed_mime("image/png", "image/png".into()), "image/png");
    }
}

#[cfg(test)]
mod preview_gate_tests {
    use super::pdf_previewable;

    /// The phone pairs `<name>.preview.pdf` to decks and Word files. A format
    /// it expects a sibling for must be one the server converts, and nothing
    /// else may quietly gain a conversion the clients do not pair.
    #[test]
    fn office_documents_get_a_pdf_preview() {
        for ext in ["pptx", "ppt", "docx", "doc"] {
            assert!(pdf_previewable(ext), "{ext} should be previewable");
        }
        for ext in ["pdf", "xlsx", "md", "html", "png", ""] {
            assert!(!pdf_previewable(ext), "{ext} should not be previewable");
        }
    }
}

#[cfg(test)]
mod stream_file_tests {
    use super::{content_type_for, stream_file};
    use axum::http::{Request, StatusCode, header};
    use tower::ServiceExt;

    /// 1 KiB of distinct bytes, so a wrong offset reads as a wrong answer.
    fn clip() -> (tempfile::TempDir, std::path::PathBuf, Vec<u8>) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("clip.mp4");
        let bytes: Vec<u8> = (0..1024u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &bytes).unwrap();
        (tmp, path, bytes)
    }

    /// The file served the way the app serves it: `stream_file` behind the
    /// server's own compression layer.
    async fn get(
        path: &std::path::Path,
        headers: &[(&str, &str)],
    ) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let path = path.to_path_buf();
        let app = axum::Router::new()
            .route(
                "/f",
                axum::routing::get(move |req: axum::extract::Request| async move {
                    stream_file(&path, content_type_for(&path), req).await
                }),
            )
            .layer(crate::compression());
        let mut req = Request::builder().uri("/f");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let res = app
            .oneshot(req.body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, headers, body.to_vec())
    }

    fn header<'a>(h: &'a axum::http::HeaderMap, name: header::HeaderName) -> &'a str {
        h.get(name).and_then(|v| v.to_str().ok()).unwrap_or("")
    }

    /// The request WebKit and the phone's player make: one byte range,
    /// answered 206 with exactly those bytes and where they sit.
    #[tokio::test]
    async fn a_range_request_gets_206_with_exactly_those_bytes() {
        let (_tmp, path, bytes) = clip();
        let (status, h, body) = get(
            &path,
            &[("range", "bytes=100-199"), ("accept-encoding", "gzip")],
        )
        .await;
        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(body, bytes[100..200]);
        assert_eq!(header(&h, header::CONTENT_RANGE), "bytes 100-199/1024");
        assert_eq!(header(&h, header::CONTENT_LENGTH), "100");
        assert_eq!(header(&h, header::ACCEPT_RANGES), "bytes");
        assert_eq!(header(&h, header::CONTENT_TYPE), "video/mp4");
        assert_eq!(header(&h, header::CONTENT_ENCODING), "");

        let (status, h, body) = get(&path, &[("range", "bytes=0-1")]).await;
        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(body, bytes[0..2]);
        assert_eq!(header(&h, header::CONTENT_RANGE), "bytes 0-1/1024");

        let (status, _, body) = get(&path, &[("range", "bytes=1000-")]).await;
        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(body, bytes[1000..]);

        let (status, _, _) = get(&path, &[("range", "bytes=5000-6000")]).await;
        assert_eq!(status, StatusCode::RANGE_NOT_SATISFIABLE);
    }

    /// A plain GET is the whole file, saying ranges are welcome; a video is
    /// never gzipped, which would hide its length and its ranges.
    #[tokio::test]
    async fn a_whole_video_says_it_takes_ranges_and_is_not_gzipped() {
        let (_tmp, path, bytes) = clip();
        let (status, h, body) = get(&path, &[("accept-encoding", "gzip")]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, bytes);
        assert_eq!(header(&h, header::ACCEPT_RANGES), "bytes");
        assert_eq!(header(&h, header::CONTENT_LENGTH), "1024");
        assert_eq!(header(&h, header::CONTENT_ENCODING), "");
        assert_eq!(header(&h, header::CONTENT_TYPE), "video/mp4");
    }

    #[test]
    fn audio_and_video_get_their_types() {
        for (name, ty) in [
            ("a.mp4", "video/mp4"),
            ("a.MOV", "video/quicktime"),
            ("a.mp3", "audio/mpeg"),
            ("a.m4a", "audio/mp4"),
            ("a.wav", "audio/wav"),
            ("a.ogg", "audio/ogg"),
            ("a.aac", "audio/aac"),
            ("a.flac", "audio/flac"),
            ("a.md", "text/plain; charset=utf-8"),
            ("a.bin", "application/octet-stream"),
        ] {
            assert_eq!(content_type_for(std::path::Path::new(name)), ty, "{name}");
        }
    }
}

#[cfg(test)]
mod hub_refusal_tests {
    use super::{hub_file_error, hub_upload_refusal};
    use comm::CommError;

    fn http(status: u16, body: &str) -> CommError {
        CommError::Http {
            status,
            body: body.to_string(),
        }
    }

    /// A file the hub removed after its keeping period reaches the apps as
    /// 410 with the hub's own sentence; any other refusal stays a 500.
    #[test]
    fn a_removed_file_is_gone_in_the_hubs_words() {
        let (status, body) = hub_file_error(http(
            410,
            r#"{"error":"This file was removed 30 days after it was uploaded."}"#,
        ));
        assert_eq!(status, axum::http::StatusCode::GONE);
        assert_eq!(
            body.0.error,
            "This file was removed 30 days after it was uploaded."
        );

        let (status, body) = hub_file_error(http(410, "gone"));
        assert_eq!(status, axum::http::StatusCode::GONE);
        assert_eq!(body.0.error, "This file was removed.");

        let (status, _) = hub_file_error(http(404, r#"{"error":"file not found"}"#));
        assert_eq!(status, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    }

    /// A full account is passed on as the hub said it, code and all; a file
    /// over the size limit (a 413 with no code) and other failures are not.
    #[tokio::test]
    async fn only_a_full_account_fails_the_upload() {
        let full = r#"{"error":"Your storage is full (2 GB). Uploads clear 30 days after they're added.","code":"storage_full"}"#;
        let resp = hub_upload_refusal(&http(413, full)).expect("passed on");
        assert_eq!(resp.status(), axum::http::StatusCode::PAYLOAD_TOO_LARGE);
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 16)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["code"], "storage_full");
        assert_eq!(
            v["error"],
            "Your storage is full (2 GB). Uploads clear 30 days after they're added."
        );

        assert!(hub_upload_refusal(&http(413, r#"{"error":"file too large"}"#)).is_none());
        assert!(hub_upload_refusal(&http(500, full)).is_none());
        assert!(hub_upload_refusal(&CommError::Other("offline".into())).is_none());
    }
}

#[cfg(test)]
mod locate_tests {
    use super::{LocatedFile, locate_mention};

    /// A bot's data folder: `files/` with a report in a subfolder, a folder
    /// named like a file, and a secret beside `files/` that must stay unseen.
    fn bot() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let files = tmp.path().join("files");
        std::fs::create_dir_all(files.join("BUG")).unwrap();
        std::fs::create_dir_all(files.join("site.d")).unwrap();
        std::fs::write(files.join("BUG/bug report #1.md"), "# Bug").unwrap();
        std::fs::write(tmp.path().join("settings.json"), "{\"secret\":1}").unwrap();
        (tmp, files)
    }

    fn abs(p: &std::path::Path) -> String {
        p.to_string_lossy().into_owned()
    }

    #[tokio::test]
    async fn a_file_in_the_files_root_opens_through_the_files_route() {
        let (_tmp, files) = bot();
        let found = locate_mention(&files, None, &abs(&files.join("BUG/bug report #1.md"))).await;
        assert_eq!(
            found,
            Some(LocatedFile {
                url: "/api/v1/files/BUG/bug%20report%20%231.md".into(),
                filename: "bug report #1.md".into(),
                directory: false,
            })
        );
    }

    #[tokio::test]
    async fn a_home_relative_path_resolves_under_home() {
        let (tmp, files) = bot();
        let found = locate_mention(&files, Some(tmp.path()), "~/files/BUG/bug report #1.md").await;
        assert_eq!(
            found.map(|f| f.filename),
            Some("bug report #1.md".to_string())
        );
        // No home known: a `~/` path names nothing.
        assert_eq!(
            locate_mention(&files, None, "~/files/BUG/bug report #1.md").await,
            None
        );
    }

    #[tokio::test]
    async fn a_folder_is_found_but_opens_nothing() {
        let (_tmp, files) = bot();
        let found = locate_mention(&files, None, &abs(&files.join("site.d")))
            .await
            .unwrap();
        assert!(found.directory);
        assert!(found.url.is_empty());
    }

    #[tokio::test]
    async fn nothing_outside_the_files_root_is_found() {
        let (tmp, files) = bot();
        let secret = abs(&tmp.path().join("settings.json"));
        let climbing = abs(&files.join("BUG/../../settings.json"));
        for refused in [
            secret.as_str(),
            climbing.as_str(),
            "/etc/hosts",
            "/etc/../etc/passwd",
            // Relative paths name nothing: only absolute and `~/` mentions.
            "BUG/bug report #1.md",
            "../settings.json",
        ] {
            assert_eq!(
                locate_mention(&files, None, refused).await,
                None,
                "{refused}"
            );
        }
        // The root itself is not a file to open.
        assert_eq!(locate_mention(&files, None, &abs(&files)).await, None);
        // Missing.
        assert_eq!(
            locate_mention(&files, None, &abs(&files.join("BUG/gone.md"))).await,
            None
        );
    }

    /// Through the handler, as a request: a system file, a climb out of the
    /// files root and a relative path all get the same 404 a missing file
    /// gets, so the lookup tells a caller nothing about the disk.
    #[tokio::test]
    async fn the_route_answers_404_for_anything_outside_the_files_root() {
        use tower::ServiceExt;
        let app = axum::Router::new().route("/locate", axum::routing::get(super::locate_work_file));
        for path in [
            "%2Fetc%2Fhosts",
            "%2Fdata%2Ffiles%2F..%2F..%2Fetc%2Fpasswd",
            "..%2Fsettings.json",
            "~%2F.ssh%2Fid_ed25519",
        ] {
            let res = app
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .uri(format!("/locate?path={path}"))
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(res.status(), axum::http::StatusCode::NOT_FOUND, "{path}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlink_out_of_the_files_root_is_refused() {
        let (tmp, files) = bot();
        std::os::unix::fs::symlink(
            tmp.path().join("settings.json"),
            files.join("BUG/link.json"),
        )
        .unwrap();
        std::os::unix::fs::symlink(tmp.path(), files.join("up")).unwrap();
        assert_eq!(
            locate_mention(&files, None, &abs(&files.join("BUG/link.json"))).await,
            None
        );
        assert_eq!(
            locate_mention(&files, None, &abs(&files.join("up/settings.json"))).await,
            None
        );
    }
}

#[cfg(test)]
mod history_tests {
    use serde_json::json;

    /// Through the real server: a workspace file a command overwrote lists
    /// its earlier content, and the restore puts it back while keeping what
    /// it replaced.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_app_lists_and_restores_a_files_earlier_content() {
        let nebo = crate::staffed_proof::session().await;
        let files = nebo.home.join("files");
        let dir = files.join(format!("history-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let rel = format!("{}/growth model.xlsx", dir.file_name().unwrap().to_string_lossy());
        std::fs::write(files.join(&rel), "nine sheets").unwrap();
        tools::workspace_history::look(nebo.store(), &files, Some("c1")).unwrap();
        std::fs::write(files.join(&rel), "partial").unwrap();
        tools::workspace_history::look(nebo.store(), &files, Some("c1")).unwrap();

        let path = urlencoding::encode(&rel).into_owned();
        let listed = nebo.get_ok(&format!("/work/history?path={path}")).await;
        assert_eq!(listed["path"], rel);
        let entries = listed["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        let url = entries[0]["url"].as_str().unwrap().to_string();
        assert!(url.starts_with("/api/v1/files/work/blobs/") && url.ends_with(".xlsx"), "{url}");

        let restored = nebo.post_ok("/work/history/restore", &json!({ "id": entries[0]["id"] })).await;
        assert_eq!(restored["path"], rel);
        assert_eq!(std::fs::read_to_string(files.join(&rel)).unwrap(), "nine sheets");
        let saved_url = restored["saved"]["url"].as_str().unwrap();
        let saved = saved_url.strip_prefix("/api/v1/files/").unwrap();
        assert_eq!(std::fs::read_to_string(files.join(saved)).unwrap(), "partial");

        let (status, _) = nebo.get("/work/history?path=..%2Fsettings.json").await;
        assert_eq!(status, 404);
        let (status, _) = nebo.post("/work/history/restore", &json!({ "id": 999_999 })).await;
        assert_eq!(status, 400);
    }
}
