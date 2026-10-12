//! `/api/v1/uploads`: the resumable door for large files (`upload_sessions`).
//!
//! - `POST /uploads` `{filename, mimeType, size, agentId?, chatId?}` opens a
//!   session: `201 {uploadId, offset, size, chunkSize}`.
//! - `PATCH /uploads/{id}` with `Upload-Offset: N` and the chunk as the body
//!   writes it: `200` with the new offset, or `409` with the offset to send
//!   from when `N` is past it.
//! - `GET /uploads/{id}` says where it stands: the offset while it is coming,
//!   and once complete the attachment and its hub copy's state.
//! - `POST /uploads/{id}/complete` `{sha256}` checks the size and the hash,
//!   lands the file and answers its attachment, the same as `POST
//!   /files/upload` does. The hub copy follows in the background (`hub_sync`).
//!
//! The bot answers as soon as the file is whole here: the phone never waits
//! on NeboAI.

use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use serde::{Deserialize, Serialize};

use crate::hub_sync::{Records, SyncState};
use crate::state::AppState;
use crate::upload_sessions::{CHUNK_SIZE, Session, Store, UploadError};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateUploadRequest {
    pub filename: String,
    #[serde(default)]
    pub mime_type: String,
    pub size: u64,
    #[serde(default)]
    pub agent_id: String,
    #[serde(default)]
    pub chat_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompleteUploadRequest {
    pub sha256: String,
}

/// Where an upload stands.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadStatus {
    pub upload_id: String,
    /// Bytes this bot holds; send from here.
    pub offset: u64,
    pub size: u64,
    pub chunk_size: u64,
    pub complete: bool,
    /// The landed file, once complete.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachment: Option<comm::wire::Attachment>,
    /// The hub copy, once complete.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync: Option<UploadSync>,
}

/// A landed file's hub copy.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadSync {
    pub state: SyncState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn in_progress(s: &Session) -> UploadStatus {
    UploadStatus {
        upload_id: s.upload_id.clone(),
        offset: s.offset,
        size: s.size,
        chunk_size: CHUNK_SIZE,
        complete: false,
        attachment: None,
        sync: None,
    }
}

fn error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(types::api::ErrorResponse { error: message.into() })).into_response()
}

fn refused(e: UploadError) -> Response {
    match e {
        UploadError::NotFound => error(StatusCode::NOT_FOUND, "no such upload"),
        UploadError::Conflict(s) => (StatusCode::CONFLICT, Json(in_progress(&s))).into_response(),
        UploadError::TooLarge(m) => error(StatusCode::PAYLOAD_TOO_LARGE, m),
        UploadError::Invalid(m) => error(StatusCode::BAD_REQUEST, m),
        UploadError::HashMismatch => error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "The file did not arrive intact; send it again.",
        ),
        UploadError::Io(m) => {
            tracing::warn!(error = %m, "upload session");
            error(StatusCode::INTERNAL_SERVER_ERROR, m)
        }
    }
}

fn stores() -> Result<(Store, Records), UploadError> {
    match (Store::open(), Records::open()) {
        (Some(s), Some(r)) => Ok((s, r)),
        _ => Err(UploadError::Io("cannot open the uploads directory".into())),
    }
}

/// A landed file as an upload's status: complete, with its hub copy.
fn landed(id: &str, r: crate::hub_sync::Record) -> UploadStatus {
    UploadStatus {
        upload_id: id.to_string(),
        offset: r.attachment.size,
        size: r.attachment.size,
        chunk_size: CHUNK_SIZE,
        complete: true,
        sync: Some(UploadSync { state: r.state, error: r.error.clone() }),
        attachment: Some(r.attachment),
    }
}

/// POST /api/v1/uploads
pub async fn create_upload(Json(req): Json<CreateUploadRequest>) -> Response {
    let (store, _) = match stores() {
        Ok(s) => s,
        Err(e) => return refused(e),
    };
    match store.create(&req.filename, &req.mime_type, req.size, &req.agent_id, &req.chat_id) {
        Ok(s) => (StatusCode::CREATED, Json(in_progress(&s))).into_response(),
        Err(e) => refused(e),
    }
}

/// GET /api/v1/uploads/{id} — also answers for a file whose hub copy is
/// owed after `POST /files/upload`, by its file id.
pub async fn upload_status(Path(id): Path<String>) -> Response {
    let (store, records) = match stores() {
        Ok(s) => s,
        Err(e) => return refused(e),
    };
    match store.get(&id) {
        Ok(s) => Json(in_progress(&s)).into_response(),
        Err(UploadError::NotFound) => match records.get(&id) {
            Some(r) => Json(landed(&id, r)).into_response(),
            None => refused(UploadError::NotFound),
        },
        Err(e) => refused(e),
    }
}

/// PATCH /api/v1/uploads/{id}
pub async fn upload_chunk(Path(id): Path<String>, headers: HeaderMap, request: Request) -> Response {
    let (store, _) = match stores() {
        Ok(s) => s,
        Err(e) => return refused(e),
    };
    let Some(offset) = headers
        .get("upload-offset")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
    else {
        return error(StatusCode::BAD_REQUEST, "Upload-Offset is required");
    };
    let body = request.into_body().into_data_stream();
    match store.write(&id, offset, body).await {
        Ok(s) => Json(in_progress(&s)).into_response(),
        Err(e) => refused(e),
    }
}

