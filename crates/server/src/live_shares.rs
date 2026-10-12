//! Links that follow their file. A Work-panel file shared by link
//! (`handlers::neboai::set_share_link`) is live unless the owner chose
//! "this version only": each new version of the file goes behind the same
//! link. The hub keeps, on every link, the hash of the bytes it opens, so
//! a version goes up only when the file on disk hashes differently.
//!
//! Two triggers, one push: a change under `<data_dir>/files` pushes that
//! file at once when it has a live link, and a sweep every few minutes
//! pushes any live link whose file changed while the hub was out of reach
//! (and learns of links made from the phone).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};

use comm::api::NeboAIApi;
use comm::api_types::{FileShare, FileShareSettings};
use notify::{EventKind, RecursiveMode, Watcher};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::state::AppState;

const SWEEP_EVERY: Duration = Duration::from_secs(5 * 60);
/// A file being written fires many events; push once it settles.
const SETTLE: Duration = Duration::from_secs(2);

/// The hash a link keeps for the bytes it opens.
pub(crate) fn content_hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Hashes by file, kept while its size and modified time stay the same, so
/// the sweep doesn't read a large file again and again.
static HASHES: LazyLock<Mutex<HashMap<PathBuf, (SystemTime, u64, String)>>> = LazyLock::new(Mutex::default);

/// The hash of the file on disk, or None when it can't be read.
pub(crate) async fn file_hash(path: &Path) -> Option<String> {
    let meta = tokio::fs::metadata(path).await.ok()?;
    let stamp = (meta.modified().ok()?, meta.len());
    if let Some((m, l, h)) = HASHES.lock().await.get(path)
        && (*m, *l) == stamp
    {
        return Some(h.clone());
    }
    let hash = content_hash(&tokio::fs::read(path).await.ok()?);
    HASHES.lock().await.insert(path.to_path_buf(), (stamp.0, stamp.1, hash.clone()));
    Some(hash)
}

/// Put the file's current bytes behind `share` when they differ from the
/// version it opens. Returns the link as it is now.
pub(crate) async fn push_version(
    state: &AppState,
    api: &NeboAIApi,
    share: &FileShare,
    path: &Path,
) -> Result<FileShare, String> {
    let hash = file_hash(path).await.ok_or_else(|| format!("read {}", path.display()))?;
    if hash == share.content_hash {
        return Ok(share.clone());
    }
    let uploaded = crate::chat_dispatch::upload_local_file(&state.comm_manager, path).await?;
    let settings = FileShareSettings {
        access: share.access.clone(),
        expires_at: share.expires_at.clone(),
        file_id: uploaded.file_id,
        content_hash: hash,
        ..Default::default()
    };
    api.update_file_share(&share.id, &settings).await.map_err(|e| e.to_string())
}

/// The Work-panel reference of a file under `files_dir`, as links name it.
fn artifact_of(files_dir: &Path, path: &Path) -> Option<String> {
    let parts: Option<Vec<&str>> = path.strip_prefix(files_dir).ok()?.iter().map(|c| c.to_str()).collect();
    let rel = parts?.join("/");
    (!rel.is_empty()).then(|| format!("/api/v1/files/{rel}"))
}

/// Live links by the file they follow: filled by the sweep, kept current by
/// the share dialog ([`remember`]).
static LIVE: LazyLock<Mutex<HashMap<String, FileShare>>> = LazyLock::new(Mutex::default);

/// The share dialog saved or turned off a link: follow its file or stop.
pub(crate) async fn remember(source: &str, share: Option<&FileShare>) {
    let mut live = LIVE.lock().await;
    match share {
        Some(s) if s.live => live.insert(source.to_string(), s.clone()),
        _ => live.remove(source),
    };
}

