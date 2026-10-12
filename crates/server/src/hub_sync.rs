//! The hub copy of an uploaded file, made after the upload has answered.
//!
//! A file lands on this machine first; the copy on NeboAI is what lets other
//! bots and devices reach it. Before, a large file's copy ran while the phone
//! waited, inside one request with a fixed deadline, and a copy that failed
//! left the file on this machine with nothing but a log line to say so. Now
//! every file whose copy is still owed has a record, `<data_dir>/upload-sync/
//! <file id>.json`, and one background loop works through them: chunked and
//! resumable through the hub's `/api/v1/uploads` (so a restart resumes from
//! the hub's own offset), retried with a growing wait, and its state —
//! pending, synced, failed, refused — is what `GET /api/v1/uploads/{id}`
//! reports, so the apps can show it.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use crate::upload_sessions::{now, valid_id};

/// Where a file's hub copy stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncState {
    /// Owed: the loop will try (again).
    Pending,
    /// NeboAI has it.
    Synced,
    /// Every try failed; tried again when the bot next starts.
    Failed,
    /// NeboAI said no (too large for its type, account full): not retried.
    Refused,
}

/// One landed file and its hub copy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    /// The file as this bot answers for it.
    pub attachment: comm::wire::Attachment,
    pub state: SyncState,
    /// The hub's upload session, kept so a copy cut short resumes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub_upload_id: Option<String>,
    /// The hub's id for its copy, once it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub_file_id: Option<String>,
    #[serde(default)]
    pub attempts: u32,
    /// Why the last try did not land.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Unix seconds before which the loop leaves it alone.
    #[serde(default)]
    pub next_attempt: u64,
}

/// Tries before a copy is called failed (about five hours of waiting).
const MAX_ATTEMPTS: u32 = 10;

/// The wait after the `attempts`-th failure: 10 s, tripling, at most an hour.
fn backoff(attempts: u32) -> u64 {
    10u64.saturating_mul(3u64.saturating_pow(attempts.saturating_sub(1))).min(3600)
}

/// The record store. Tests give a tempdir.
#[derive(Clone)]
pub struct Records {
    dir: PathBuf,
}

impl Records {
    pub fn open() -> Option<Records> {
        Some(Records::at(&config::data_dir().ok()?))
    }

    pub fn at(data: &Path) -> Records {
        Records { dir: data.join("upload-sync") }
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    pub fn get(&self, file_id: &str) -> Option<Record> {
        if !valid_id(file_id) {
            return None;
        }
        serde_json::from_slice(&std::fs::read(self.path(file_id)).ok()?).ok()
    }

    pub fn save(&self, r: &Record) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let tmp = self.dir.join(format!("{}.json.tmp", r.attachment.file_id));
        std::fs::write(&tmp, serde_json::to_vec(r).map_err(std::io::Error::other)?)?;
        std::fs::rename(&tmp, self.path(&r.attachment.file_id))
    }

    fn all(&self) -> Vec<Record> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
            .filter_map(|e| serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok())
            .collect()
    }

    /// Owe the hub a copy of `attachment`, and wake the loop.
    pub fn owe(&self, attachment: &comm::wire::Attachment) -> std::io::Result<Record> {
        let r = Record {
            attachment: attachment.clone(),
            state: SyncState::Pending,
            hub_upload_id: None,
            hub_file_id: None,
            attempts: 0,
            error: None,
            next_attempt: 0,
        };
        self.save(&r)?;
        WAKE.notify_one();
        Ok(r)
    }
}

/// Wakes the loop when a copy is owed.
static WAKE: LazyLock<tokio::sync::Notify> = LazyLock::new(tokio::sync::Notify::new);

/// One try at one record's copy, and the record as it then stands. `api` is
/// absent while signed out: nothing to copy to yet, so it waits.
pub async fn attempt(api: Option<&comm::api::NeboAIApi>, records: &Records, file: &Path, mut r: Record) -> Record {
    let Some(api) = api else {
        r.error = Some("not signed in to NeboAI".into());
        r.next_attempt = now() + 3600;
        let _ = records.save(&r);
        return r;
    };
    let session = r.hub_upload_id.clone();
    let mut started = None;
    let result = api
        .upload_file_resumable(file, &r.attachment.filename, session, |id| {
            // Kept at once: a restart mid-copy resumes this session.
            let mut kept = r.clone();
            kept.hub_upload_id = Some(id.to_string());
            let _ = records.save(&kept);
            started = Some(id.to_string());
        })
        .await;
    if let Some(id) = started {
        r.hub_upload_id = Some(id);
    }
    match result {
        Ok(copy) => {
            r.state = SyncState::Synced;
            r.hub_file_id = Some(copy.file_id);
            r.error = None;
        }
        // A frozen bot sends nothing; it is not this file's failure.
        Err(comm::CommError::Paused) => {
            r.next_attempt = now() + 60;
        }
        // The hub refused the file itself: trying again changes nothing.
        Err(comm::CommError::Http { status: 400 | 413 | 415 | 422, body }) => {
            r.state = SyncState::Refused;
            r.error = Some(hub_sentence(&body));
        }
        Err(e) => {
            r.attempts += 1;
            r.error = Some(e.to_string());
            if r.attempts >= MAX_ATTEMPTS {
                r.state = SyncState::Failed;
            } else {
                r.next_attempt = now() + backoff(r.attempts);
            }
        }
    }
    if let Err(e) = records.save(&r) {
        tracing::warn!(file_id = %r.attachment.file_id, error = %e, "hub copy: record not saved");
    }
    match r.state {
        SyncState::Synced => tracing::info!(file_id = %r.attachment.file_id, "hub copy made"),
        SyncState::Pending => tracing::info!(file_id = %r.attachment.file_id, error = ?r.error, "hub copy: will try again"),
        _ => tracing::warn!(file_id = %r.attachment.file_id, state = ?r.state, error = ?r.error, "hub copy not made"),
    }
    r
}