/// POST /api/v1/uploads/{id}/complete
pub async fn complete_upload(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<CompleteUploadRequest>,
) -> Response {
    let (store, records) = match stores() {
        Ok(s) => s,
        Err(e) => return refused(e),
    };
    let (agent_id, chat_id) = match store.get(&id) {
        Ok(s) => (s.agent_id, s.chat_id),
        // Completed before (its answer was lost): the same attachment again.
        Err(UploadError::NotFound) => {
            return match records.get(&id) {
                Some(r) => Json(r.attachment).into_response(),
                None => refused(UploadError::NotFound),
            };
        }
        Err(e) => return refused(e),
    };
    let attachment = match store.complete(&id, &req.sha256).await {
        Ok(a) => a,
        Err(e) => return refused(e),
    };
    // Recorded before answering: from here the hub copy is owed, and the
    // record is what says how it went.
    if let Err(e) = records.owe(&attachment) {
        tracing::warn!(file_id = %attachment.file_id, error = %e, "hub copy not recorded");
    }
    crate::attachments::announce(
        &state,
        &attachment,
        Some(agent_id.as_str()).filter(|s| !s.is_empty()),
        Some(chat_id.as_str()).filter(|s| !s.is_empty()),
    );
    Json(attachment).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use sha2::Digest;
    use tower::ServiceExt;

    /// The stores live under `config::data_dir()`: a scratch one, set once
    /// per process and never over a root another test already set.
    fn scratch_home() {
        static HOME: std::sync::OnceLock<Option<tempfile::TempDir>> = std::sync::OnceLock::new();
        HOME.get_or_init(|| {
            if std::env::var_os("NEBO_HOME").is_some() {
                return None;
            }
            let dir = tempfile::tempdir().unwrap();
            // SAFETY: one-time initialization, and only when no root is set.
            unsafe { std::env::set_var("NEBO_HOME", dir.path()) };
            Some(dir)
        });
    }

    /// The doors as the files router mounts them (all but complete, which
    /// announces through the app's state and is proven in `upload_sessions`).
    fn doors() -> axum::Router {
        axum::Router::new()
            .route("/uploads", axum::routing::post(create_upload))
            .route("/uploads/{id}", axum::routing::get(upload_status).patch(upload_chunk))
    }

    async fn call(req: HttpRequest<Body>) -> (StatusCode, serde_json::Value) {
        let resp = doors().oneshot(req).await.unwrap();
        let status = resp.status();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null))
    }

    fn chunk(id: &str, offset: u64, bytes: &[u8]) -> HttpRequest<Body> {
        HttpRequest::patch(format!("/uploads/{id}"))
            .header("Upload-Offset", offset.to_string())
            .header("Content-Type", "application/offset+octet-stream")
            .body(Body::from(bytes.to_vec()))
            .unwrap()
    }

    /// Over HTTP: open, a chunk, a chunk past the offset (409 with the
    /// offset), resume from the offset, and once landed the status carries
    /// the attachment and its owed hub copy.
    #[tokio::test]
    async fn the_doors_speak_the_protocol() {
        scratch_home();
        let data: Vec<u8> = (0..20u8).collect();
        let (status, opened) = call(
            HttpRequest::post("/uploads")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"filename":"v.mp4","mimeType":"video/mp4","size":20}"#))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{opened}");
        assert_eq!(opened["offset"], 0);
        assert_eq!(opened["chunkSize"], CHUNK_SIZE);
        let id = opened["uploadId"].as_str().unwrap().to_string();

        let (status, after) = call(chunk(&id, 0, &data[..8])).await;
        assert_eq!(status, StatusCode::OK, "{after}");
        assert_eq!(after["offset"], 8);

        let (status, gap) = call(chunk(&id, 12, &data[12..])).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(gap["offset"], 8, "the offset to send from");

        let (status, missing) = call(HttpRequest::patch(format!("/uploads/{id}")).body(Body::from("x")).unwrap()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{missing}");

        let (_, held) = call(HttpRequest::get(format!("/uploads/{id}")).body(Body::empty()).unwrap()).await;
        assert_eq!(held["offset"], 8);
        assert_eq!(held["complete"], false);
        let (status, done) = call(chunk(&id, 8, &data[8..])).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(done["offset"], 20);

        let (store, records) = stores().unwrap();
        let att = store.complete(&id, &hex::encode(sha2::Sha256::digest(&data))).await.unwrap();
        records.owe(&att).unwrap();
        let (status, landed) = call(HttpRequest::get(format!("/uploads/{id}")).body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(landed["complete"], true);
        assert_eq!(landed["attachment"]["fileId"], id.as_str());
        assert_eq!(landed["sync"]["state"], "pending");

        let (status, _) = call(HttpRequest::get(format!("/uploads/{}", uuid::Uuid::new_v4())).body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// Over the cap, refused at the door before a byte is sent.
    #[tokio::test]
    async fn a_file_over_the_cap_is_refused_at_create() {
        scratch_home();
        let body = format!(r#"{{"filename":"v.mp4","size":{}}}"#, crate::upload_sessions::MAX_BYTES + 1);
        let (status, refused) = call(
            HttpRequest::post("/uploads").header("Content-Type", "application/json").body(Body::from(body)).unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{refused}");
    }
}