async fn sweep(state: &AppState, files_dir: &Path) {
    let Ok(api) = crate::deps::build_api_client(state) else { return };
    let shares = match api.file_shares("").await {
        Ok(s) => s,
        Err(e) => {
            debug!(error = %e, "live shares: list failed");
            return;
        }
    };
    let mut next = HashMap::new();
    for share in shares.into_iter().filter(|s| s.live && !s.source.is_empty()) {
        let Some(path) = crate::chat_dispatch::artifact_local_path(files_dir, &share.source) else { continue };
        let share = match push_version(state, &api, &share, &path).await {
            Ok(s) => s,
            Err(e) => {
                debug!(source = %share.source, error = %e, "live shares: push failed");
                share
            }
        };
        next.insert(share.source.clone(), share);
    }
    *LIVE.lock().await = next;
}

async fn push_changed(state: &AppState, files_dir: &Path, path: &Path) {
    let Some(artifact) = artifact_of(files_dir, path) else { return };
    let Some(share) = LIVE.lock().await.get(&artifact).cloned() else { return };
    let Ok(api) = crate::deps::build_api_client(state) else { return };
    match push_version(state, &api, &share, path).await {
        Ok(s) => {
            if s.content_hash != share.content_hash {
                info!(source = %artifact, "live shares: new version behind the link");
            }
            LIVE.lock().await.insert(artifact, s);
        }
        // The sweep tries again.
        Err(e) => debug!(source = %artifact, error = %e, "live shares: push failed"),
    }
}

/// Keep live links current, for as long as the server runs.
pub(crate) fn spawn(state: AppState) {
    tokio::spawn(async move {
        let files_dir = match config::files_dir() {
            Ok(d) => d,
            Err(e) => {
                warn!(error = %e, "live shares: no files folder");
                return;
            }
        };
        let _ = std::fs::create_dir_all(&files_dir);
        // Watchers report resolved paths.
        let files_dir = std::fs::canonicalize(&files_dir).unwrap_or(files_dir);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<PathBuf>();
        let watcher = notify::RecommendedWatcher::new(
            move |res: notify::Result<notify::Event>| {
                if let Ok(ev) = res
                    && matches!(ev.kind, EventKind::Create(_) | EventKind::Modify(_))
                {
                    for p in ev.paths {
                        let _ = tx.send(p);
                    }
                }
            },
            notify::Config::default(),
        );
        // Without a watcher the sweep still keeps links current, later.
        let _watcher = match watcher {
            Ok(mut w) => match w.watch(&files_dir, RecursiveMode::Recursive) {
                Ok(()) => Some(w),
                Err(e) => {
                    warn!(error = %e, "live shares: can't watch the files folder");
                    None
                }
            },
            Err(e) => {
                warn!(error = %e, "live shares: no file watcher");
                None
            }
        };

        let mut tick = tokio::time::interval(SWEEP_EVERY);
        let mut changed: Vec<PathBuf> = Vec::new();
        loop {
            tokio::select! {
                _ = tick.tick() => sweep(&state, &files_dir).await,
                Some(p) = rx.recv() => {
                    if !changed.contains(&p) {
                        changed.push(p);
                    }
                    // Settle: keep collecting until the folder is quiet.
                    while let Ok(Some(p)) = tokio::time::timeout(SETTLE, rx.recv()).await {
                        if !changed.contains(&p) {
                            changed.push(p);
                        }
                    }
                    for p in changed.drain(..) {
                        push_changed(&state, &files_dir, &p).await;
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_named_the_way_its_link_names_it() {
        let dir = Path::new("/data/files");
        assert_eq!(
            artifact_of(dir, Path::new("/data/files/Design Studio/Deck.html")).as_deref(),
            Some("/api/v1/files/Design Studio/Deck.html")
        );
        assert_eq!(artifact_of(dir, Path::new("/elsewhere/Deck.html")), None);
        assert_eq!(artifact_of(dir, dir), None);
    }

    #[test]
    fn the_hash_is_the_bytes_sha256() {
        assert_eq!(content_hash(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
