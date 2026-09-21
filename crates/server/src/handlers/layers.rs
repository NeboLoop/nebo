//! The layers screen: the owner reads and writes the industry, franchise and
//! company packs on this Nebo, and says when the workforce learns them.
//!
//! Editing a layer is not a decision to teach forty-eight seats. Every write
//! here parks: [`crate::layers_update::detect_changes`] records one pending entry
//! per pack and raises nothing, and `POST /layers/apply` is the owner saying now.
//! The one exception is an install, which is already explicit — see
//! [`crate::handlers::org::install_org`].
//!
//! **The company layer is the owner's and is not locked.** Every endpoint here
//! works on it. What the company's own law reserves to the owner is an employee
//! rewriting the company layer unattended from inside a workflow; these
//! endpoints are the owner acting, which is the other side of that same law.
//!
//! Two things are refused without exception: a path that leaves the packs
//! directory, and a change that would stop the pack loading. The second is not
//! checked here — every change lands through [`napp::commit_change`], the one
//! gate a pack passes on any path, which stages the change and lets the real
//! loader refuse it before a byte of it reaches the pack the seats work from.

use std::path::{Path, PathBuf};

use axum::extract::{FromRequest, Multipart, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use tracing::info;
use types::NeboError;

use crate::handlers::{to_error_response, HandlerResult};
use crate::layers_update;
use crate::state::AppState;

// ------------------------------------------------------------- requests

#[derive(Debug, Deserialize)]
pub struct PathQuery {
    pub path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteLayerFileRequest {
    /// Relative to the pack root: `laws/payments.md`.
    pub path: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyLayersRequest {
    /// The packs to apply; every parked pack when absent.
    #[serde(default)]
    pub slugs: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadLayerRequest {
    /// An absolute path to a pack directory on this machine.
    pub path: String,
}

// ------------------------------------------------------------ responses

/// One pack installed on this Nebo.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerPack {
    /// `industry` | `franchise` | `company`.
    pub layer: String,
    pub slug: String,
    pub name: String,
    pub version: String,
    pub stamp: String,
    pub file_count: usize,
    pub updated_at: i64,
}

/// One edit the owner has made that no seat has read yet.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingLayerEntry {
    pub layer: String,
    pub slug: String,
    pub name: String,
    /// `added` | `changed` | `removed`.
    pub kind: String,
    pub stamp: String,
    /// The stamp the seats' sections are written against today.
    pub previous_stamp: Option<String>,
    /// What a seat will read on apply, and it has three shapes: a unified diff
    /// when a pack changed, the whole pack when one was added, one line of prose
    /// when one was removed.
    pub diff: String,
    pub detected_at: i64,
}

impl From<&layers_update::PendingLayer> for PendingLayerEntry {
    fn from(p: &layers_update::PendingLayer) -> Self {
        Self {
            layer: p.layer.clone(),
            slug: p.slug.clone(),
            name: p.name.clone(),
            kind: p.kind.clone(),
            stamp: p.stamp.clone(),
            previous_stamp: p.previous_stamp.clone(),
            diff: p.diff.clone(),
            detected_at: p.detected_at,
        }
    }
}

/// Where the workforce stands on reading the layers.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerSeatTally {
    pub total: usize,
    pub written: usize,
    pub pending: usize,
    pub stale: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayersResponse {
    pub packs: Vec<LayerPack>,
    pub pending: Vec<PendingLayerEntry>,
    pub seats: LayerSeatTally,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerFile {
    /// Relative to the pack root.
    pub path: String,
    pub bytes: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerFilesResponse {
    pub files: Vec<LayerFile>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerFileContent {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerWriteResponse {
    pub ok: bool,
    /// The pack's parked entry as it now stands.
    pub pending: Option<PendingLayerEntry>,
}

/// How many entries the loader read in each typed folder of an uploaded pack.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerFolderCounts {
    pub vocabulary: usize,
    pub parties: usize,
    pub rules: usize,
    pub laws: usize,
    pub standards: usize,
    pub workflows: usize,
    pub reference: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerUploadResponse {
    pub layer: String,
    pub slug: String,
    pub name: String,
    pub version: String,
    pub counts: LayerFolderCounts,
    pub pending: Option<PendingLayerEntry>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerApplyResponse {
    pub applied: Vec<String>,
    /// How many seats got an update run.
    pub seats: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerSeat {
    pub id: String,
    pub name: String,
    /// `written` | `pending` | `stale`.
    pub status: String,
    /// The layer stamp this seat's section is written against.
    pub against: String,
    pub at: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerSeatsResponse {
    pub seats: Vec<LayerSeat>,
}

// ---------------------------------------------------------------- paths

fn bad(msg: impl Into<String>) -> (reqwest::StatusCode, Json<types::api::ErrorResponse>) {
    to_error_response(NeboError::Validation(msg.into()))
}

fn io(e: std::io::Error) -> (reqwest::StatusCode, Json<types::api::ErrorResponse>) {
    to_error_response(NeboError::Internal(e.to_string()))
}

/// `<packs_dir>/<slug>`. A slug names one directory under `packs/` and nothing
/// else: no separator, no dot-directory (which is where `commit_change` stages),
/// no drive letter.
fn pack_dir(slug: &str) -> Result<PathBuf, NeboError> {
    let slug = slug.trim();
    if slug.is_empty()
        || slug.starts_with('.')
        || slug.contains('/')
        || slug.contains('\\')
        || slug.contains(':')
    {
        return Err(NeboError::Validation(format!("`{slug}` is not a layer slug")));
    }
    let dir = config::packs_dir()?.join(slug);
    if !dir.is_dir() {
        // The crate's 404 carries no message; the slug is in the request.
        return Err(NeboError::NotFound);
    }
    Ok(dir)
}

/// A path inside the pack and nowhere else.
///
/// Refused: an absolute path, a drive letter, any `..` or `.` segment, an empty
/// segment, a `skills/` folder or a `SKILL.md` of any casing (a pack is
/// knowledge, never procedure — the loader refuses it too), and anything whose
/// real location resolves outside the pack, which is how a symlink would try.
///
/// The path comes back relative to the pack root, because that is what a change
/// staged through [`napp::commit_change`] is written against.
fn resolve_in_pack(pack: &Path, rel: &str) -> Result<PathBuf, NeboError> {
    let rel = rel.trim();
    let escape = || NeboError::Validation(format!("`{rel}` is not a path inside this layer"));
    if rel.is_empty() || rel.starts_with('/') || rel.starts_with('\\') || rel.contains(':') {
        return Err(escape());
    }
    let mut safe = PathBuf::new();
    for part in rel.split(['/', '\\']) {
        if part.is_empty() || part == "." || part == ".." {
            return Err(escape());
        }
        if part.eq_ignore_ascii_case("SKILL.md") || part.eq_ignore_ascii_case("skills") {
            return Err(NeboError::Validation(
                "a pack never contains a skill: a layer is knowledge, never procedure".into(),
            ));
        }
        safe.push(part);
    }
    // The deepest part of the path that exists must resolve inside the pack, so a
    // symlink cannot be used as a door out of it.
    let root = pack.canonicalize().map_err(|e| NeboError::Internal(e.to_string()))?;
    let mut probe = pack.join(&safe);
    loop {
        match probe.canonicalize() {
            Ok(real) => {
                if !real.starts_with(&root) {
                    return Err(escape());
                }
                break;
            }
            Err(_) => match probe.parent() {
                Some(parent) if parent != probe => probe = parent.to_path_buf(),
                _ => return Err(escape()),
            },
        }
    }
    Ok(safe)
}

/// Every file in the pack, by path relative to its root, sorted.
fn pack_files(pack: &Path) -> Vec<LayerFile> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<LayerFile>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, root, out);
            } else if let Ok(meta) = entry.metadata() {
                let rel = path.strip_prefix(root).unwrap_or(&path);
                out.push(LayerFile {
                    path: rel.to_string_lossy().replace('\\', "/"),
                    bytes: meta.len(),
                });
            }
        }
    }
    let mut out = Vec::new();
    walk(pack, pack, &mut out);
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// The newest mtime in the pack, as unix seconds.
fn updated_at(pack: &Path) -> i64 {
    fn newest(dir: &Path, best: &mut i64) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                newest(&path, best);
            } else if let Ok(t) = entry.metadata().and_then(|m| m.modified()) {
                if let Ok(d) = t.duration_since(std::time::UNIX_EPOCH) {
                    *best = (*best).max(d.as_secs() as i64);
                }
            }
        }
    }
    let mut best = 0;
    newest(pack, &mut best);
    best
}

// ---------------------------------------------------------------- reads

/// GET /layers — the packs on disk, the edits parked against them, and where the
/// workforce stands on reading them.
pub async fn list_layers(State(state): State<AppState>) -> HandlerResult<LayersResponse> {
    let packs_dir = config::packs_dir().map_err(to_error_response)?;
    let _ = std::fs::create_dir_all(&packs_dir);
    let packs: Vec<LayerPack> = napp::scan_packs(&packs_dir)
        .into_iter()
        .map(|p| {
            let dir = packs_dir.join(&p.slug);
            LayerPack {
                layer: p.layer.as_str().to_string(),
                stamp: p.stamp(),
                slug: p.slug,
                name: p.name,
                version: p.version,
                file_count: pack_files(&dir).len(),
                updated_at: updated_at(&dir),
            }
        })
        .collect();
    let pending: Vec<PendingLayerEntry> = state
        .pending_layers
        .read()
        .await
        .iter()
        .map(PendingLayerEntry::from)
        .collect();
    let seats = seat_rows(&state).await;
    let mut tally = LayerSeatTally { total: seats.len(), written: 0, pending: 0, stale: 0 };
    for s in &seats {
        match s.status.as_str() {
            "written" => tally.written += 1,
            "stale" => tally.stale += 1,
            _ => tally.pending += 1,
        }
    }
    Ok(Json(LayersResponse { packs, pending, seats: tally }))
}

/// GET /layers/{slug}/files
pub async fn list_layer_files(
    axum::extract::Path(slug): axum::extract::Path<String>,
) -> HandlerResult<LayerFilesResponse> {
    let dir = pack_dir(&slug).map_err(to_error_response)?;
    Ok(Json(LayerFilesResponse { files: pack_files(&dir) }))
}

/// GET /layers/{slug}/file?path=<rel>
pub async fn read_layer_file(
    axum::extract::Path(slug): axum::extract::Path<String>,
    Query(q): Query<PathQuery>,
) -> HandlerResult<LayerFileContent> {
    let dir = pack_dir(&slug).map_err(to_error_response)?;
    let rel = resolve_in_pack(&dir, &q.path).map_err(to_error_response)?;
    let content = std::fs::read_to_string(dir.join(&rel))
        .map_err(|_| to_error_response(NeboError::NotFound))?;
    Ok(Json(LayerFileContent { path: q.path, content }))
}

// ---------------------------------------------------------------- writes

/// PUT /layers/{slug}/file — the owner's assertion: the save is the change.
///
/// It lands through the one gate every pack change passes, so a save that would
/// stop the pack loading is refused with the loader's own message and the file
/// that stood survives. Then it parks: no seat, law, standard or company policy
/// moves until the owner applies.
pub async fn write_layer_file(
    State(state): State<AppState>,
    axum::extract::Path(slug): axum::extract::Path<String>,
    Json(body): Json<WriteLayerFileRequest>,
) -> HandlerResult<LayerWriteResponse> {
    let dir = pack_dir(&slug).map_err(to_error_response)?;
    let rel = resolve_in_pack(&dir, &body.path).map_err(to_error_response)?;
    napp::commit_change(&dir, |staged| {
        napp::copy_tree(&dir, staged)?;
        let target = staged.join(&rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(target, &body.content)?;
        Ok(())
    })
    .map_err(|e| bad(e.to_string()))?;
    info!(slug = %slug, path = %body.path, "layers: written and parked");
    Ok(Json(LayerWriteResponse { ok: true, pending: park_now(&state, &slug).await }))
}

/// DELETE /layers/{slug}/file?path=<rel>
pub async fn delete_layer_file(
    State(state): State<AppState>,
    axum::extract::Path(slug): axum::extract::Path<String>,
    Query(q): Query<PathQuery>,
) -> HandlerResult<LayerWriteResponse> {
    let dir = pack_dir(&slug).map_err(to_error_response)?;
    let rel = resolve_in_pack(&dir, &q.path).map_err(to_error_response)?;
    if !dir.join(&rel).is_file() {
        return Err(to_error_response(NeboError::NotFound));
    }
    // A delete goes through the same gate: taking the marker out would leave a
    // directory that is not a pack at all.
    napp::commit_change(&dir, |staged| {
        napp::copy_tree(&dir, staged)?;
        std::fs::remove_file(staged.join(&rel))?;
        Ok(())
    })
    .map_err(|e| bad(e.to_string()))?;
    info!(slug = %slug, path = %q.path, "layers: removed and parked");
    Ok(Json(LayerWriteResponse { ok: true, pending: park_now(&state, &slug).await }))
}

/// Rescan and park now rather than waiting on the watcher's debounce, so the
/// owner's screen shows their own edit the moment they made it. The watcher's
/// later pass finds the same thing and replaces the same one entry.
async fn park_now(state: &AppState, slug: &str) -> Option<PendingLayerEntry> {
    let packs_dir = config::packs_dir().ok()?;
    layers_update::detect_changes(state, &packs_dir).await;
    state
        .pending_layers
        .read()
        .await
        .iter()
        .find(|p| p.slug == slug)
        .map(PendingLayerEntry::from)
}

// ---------------------------------------------------------------- upload

/// POST /layers/upload — a pack as a zip (multipart, field `file`) or as the path
/// of a directory on this machine (JSON `{ "path": "…" }`).
///
/// It is unpacked outside `packs/` and read by the real loader first, and it lands
/// through the same gate every other pack change does, so a pack that does not
/// load never reaches the directory the seats work from. What the loader found
/// comes back: the layer, the slug, and the counts per typed folder.
pub async fn upload_layer_pack(
    State(state): State<AppState>,
    req: axum::extract::Request,
) -> HandlerResult<LayerUploadResponse> {
    let content_type = req
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let staging = tempfile::tempdir().map_err(io)?;
    let staged = staging.path().join("upload");
    // The pack's own name: the source folder for a path upload, the wrapping
    // folder for a zip, else the marker's frontmatter. The staging folder is
    // never the name, or every uploaded pack would be called `upload` and each
    // would replace the last.
    let mut named: Option<String> = None;
    std::fs::create_dir_all(&staged).map_err(io)?;

    if content_type.starts_with("multipart/") {
        let mut multipart = Multipart::from_request(req, &state)
            .await
            .map_err(|e| bad(e.to_string()))?;
        // A pack over the ceiling is a file that is too big, not a broken
        // request — the same sentence the file door gives, from the same number.
        let max_upload_bytes = state.config.runtime.max_upload_bytes();
        let too_big = |e| crate::handlers::files::too_big_or_bad(max_upload_bytes, e);
        let mut bytes: Vec<u8> = Vec::new();
        while let Some(field) = multipart.next_field().await.map_err(too_big)? {
            if field.name() == Some("file") {
                bytes = field.bytes().await.map_err(too_big)?.to_vec();
            }
        }
        if bytes.is_empty() {
            return Err(bad("no pack was uploaded: send the zip as the `file` field"));
        }
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))
            .map_err(|e| bad(format!("that is not a readable zip: {e}")))?;
        zip.extract(&staged)
            .map_err(|e| bad(format!("the zip could not be unpacked: {e}")))?;
    } else {
        let Json(body) = Json::<UploadLayerRequest>::from_request(req, &state)
            .await
            .map_err(|e| bad(e.to_string()))?;
        let src = PathBuf::from(body.path.trim());
        if !src.is_dir() {
            return Err(bad(format!("{} is not a directory", src.display())));
        }
        napp::copy_tree(&src, &staged).map_err(|e| bad(e.to_string()))?;
        named = src.file_name().and_then(|n| n.to_str()).map(str::to_string);
    }

    // A symlink in an uploaded pack is a door out of it; a pack is markdown.
    if let Some(link) = first_symlink(&staged) {
        return Err(bad(format!(
            "{} is a symbolic link; a pack is markdown files and nothing else",
            link.strip_prefix(&staged).unwrap_or(&link).display()
        )));
    }

    // The pack root is the unpacked directory itself, or its single child when the
    // zip carried one wrapping folder.
    let root = pack_root(&staged).ok_or_else(|| {
        bad("no marker file: a pack holds exactly one of INDUSTRY.md, FRANCHISE.md, COMPANY.md")
    })?;
    let slug = named
        .or_else(|| {
            (root != staged)
                .then(|| root.file_name().and_then(|n| n.to_str()).map(str::to_string))
                .flatten()
        })
        .or_else(|| super::org::pack_slug(&root))
        .filter(|n| !n.starts_with('.') && !n.is_empty())
        .ok_or_else(|| {
            bad("the pack has no name: zip it inside a folder named for it, or set `slug:` in its marker")
        })?;

    let packs_dir = config::packs_dir().map_err(to_error_response)?;
    std::fs::create_dir_all(&packs_dir).map_err(io)?;
    let pack = napp::commit_change(&packs_dir.join(&slug), |target| {
        napp::copy_tree(&root, target)
    })
    .map_err(|e| bad(e.to_string()))?;

    let count = |folder: &str| {
        std::fs::read_dir(root.join(folder))
            .map(|d| d.flatten().filter(|e| e.path().is_file()).count())
            .unwrap_or(0)
    };
    let counts = LayerFolderCounts {
        vocabulary: count("vocabulary"),
        parties: count("parties"),
        rules: count("rules"),
        laws: count("laws"),
        standards: count("standards"),
        workflows: count("workflows"),
        reference: count("reference"),
    };
    info!(layer = pack.layer.as_str(), slug = %pack.slug, "layers: pack unpacked, loaded and installed");
    let pending = park_now(&state, &pack.slug).await;
    Ok(Json(LayerUploadResponse {
        layer: pack.layer.as_str().to_string(),
        slug: pack.slug,
        name: pack.name,
        version: pack.version,
        counts,
        pending,
    }))
}

/// The directory that holds a marker: the unpacked root, else its single child.
fn pack_root(staged: &Path) -> Option<PathBuf> {
    let has_marker = |d: &Path| napp::pack::MARKERS.iter().any(|(m, _)| d.join(m).is_file());
    if has_marker(staged) {
        return Some(staged.to_path_buf());
    }
    let children: Vec<PathBuf> = std::fs::read_dir(staged)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && !p.file_name().is_some_and(|n| n == "__MACOSX"))
        .collect();
    children.into_iter().find(|c| has_marker(c))
}

