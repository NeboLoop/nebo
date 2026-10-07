//! The Work panel's sheet view: a workbook version as the grid's view model,
//! live edits to its unlocked inputs, and a save that appends a new version.
//!
//! Edits live in an in-memory session per grid (the client names it), over
//! one version of the document. Every edit and save carries that version; once
//! the document has moved on (an employee wrote a newer one, another grid
//! saved), the session is stale and is refused with 409.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use types::NeboError;
use types::api::ErrorResponse;

use super::to_error_response;
use crate::state::AppState;

/// A session nobody has touched for this long is dropped (its unsaved edits with it).
const IDLE: Duration = Duration::from_secs(30 * 60);

pub struct Session {
    document_id: String,
    version: i64,
    book: sheet::Book,
    used: Instant,
}

/// Open edit sessions by the id the grid gave them.
pub type Sessions = Arc<Mutex<HashMap<String, Arc<Mutex<Session>>>>>;

type Refusal = (StatusCode, Json<ErrorResponse>);

fn refuse(status: StatusCode, error: impl Into<String>) -> Refusal {
    (
        status,
        Json(ErrorResponse {
            error: error.into(),
        }),
    )
}

fn stale(latest: i64) -> Refusal {
    refuse(
        StatusCode::CONFLICT,
        format!(
            "This sheet has changed since it was opened (it is now version {latest}). Reload it to keep editing."
        ),
    )
}

fn unreadable(e: sheet::Error) -> Refusal {
    to_error_response(NeboError::Validation(e.to_string()))
}

/// Run the engine off the async runtime: loading and recalculating a large
/// workbook takes long enough to stall other requests.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, Refusal> + Send + 'static,
) -> Result<T, Refusal> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("sheet task: {e}"))))?
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The document, refused unless it is a workbook.
fn workbook(state: &AppState, document_id: &str) -> Result<db::WorkDocument, Refusal> {
    let doc = state
        .store
        .get_work_document(document_id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(NeboError::NotFound))?;
    let name = doc.filename.to_ascii_lowercase();
    if !(name.ends_with(".xlsx") || name.ends_with(".xlsm")) {
        return Err(refuse(
            StatusCode::BAD_REQUEST,
            format!("{} is not an Excel workbook.", doc.filename),
        ));
    }
    Ok(doc)
}

/// The bytes of one version, read from its blob under `<data_dir>/files`.
async fn version_bytes(
    state: &AppState,
    document_id: &str,
    version: i64,
) -> Result<Vec<u8>, Refusal> {
    let found = state
        .store
        .list_work_versions(document_id)
        .map_err(to_error_response)?
        .into_iter()
        .find(|v| v.version_number == version)
        .ok_or_else(|| to_error_response(NeboError::NotFound))?;
    let rel = found
        .url
        .strip_prefix("/api/v1/files/")
        .ok_or_else(|| to_error_response(NeboError::NotFound))?;
    let files_root = config::data_dir().map_err(to_error_response)?.join("files");
    let (path, _) = super::files::within_files_root(&files_root, &files_root.join(rel))
        .await
        .ok_or_else(|| to_error_response(NeboError::NotFound))?;
    tokio::fs::read(&path)
        .await
        .map_err(|e| to_error_response(NeboError::Io(e)))
}

/// The session `id` over (document, version), opening the version when the
/// session is new. A session over another version of the document is stale:
/// `replace` (a fresh view load) starts it over, otherwise it is refused.
async fn session(
    state: &AppState,
    id: &str,
    document_id: &str,
    version: i64,
    replace: bool,
) -> Result<Arc<Mutex<Session>>, Refusal> {
    let existing = {
        let mut open = lock(&state.sheets);
        // A session busy with an edit is in use, not idle.
        open.retain(|_, s| s.try_lock().map_or(true, |s| s.used.elapsed() < IDLE));
        open.get(id).cloned()
    };
    if let Some(existing) = existing {
        let mut s = lock(&existing);
        if s.document_id == document_id && s.version == version {
            s.used = Instant::now();
            drop(s);
            return Ok(existing);
        }
        if !replace {
            return Err(stale(version));
        }
    }
    let bytes = version_bytes(state, document_id, version).await?;
    let book = blocking(move || sheet::Book::open(bytes).map_err(unreadable)).await?;
    let session = Arc::new(Mutex::new(Session {
        document_id: document_id.to_string(),
        version,
        book,
        used: Instant::now(),
    }));
    lock(&state.sheets).insert(id.to_string(), session.clone());
    Ok(session)
}