/// The hub's own sentence from a refusal body, or the body.
fn hub_sentence(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(|m| m.as_str()).map(str::to_string))
        .unwrap_or_else(|| body.to_string())
}

/// The loop: started once at boot. A failed copy from before the restart is
/// owed again; then every due record is tried in turn, and the loop sleeps
/// until the next is due or a new one is owed.
pub fn spawn(state: crate::state::AppState) {
    let Some(records) = Records::open() else { return };
    tokio::spawn(async move {
        for mut r in records.all() {
            if r.state == SyncState::Failed {
                r.state = SyncState::Pending;
                r.attempts = 0;
                r.next_attempt = 0;
                let _ = records.save(&r);
            }
        }
        loop {
            let sessions = crate::upload_sessions::Store::open();
            if let Some(s) = &sessions {
                s.sweep(now());
            }
            let api = crate::codes::build_api_client(&state).ok();
            let mut next = now() + 3600;
            for r in records.all().into_iter().filter(|r| r.state == SyncState::Pending) {
                if r.next_attempt > now() {
                    next = next.min(r.next_attempt);
                    continue;
                }
                let Some(file) = agent::uploads::by_id(&r.attachment.file_id) else {
                    let mut gone = r;
                    gone.state = SyncState::Failed;
                    gone.error = Some("the file is no longer on this machine".into());
                    let _ = records.save(&gone);
                    continue;
                };
                let r = attempt(api.as_ref(), &records, &file, r).await;
                if r.state == SyncState::Pending {
                    next = next.min(r.next_attempt);
                }
            }
            let wait = std::time::Duration::from_secs(next.saturating_sub(now()).max(1));
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = WAKE.notified() => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use axum::extract::{Path as AxPath, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::{get, post};
    use axum::{Json, Router};

    /// A hub's `/api/v1/uploads` that keeps the bytes it is sent, answers a
    /// chunk at `fail_at` with a 502 once, and can refuse at create.
    #[derive(Clone, Default)]
    struct Hub {
        held: Arc<Mutex<Vec<u8>>>,
        size: Arc<Mutex<u64>>,
        fail_at: Arc<Mutex<Option<usize>>>,
        creates: Arc<AtomicUsize>,
        refuse: Arc<Mutex<Option<(u16, String)>>>,
    }

    const CHUNK: u64 = 4;

    fn status(h: &Hub) -> serde_json::Value {
        serde_json::json!({"uploadId": "s1", "offset": h.held.lock().unwrap().len(), "size": *h.size.lock().unwrap(), "chunkSize": CHUNK})
    }

    async fn create(State(h): State<Hub>, Json(body): Json<serde_json::Value>) -> (StatusCode, Json<serde_json::Value>) {
        if let Some((code, msg)) = h.refuse.lock().unwrap().clone() {
            return (StatusCode::from_u16(code).unwrap(), Json(serde_json::json!({"error": msg})));
        }
        h.creates.fetch_add(1, Ordering::SeqCst);
        h.held.lock().unwrap().clear();
        *h.size.lock().unwrap() = body["size"].as_u64().unwrap();
        (StatusCode::CREATED, Json(status(&h)))
    }

    async fn head(State(h): State<Hub>, AxPath(_id): AxPath<String>) -> Json<serde_json::Value> {
        Json(status(&h))
    }

    async fn chunk(State(h): State<Hub>, AxPath(_id): AxPath<String>, headers: HeaderMap, body: axum::body::Bytes) -> (StatusCode, Json<serde_json::Value>) {
        let offset: usize = headers["upload-offset"].to_str().unwrap().parse().unwrap();
        if *h.fail_at.lock().unwrap() == Some(offset) {
            *h.fail_at.lock().unwrap() = None;
            return (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error": "bad gateway"})));
        }
        let mut held = h.held.lock().unwrap();
        if offset != held.len() {
            drop(held);
            return (StatusCode::CONFLICT, Json(status(&h)));
        }
        held.extend_from_slice(&body);
        drop(held);
        (StatusCode::OK, Json(status(&h)))
    }

    async fn complete(State(h): State<Hub>, AxPath(_id): AxPath<String>) -> (StatusCode, Json<serde_json::Value>) {
        let n = h.held.lock().unwrap().len();
        (StatusCode::CREATED, Json(serde_json::json!({"fileId": "hub-1", "filename": "v.mp4", "mimeType": "video/mp4", "size": n, "url": "/x"})))
    }

    async fn hub() -> (Hub, comm::api::NeboAIApi) {
        let h = Hub::default();
        let app = Router::new()
            .route("/api/v1/uploads", post(create))
            .route("/api/v1/uploads/{id}", get(head).patch(chunk))
            .route("/api/v1/uploads/{id}/complete", post(complete))
            .with_state(h.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (h, comm::api::NeboAIApi::new(format!("http://{addr}"), "bot".into(), "token".into()))
    }

    fn landed(dir: &Path, bytes: &[u8]) -> (Records, PathBuf, Record) {
        let records = Records::at(dir);
        let file = dir.join("v.mp4");
        std::fs::write(&file, bytes).unwrap();
        let att = comm::wire::Attachment {
            file_id: uuid::Uuid::new_v4().to_string(),
            filename: "v.mp4".into(),
            mime_type: "video/mp4".into(),
            size: bytes.len() as u64,
            url: String::new(),
            thumbnail_url: None,
            width: None,
            height: None,
            duration: None,
        };
        let r = records.owe(&att).unwrap();
        (records, file, r)
    }

    /// The hub fails a chunk: the record stays owed with a wait, and the next
    /// try resumes the same hub session from the hub's offset and lands.
    #[tokio::test]
    async fn the_hub_copy_retries_and_resumes() {
        let (h, api) = hub().await;
        let tmp = tempfile::tempdir().unwrap();
        let bytes = b"0123456789abc";
        let (records, file, r) = landed(tmp.path(), bytes);

        // The second chunk fails once.
        *h.fail_at.lock().unwrap() = Some(CHUNK as usize);
        let r = attempt(Some(&api), &records, &file, r).await;
        assert_eq!(r.state, SyncState::Pending, "{r:?}");
        assert_eq!(r.attempts, 1);
        assert!(r.next_attempt > now(), "it waits before trying again");
        assert_eq!(r.hub_upload_id.as_deref(), Some("s1"), "the hub session is kept");
        assert_eq!(records.get(&r.attachment.file_id).unwrap().state, SyncState::Pending);
        let held_before = h.held.lock().unwrap().len();
        assert_eq!(held_before, CHUNK as usize, "the first chunk is held");

        let r = attempt(Some(&api), &records, &file, records.get(&r.attachment.file_id).unwrap()).await;
        assert_eq!(r.state, SyncState::Synced, "{r:?}");
        assert_eq!(r.hub_file_id.as_deref(), Some("hub-1"));
        assert_eq!(h.creates.load(Ordering::SeqCst), 1, "resumed, not started over");
        assert_eq!(h.held.lock().unwrap().as_slice(), bytes);
        assert_eq!(records.get(&r.attachment.file_id).unwrap().state, SyncState::Synced);
    }

    /// The hub refuses the file itself: refused, with the hub's words, and
    /// never tried again.
    #[tokio::test]
    async fn a_refusal_is_recorded_not_retried() {
        let (h, api) = hub().await;
        *h.refuse.lock().unwrap() = Some((400, "file too large (max 50MB for file)".into()));
        let tmp = tempfile::tempdir().unwrap();
        let (records, file, r) = landed(tmp.path(), b"abc");
        let r = attempt(Some(&api), &records, &file, r).await;
        assert_eq!(r.state, SyncState::Refused);
        assert_eq!(r.error.as_deref(), Some("file too large (max 50MB for file)"));
    }

    /// Signed out: owed, waiting, and saying why.
    #[tokio::test]
    async fn signed_out_it_waits() {
        let tmp = tempfile::tempdir().unwrap();
        let (records, file, r) = landed(tmp.path(), b"abc");
        let r = attempt(None, &records, &file, r).await;
        assert_eq!(r.state, SyncState::Pending);
        assert_eq!(r.error.as_deref(), Some("not signed in to NeboAI"));
        assert!(r.next_attempt > now());
    }

    /// Transport failures count, and after the last try the copy is failed.
    #[tokio::test]
    async fn repeated_failures_end_failed() {
        let api = comm::api::NeboAIApi::new("http://127.0.0.1:9".into(), "bot".into(), "token".into());
        let tmp = tempfile::tempdir().unwrap();
        let (records, file, mut r) = landed(tmp.path(), b"abc");
        r.attempts = MAX_ATTEMPTS - 1;
        let r = attempt(Some(&api), &records, &file, r).await;
        assert_eq!(r.state, SyncState::Failed);
        assert!(r.error.is_some());
    }

    #[test]
    fn the_wait_grows_to_an_hour() {
        assert_eq!(backoff(1), 10);
        assert_eq!(backoff(2), 30);
        assert_eq!(backoff(3), 90);
        assert_eq!(backoff(20), 3600);
    }
}