fn first_symlink(dir: &Path) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_symlink()) {
            return Some(path);
        }
        if path.is_dir() {
            if let Some(found) = first_symlink(&path) {
                return Some(found);
            }
        }
    }
    None
}

// ---------------------------------------------------------------- apply

/// POST /layers/apply — the owner says now.
///
/// One routine applies a layer change on every path (CODE_AUDITOR 8.1):
/// [`layers_update::apply_pending`]. This endpoint calls it directly; an org
/// install reaches it through [`layers_update::detect_and_apply`].
pub async fn apply_layers(
    State(state): State<AppState>,
    body: Option<Json<ApplyLayersRequest>>,
) -> HandlerResult<LayerApplyResponse> {
    let slugs = body.and_then(|Json(b)| b.slugs);
    let (applied, seats) = layers_update::apply_pending(&state, slugs).await;
    Ok(Json(LayerApplyResponse { applied, seats }))
}

// ---------------------------------------------------------------- seats

/// GET /layers/seats — where each seat stands, read from its context stamp.
pub async fn list_layer_seats(State(state): State<AppState>) -> HandlerResult<LayerSeatsResponse> {
    Ok(Json(LayerSeatsResponse { seats: seat_rows(&state).await }))
}

/// One row per seat: `written` when its section is written against a layer that
/// is still current, `stale` when it is written against one that has moved on,
/// and `pending` when a run is out or the seat has never read the layers at all.
async fn seat_rows(state: &AppState) -> Vec<LayerSeat> {
    let current: std::collections::HashSet<String> = state
        .packs
        .read()
        .await
        .values()
        .map(napp::Pack::stamp)
        .collect();
    let mut out = Vec::new();
    for seat in state
        .store
        .list_agents(10_000, 0)
        .unwrap_or_default()
        .into_iter()
        .filter(|a| a.is_enabled == 1 && a.is_app.unwrap_or(0) == 0)
    {
        let stamp: serde_json::Value = seat
            .context_stamp
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(serde_json::Value::Null);
        let against = stamp.get("against").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let written = stamp.get("status").and_then(|v| v.as_str()) == Some("written");
        // A seat's `against` is one stamp, or the comma-joined set it read on its
        // first day. It is current only when every stamp in it still is.
        let fresh = !against.is_empty()
            && against
                .split(',')
                .all(|s| s.trim().is_empty() || current.contains(s.trim()));
        let status = match (written, fresh) {
            (true, true) => "written",
            (true, false) => "stale",
            _ => "pending",
        };
        out.push(LayerSeat {
            id: seat.id,
            name: seat.name,
            status: status.to_string(),
            against,
            at: stamp
                .get("written_at")
                .or_else(|| stamp.get("launched_at"))
                .and_then(|v| v.as_i64()),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn company(root: &Path) -> PathBuf {
        let d = root.join("acme");
        write(
            &d,
            "COMPANY.md",
            "---\ntype: company\ncompany: Acme\nversion: 1.0.0\n---\n\n# Acme\n\nFix roofs and get paid.\n",
        );
        write(&d, "laws/pay.md", "---\nlaw: Payments\nceiling: [\"ledger.payment.send\"]\n---\n\nNo seat pays unattended.\n");
        d
    }

    /// A path that leaves the packs directory is refused, however it is spelled.
    #[test]
    fn a_path_out_of_the_pack_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let pack = company(tmp.path());
        // What the owner legitimately edits.
        assert_eq!(resolve_in_pack(&pack, "laws/pay.md").unwrap(), PathBuf::from("laws/pay.md"));
        assert!(resolve_in_pack(&pack, "standards/new.md").is_ok(), "a new file is fine");
        assert!(resolve_in_pack(&pack, "COMPANY.md").is_ok(), "the company layer is the owner's");
        for spelling in [
            "../settings.json",
            "laws/../../settings.json",
            "/etc/passwd",
            "C:\\Windows\\win.ini",
            "laws\\..\\..\\settings.json",
            "./laws/pay.md",
            "",
            "   ",
        ] {
            assert!(
                resolve_in_pack(&pack, spelling).is_err(),
                "`{spelling}` must be refused"
            );
        }
        // A pack is knowledge, never procedure — the loader refuses a skill and so
        // does the write path, before it lands.
        assert!(resolve_in_pack(&pack, "SKILL.md").is_err());
        assert!(resolve_in_pack(&pack, "reference/skill.MD").is_err());
        assert!(resolve_in_pack(&pack, "skills/x.md").is_err());
    }

    /// A symlink is not a door out of the pack either.
    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_the_pack_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let pack = company(tmp.path());
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, pack.join("away")).unwrap();
        assert!(resolve_in_pack(&pack, "away/secrets.md").is_err());
    }

    /// The staging directory `commit_change` writes beside a pack must never be
    /// addressable as a layer of its own.
    #[test]
    fn a_slug_is_one_directory_name_and_never_a_dot_directory() {
        for slug in ["", " ", ".", "..", ".staging-acme", "a/b", "a\\b", "C:x"] {
            assert!(
                matches!(pack_dir(slug), Err(NeboError::Validation(_))),
                "`{slug}` must be refused as a slug"
            );
        }
    }

    /// The pack root is found whether the zip wrapped it in a folder or not.
    #[test]
    fn the_pack_root_is_the_directory_with_the_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let pack = company(tmp.path());
        assert_eq!(pack_root(&pack).unwrap(), pack);
        assert_eq!(pack_root(tmp.path()).unwrap(), pack);
        let empty = tmp.path().join("nothing");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(pack_root(&empty).is_none());
    }

    /// The gate, from the endpoint's side: a write that would stop the pack
    /// loading is refused with the loader's own message and the pack that stands
    /// is exactly as it was. Every path lands through `napp::commit_change`, so
    /// this is the same gate a seat's `pack` tool and an org install pass.
    #[test]
    fn a_write_that_breaks_the_loader_is_refused_and_the_pack_survives() {
        let tmp = tempfile::tempdir().unwrap();
        let pack = company(tmp.path());
        let law = pack.join("laws/pay.md");
        let before = std::fs::read_to_string(&law).unwrap();

        // A second marker makes the directory two packs at once.
        let rel = resolve_in_pack(&pack, "INDUSTRY.md").unwrap();
        let err = napp::commit_change(&pack, |staged| {
            napp::copy_tree(&pack, staged)?;
            std::fs::write(staged.join(&rel), "---\nindustry: roofing\n---\n\n# Roofing\n")?;
            Ok(())
        })
        .expect_err("two markers is not a pack");
        assert!(err.to_string().contains("more than one marker"), "{err}");
        assert!(!pack.join("INDUSTRY.md").exists(), "the refused write never landed");

        // Frontmatter that carries no structure at all.
        let rel = resolve_in_pack(&pack, "laws/pay.md").unwrap();
        let err = napp::commit_change(&pack, |staged| {
            napp::copy_tree(&pack, staged)?;
            std::fs::write(staged.join(&rel), "---\n[ unclosed and not a mapping\n---\n\nbody\n")?;
            Ok(())
        })
        .expect_err("frontmatter that carries no structure must be refused");
        assert!(err.to_string().contains("pay.md"), "the loader names the file: {err}");

        // Taking the marker out leaves a directory that is not a pack.
        let marker = resolve_in_pack(&pack, "COMPANY.md").unwrap();
        let err = napp::commit_change(&pack, |staged| {
            napp::copy_tree(&pack, staged)?;
            std::fs::remove_file(staged.join(&marker))?;
            Ok(())
        })
        .expect_err("a pack needs its marker");
        assert!(err.to_string().contains("no marker file"), "{err}");

        assert_eq!(std::fs::read_to_string(&law).unwrap(), before);
        assert!(pack.join("COMPANY.md").is_file(), "the refused delete never happened");

        // And the write that does load lands.
        napp::commit_change(&pack, |staged| {
            napp::copy_tree(&pack, staged)?;
            std::fs::write(staged.join(&rel), "---\nlaw: Payments\nceiling: []\n---\n\nChanged.\n")?;
            Ok(())
        })
        .expect("a loadable write is allowed");
        assert!(std::fs::read_to_string(&law).unwrap().contains("Changed."));
    }
}