#[derive(Deserialize)]
pub struct ViewQuery {
    /// Default: the latest version.
    pub version: Option<i64>,
    /// `a-b`: only cells in rows a through b (1-based, inclusive).
    pub rows: Option<String>,
    /// Only this sheet's cells; the other sheets come without cells.
    pub sheet: Option<String>,
    /// The grid's edit session, to show its edits so far. Without one, a new
    /// session starts and its id comes back in the view.
    pub session: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SheetResponse {
    pub document_id: String,
    pub version: i64,
    pub session: String,
    #[serde(flatten)]
    pub view: sheet::View,
}

/// GET /api/v1/work/{documentId}/sheet?version=N&rows=a-b&session=…
pub async fn view(
    State(state): State<AppState>,
    Path(document_id): Path<String>,
    Query(q): Query<ViewQuery>,
) -> Result<Json<SheetResponse>, Refusal> {
    let doc = workbook(&state, &document_id)?;
    let version = q.version.unwrap_or(doc.latest_version);
    let rows = match q.rows.as_deref() {
        None => None,
        Some(spec) => {
            let range = spec.split_once('-').and_then(|(a, b)| {
                Some(a.trim().parse::<i32>().ok()?..=b.trim().parse::<i32>().ok()?)
            });
            Some(range.ok_or_else(|| {
                refuse(
                    StatusCode::BAD_REQUEST,
                    format!("rows must look like 1-500, not \"{spec}\""),
                )
            })?)
        }
    };
    let id = q
        .session
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let session = session(&state, &id, &document_id, version, true).await?;
    let view = blocking(move || Ok(lock(&session).book.view(q.sheet.as_deref(), rows))).await?;
    Ok(Json(SheetResponse {
        document_id,
        version,
        session: id,
        view,
    }))
}

#[derive(Deserialize)]
pub struct EditRequest {
    pub session: String,
    /// The version the grid shows; refused with 409 once it is not the latest.
    pub version: i64,
    pub edits: Vec<sheet::Edit>,
}

/// POST /api/v1/work/{documentId}/sheet/edit
pub async fn edit(
    State(state): State<AppState>,
    Path(document_id): Path<String>,
    Json(req): Json<EditRequest>,
) -> Result<Json<sheet::Edited>, Refusal> {
    let doc = workbook(&state, &document_id)?;
    if req.version != doc.latest_version {
        return Err(stale(doc.latest_version));
    }
    let session = session(&state, &req.session, &document_id, req.version, false).await?;
    let edited = blocking(move || Ok(lock(&session).book.edit(&req.edits))).await?;
    Ok(Json(edited))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveRequest {
    pub session: String,
    /// Who the "Saved …" message in the chat is from, and the conversation
    /// it lands in (as `restore_version` takes them).
    #[serde(default)]
    pub agent_id: String,
    pub session_key: Option<String>,
}

#[derive(Serialize)]
pub struct Saved {
    pub version: i64,
}

/// POST /api/v1/work/{documentId}/sheet/save — the session's edits as a new
/// version (the original file with the changed cells patched in), announced
/// in the chat like a restore. Nothing changed: the current version, and no
/// new one.
pub async fn save(
    State(state): State<AppState>,
    Path(document_id): Path<String>,
    Json(req): Json<SaveRequest>,
) -> Result<Json<Saved>, Refusal> {
    let doc = workbook(&state, &document_id)?;
    let session = lock(&state.sheets).get(&req.session).cloned().ok_or_else(|| {
        refuse(
            StatusCode::NOT_FOUND,
            "These edits are no longer open (Nebo restarted, or they sat unsaved too long). Reload the sheet.",
        )
    })?;
    let store = state.store.clone();
    let files_dir = config::data_dir().map_err(to_error_response)?.join("files");
    let ext = doc
        .filename
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    let saved_doc = doc.clone();
    let added = blocking(move || {
        let mut s = lock(&session);
        if s.document_id != doc.id {
            return Err(to_error_response(NeboError::NotFound));
        }
        let latest = store
            .latest_work_version(&doc.id)
            .map_err(to_error_response)?;
        let latest_number = latest.as_ref().map_or(0, |v| v.version_number);
        if s.version != latest_number {
            return Err(stale(latest_number));
        }
        let Some(bytes) = s.book.save().map_err(unreadable)? else {
            return Ok(None);
        };
        let hash = hex::encode(Sha256::digest(&bytes));
        let url = tools::workspace_history::put_work_blob(&store, &files_dir, &hash, &ext, &bytes)
            .map_err(to_error_response)?;
        let added = store
            .add_work_version(
                &doc.id,
                latest.as_ref().map(|v| v.id.as_str()),
                &url,
                Some(&hash),
                None,
                None,
            )
            .map_err(to_error_response)?;
        s.book.commit(bytes);
        s.version = added.version_number;
        s.used = Instant::now();
        Ok(Some(added))
    })
    .await?;
    let Some(added) = added else {
        return Ok(Json(Saved {
            version: saved_doc.latest_version,
        }));
    };
    let content = format!(
        "Saved edits to {} as version {}",
        saved_doc.filename, added.version_number
    );
    crate::chat_dispatch::announce_work_version(
        &state,
        &saved_doc,
        &added,
        &content,
        &req.agent_id,
        req.session_key.as_deref(),
    );
    Ok(Json(Saved {
        version: added.version_number,
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};

    /// The sheet engine's fixture: inputs unlocked on a protected sheet,
    /// formulas across sheets, a chart, data validation, a calcChain.
    const FIXTURE: &[u8] = include_bytes!("../../../sheet/tests/fixtures/model.xlsx");

    fn changed(edited: &Value, at: &str) -> Value {
        edited["changed"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| {
                format!(
                    "{}!{}",
                    c["sheet"].as_str().unwrap(),
                    c["cell"].as_str().unwrap()
                ) == at
            })
            .cloned()
            .unwrap_or(Value::Null)
    }

    fn cell(view: &Value, sheet: usize, r: i64, c: i64) -> Value {
        view["sheets"][sheet]["cells"]
            .as_array()
            .unwrap()
            .iter()
            .find(|cell| cell["r"] == r && cell["c"] == c)
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// View, edit, save and the version guard, through the real server: a
    /// workbook version opens as the view model with a session, an input edit
    /// recalculates across sheets, a save appends a version announced in the
    /// chat, and a session or version left behind is refused with 409.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_sheet_is_viewed_edited_and_saved_as_a_new_version() {
        let nebo = crate::staffed_proof::session().await;
        let store = nebo.store().clone();
        let chat = format!("sheet-{}", uuid::Uuid::new_v4());
        store.create_chat(&chat, "Sheet").unwrap();
        let doc = store
            .upsert_work_document(&chat, "model.xlsx", "table")
            .unwrap();
        let hash = hex::encode(Sha256::digest(FIXTURE));
        let url = tools::workspace_history::put_work_blob(
            &store,
            &nebo.home.join("files"),
            &hash,
            "xlsx",
            FIXTURE,
        )
        .unwrap();
        store
            .add_work_version(&doc.id, None, &url, Some(&hash), None, None)
            .unwrap();
        let base = format!("/work/{}/sheet", doc.id);

        let (status, view) = nebo.get(&base).await;
        assert_eq!(status, 200, "{view}");
        assert_eq!(view["documentId"], doc.id.as_str());
        assert_eq!(view["version"], 1);
        assert_eq!(view["sheets"].as_array().unwrap().len(), 3);
        let session = view["session"].as_str().expect("a session id").to_string();
        assert_eq!(cell(&view, 2, 3, 3)["value"], 2700.0);

        let (status, page) = nebo
            .get(&format!("{base}?sheet=Model&rows=3-3&session={session}"))
            .await;
        assert_eq!(status, 200, "{page}");
        assert_eq!(page["sheets"][0]["cells"], json!([]));
        assert!(
            page["sheets"][2]["cells"]
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c["r"] == 3)
        );

        let edit = |version: i64, cell: &str| json!({ "session": session, "version": version, "edits": [{ "sheet": "Inputs", "cell": cell, "input": "40" }] });
        let (status, edited) = nebo.post(&format!("{base}/edit"), &edit(1, "B2")).await;
        assert_eq!(status, 200, "{edited}");
        assert_eq!(changed(&edited, "Model!C3")["value"], 3600.0);
        let (_, refused) = nebo.post(&format!("{base}/edit"), &edit(1, "A2")).await;
        assert!(
            refused["errors"][0]["error"]
                .as_str()
                .unwrap()
                .contains("locked"),
            "{refused}"
        );
        let (status, _) = nebo.post(&format!("{base}/edit"), &edit(7, "B2")).await;
        assert_eq!(status, 409);

        // The session's edits show in its view.
        let (_, view) = nebo.get(&format!("{base}?session={session}")).await;
        assert_eq!(cell(&view, 0, 2, 2)["display"], "$40");

        let (status, saved) = nebo
            .post(
                &format!("{base}/save"),
                &json!({ "session": session, "agentId": "a" }),
            )
            .await;
        assert_eq!(status, 200, "{saved}");
        assert_eq!(saved["version"], 2);
        let messages = store.get_chat_messages(&chat).unwrap();
        assert!(
            messages
                .iter()
                .any(|m| m.content == "Saved edits to model.xlsx as version 2")
        );

        // A fresh view of the latest version reads the saved file.
        let (_, latest) = nebo.get(&base).await;
        assert_eq!(latest["version"], 2);
        assert_eq!(cell(&latest, 2, 3, 3)["value"], 3600.0);
        assert_eq!(cell(&latest, 0, 2, 2)["editable"], true);

        // The session moved to version 2: nothing new to save, edits go on
        // against 2, and version 1 is stale.
        let (_, again) = nebo
            .post(&format!("{base}/save"), &json!({ "session": session }))
            .await;
        assert_eq!(again["version"], 2);
        let (status, _) = nebo.post(&format!("{base}/edit"), &edit(1, "B2")).await;
        assert_eq!(status, 409);
        let (status, _) = nebo.post(&format!("{base}/edit"), &edit(2, "B2")).await;
        assert_eq!(status, 200);

        // Someone else's version lands: the session's save is refused.
        store.restore_work_version(&doc.id, 1).unwrap();
        let (status, stale) = nebo
            .post(&format!("{base}/save"), &json!({ "session": session }))
            .await;
        assert_eq!(status, 409, "{stale}");
    }
}
