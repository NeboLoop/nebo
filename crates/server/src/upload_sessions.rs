//! Resumable uploads: a large file (a phone video) comes in as chunks.
//!
//! One request carrying a whole video has to finish inside the edge's body
//! and time limits, and a dropped connection loses all of it. Here the file
//! arrives as a session instead, in the shape tus uses: the client opens a
//! session with the file's size, sends chunks at offsets (`Upload-Offset`),
//! asks for the offset this side holds after any break, and completes with
//! the file's sha256. Each chunk streams to disk; nothing holds the file in
//! memory. A chunk sent twice is written twice at the same place, which is
//! why a retry is always safe.
//!
//! On disk, under `<data_dir>/upload-sessions/`: `<id>.json` (the session)
//! and `<id>.part` (the bytes so far). Completing moves the bytes into the
//! upload store (`agent::uploads`) under the session's id, which becomes the
//! attachment's id, and records the landing for the hub copy (`hub_sync`).
//! A session left untouched for a day is removed (`sweep`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

/// The chunk a client sends: small enough that one chunk crosses the edge
/// on a slow phone line well inside its time limit.
pub const CHUNK_SIZE: u64 = 8 * 1024 * 1024;

/// The largest file a session takes.
pub const MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// How long a session may sit untouched before it is removed.
const IDLE_EXPIRY_SECS: u64 = 24 * 60 * 60;

/// One upload in progress.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub upload_id: String,
    pub filename: String,
    pub mime_type: String,
    pub size: u64,
    /// Bytes held, from the start of the file, with no gap.
    pub offset: u64,
    #[serde(default)]
    pub agent_id: String,
    #[serde(default)]
    pub chat_id: String,
    /// Unix seconds of the last change.
    pub updated_at: u64,
}

/// Why a session call was refused.
#[derive(Debug)]
pub enum UploadError {
    /// No such session (never was, completed and gone, or expired).
    NotFound,
    /// The chunk starts past what is held: send from the session's offset.
    Conflict(Box<Session>),
    /// More than the file's size or a chunk's size.
    TooLarge(String),
    /// A malformed request.
    Invalid(String),
    /// The bytes do not hash to what the client said: the session is gone.
    HashMismatch,
    Io(String),
}

impl From<std::io::Error> for UploadError {
    fn from(e: std::io::Error) -> Self {
        UploadError::Io(e.to_string())
    }
}

/// The session store. `root` is the data directory; tests give a tempdir.
#[derive(Clone)]
pub struct Store {
    sessions: PathBuf,
    uploads: PathBuf,
}

/// One lock per session id, so two chunks of one upload never write at once.
static LOCKS: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn lock_for(id: &str) -> Arc<tokio::sync::Mutex<()>> {
    let mut locks = LOCKS.lock().unwrap_or_else(|p| p.into_inner());
    locks.entry(id.to_string()).or_default().clone()
}

fn forget_lock(id: &str) {
    LOCKS.lock().unwrap_or_else(|p| p.into_inner()).remove(id);
}

pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// An id is a uuid: it names files on disk, so nothing else gets through.
pub(crate) fn valid_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok()
}

/// A filename as it is stored: its last path component, never empty.
fn safe_filename(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("").trim();
    if base.is_empty() || base == "." || base == ".." {
        "upload".to_string()
    } else {
        base.to_string()
    }
}

impl Store {
    /// The store in this bot's data directory, with the upload store beside it.
    pub fn open() -> Option<Store> {
        let data = config::data_dir().ok()?;
        let uploads = agent::uploads::dir()?;
        Some(Store::at(&data, uploads))
    }

    /// The store under `data`, landing completed files in `uploads`.
    pub fn at(data: &Path, uploads: PathBuf) -> Store {
        Store { sessions: data.join("upload-sessions"), uploads }
    }

    fn meta_path(&self, id: &str) -> PathBuf {
        self.sessions.join(format!("{id}.json"))
    }

    fn part_path(&self, id: &str) -> PathBuf {
        self.sessions.join(format!("{id}.part"))
    }

    /// Where a completed file lives: the upload store, under its id.
    pub fn landed_path(&self, id: &str, filename: &str) -> PathBuf {
        self.uploads.join(agent::uploads::file_name(id, filename))
    }

    fn save(&self, s: &Session) -> Result<(), UploadError> {
        let tmp = self.sessions.join(format!("{}.json.tmp", s.upload_id));
        std::fs::write(&tmp, serde_json::to_vec(s).map_err(|e| UploadError::Io(e.to_string()))?)?;
        std::fs::rename(&tmp, self.meta_path(&s.upload_id))?;
        Ok(())
    }

    /// Open a session for a file of `size` bytes.
    pub fn create(
        &self,
        filename: &str,
        mime_type: &str,
        size: u64,
        agent_id: &str,
        chat_id: &str,
    ) -> Result<Session, UploadError> {
        if size == 0 {
            return Err(UploadError::Invalid("an upload needs the file's size".into()));
        }
        if size > MAX_BYTES {
            return Err(UploadError::TooLarge(format!(
                "That file is larger than {} GB, which is the most one upload may carry.",
                MAX_BYTES / (1024 * 1024 * 1024)
            )));
        }
        std::fs::create_dir_all(&self.sessions)?;
        let s = Session {
            upload_id: uuid::Uuid::new_v4().to_string(),
            filename: safe_filename(filename),
            mime_type: if mime_type.is_empty() { "application/octet-stream".into() } else { mime_type.into() },
            size,
            offset: 0,
            agent_id: agent_id.into(),
            chat_id: chat_id.into(),
            updated_at: now(),
        };
        std::fs::File::create(self.part_path(&s.upload_id))?;
        self.save(&s)?;
        Ok(s)
    }

    /// The session as it stands.
    pub fn get(&self, id: &str) -> Result<Session, UploadError> {
        if !valid_id(id) {
            return Err(UploadError::NotFound);
        }
        let bytes = std::fs::read(self.meta_path(id)).map_err(|_| UploadError::NotFound)?;
        serde_json::from_slice(&bytes).map_err(|e| UploadError::Io(e.to_string()))
    }

    /// Write one chunk at `offset`. The chunk may start anywhere the session
    /// already holds (a resend lands where it did before); past that is a
    /// gap, refused with the session so the client sends from its offset.
    pub async fn write<S, B, E>(&self, id: &str, offset: u64, mut body: S) -> Result<Session, UploadError>
    where
        S: Stream<Item = Result<B, E>> + Unpin,
        B: AsRef<[u8]>,
        E: std::fmt::Display,
    {
        let lock = lock_for(id);
        let _held = lock.lock().await;
        let mut s = self.get(id)?;
        if offset > s.offset {
            return Err(UploadError::Conflict(Box::new(s)));
        }
        let mut file = tokio::fs::OpenOptions::new().write(true).open(self.part_path(id)).await?;
        file.seek(std::io::SeekFrom::Start(offset)).await?;
        let mut written = 0u64;
        while let Some(piece) = body.next().await {
            let piece = piece.map_err(|e| UploadError::Io(format!("the chunk was cut short: {e}")))?;
            let piece = piece.as_ref();
            written += piece.len() as u64;
            if written > CHUNK_SIZE {
                return Err(UploadError::TooLarge(format!("a chunk carries at most {CHUNK_SIZE} bytes")));
            }
            if offset + written > s.size {
                return Err(UploadError::TooLarge(format!("the file is {} bytes", s.size)));
            }
            file.write_all(piece).await?;
        }
        file.sync_data().await?;
        s.offset = s.offset.max(offset + written);
        s.updated_at = now();
        self.save(&s)?;
        Ok(s)
    }

    /// Finish the upload: every byte held, and their sha256 is `sha256`.
    /// The file moves into the upload store and the session ends; what is
    /// returned is the stored file's attachment.
    pub async fn complete(&self, id: &str, sha256: &str) -> Result<comm::wire::Attachment, UploadError> {
        let lock = lock_for(id);
        let _held = lock.lock().await;
        let s = self.get(id)?;
        let part = self.part_path(id);
        let landed = self.landed_path(id, &s.filename);
        if s.offset < s.size {
            return Err(UploadError::Conflict(Box::new(s)));
        }
        // A completion cut off after the move: the bytes are already landed.
        if part.exists() {
            let len = tokio::fs::metadata(&part).await?.len();
            if len != s.size {
                return Err(UploadError::Invalid(format!("holding {len} of {} bytes", s.size)));
            }
            let got = hash_file(&part).await?;
            if !got.eq_ignore_ascii_case(sha256.trim()) {
                self.remove(id);
                return Err(UploadError::HashMismatch);
            }
            tokio::fs::rename(&part, &landed).await?;
        } else if !landed.exists() {
            return Err(UploadError::NotFound);
        }
        let attachment = comm::wire::Attachment {
            url: format!("/api/v1/comm-files/{id}"),
            file_id: id.to_string(),
            filename: s.filename.clone(),
            mime_type: s.mime_type.clone(),
            size: s.size,
            thumbnail_url: None,
            width: None,
            height: None,
            duration: None,
        };
        let _ = std::fs::remove_file(self.meta_path(id));
        drop(_held);
        forget_lock(id);
        Ok(attachment)
    }

    fn remove(&self, id: &str) {
        let _ = std::fs::remove_file(self.part_path(id));
        let _ = std::fs::remove_file(self.meta_path(id));
    }

    /// Remove sessions untouched for a day: an upload the phone gave up on
    /// leaves nothing behind.
    pub fn sweep(&self, now: u64) -> usize {
        let Ok(entries) = std::fs::read_dir(&self.sessions) else {
            return 0;
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(id) = name.strip_suffix(".json") else { continue };
            let stale = match self.get(id) {
                Ok(s) => now.saturating_sub(s.updated_at) > IDLE_EXPIRY_SECS,
                Err(_) => true,
            };
            if stale {
                self.remove(id);
                forget_lock(id);
                removed += 1;
            }
        }
        removed
    }
}

/// The sha256 of a file, read in pieces off the async threads.
async fn hash_file(path: &Path) -> Result<String, UploadError> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || -> std::io::Result<String> {
        use sha2::Digest;
        use std::io::Read;
        let mut file = std::fs::File::open(path)?;
        let mut hasher = sha2::Sha256::new();
        let mut buf = vec![0u8; 1024 * 1024];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(hex::encode(hasher.finalize()))
    })
    .await
    .map_err(|e| UploadError::Io(e.to_string()))?
    .map_err(UploadError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;

    fn store() -> (tempfile::TempDir, Store) {
        let tmp = tempfile::tempdir().unwrap();
        let uploads = tmp.path().join("files/uploads");
        std::fs::create_dir_all(&uploads).unwrap();
        let store = Store::at(tmp.path(), uploads);
        (tmp, store)
    }

    fn chunks(bytes: &[u8]) -> futures::stream::Iter<std::vec::IntoIter<Result<Vec<u8>, std::io::Error>>> {
        futures::stream::iter(vec![Ok(bytes.to_vec())])
    }

    fn sha(bytes: &[u8]) -> String {
        hex::encode(sha2::Sha256::digest(bytes))
    }

    /// A file of `n` bytes that is not all one value, so a misplaced chunk shows.
    fn file(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i % 251) as u8).collect()
    }

    /// The connection drops partway: the session keeps what came, says so,
    /// and the rest sent from that offset completes the same file.
    #[tokio::test]
    async fn an_interrupted_upload_resumes_from_the_offset() {
        let (_tmp, store) = store();
        let data = file(1000);
        let s = store.create("clip.mp4", "video/mp4", 1000, "", "").unwrap();

        store.write(&s.upload_id, 0, chunks(&data[..400])).await.unwrap();
        // The next chunk is cut off mid-body.
        let cut = futures::stream::iter(vec![
            Ok(data[400..550].to_vec()),
            Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset")),
        ]);
        assert!(store.write(&s.upload_id, 400, cut).await.is_err());

        // After the break the client asks where to go on from.
        let held = store.get(&s.upload_id).unwrap().offset;
        assert_eq!(held, 400, "a cut chunk does not count");
        store.write(&s.upload_id, held, chunks(&data[400..])).await.unwrap();

        let att = store.complete(&s.upload_id, &sha(&data)).await.unwrap();
        assert_eq!(att.file_id, s.upload_id);
        assert_eq!(att.size, 1000);
        let landed = store.landed_path(&s.upload_id, "clip.mp4");
        assert_eq!(std::fs::read(landed).unwrap(), data);
        assert!(store.get(&s.upload_id).is_err(), "the session ends");
    }

    /// The same chunk twice (its answer was lost) changes nothing.
    #[tokio::test]
    async fn a_duplicate_chunk_is_idempotent() {
        let (_tmp, store) = store();
        let data = file(300);
        let s = store.create("a.mov", "video/quicktime", 300, "", "").unwrap();
        store.write(&s.upload_id, 0, chunks(&data[..200])).await.unwrap();
        let again = store.write(&s.upload_id, 0, chunks(&data[..200])).await.unwrap();
        assert_eq!(again.offset, 200);
        // An overlapping resend from inside what is held is fine too.
        let after = store.write(&s.upload_id, 100, chunks(&data[100..])).await.unwrap();
        assert_eq!(after.offset, 300);
        store.complete(&s.upload_id, &sha(&data)).await.unwrap();
        assert_eq!(std::fs::read(store.landed_path(&s.upload_id, "a.mov")).unwrap(), data);
    }

    /// A chunk past what is held is a gap: refused with the offset to use.
    #[tokio::test]
    async fn a_chunk_past_the_offset_is_refused_with_the_offset() {
        let (_tmp, store) = store();
        let s = store.create("a.mp4", "video/mp4", 100, "", "").unwrap();
        store.write(&s.upload_id, 0, chunks(&file(10))).await.unwrap();
        match store.write(&s.upload_id, 50, chunks(&file(10))).await {
            Err(UploadError::Conflict(s)) => assert_eq!(s.offset, 10),
            other => panic!("expected a conflict, got {other:?}"),
        }
    }

    /// Completing checks the size and the hash.
    #[tokio::test]
    async fn complete_checks_the_size_and_the_hash() {
        let (_tmp, store) = store();
        let data = file(100);
        let s = store.create("a.mp4", "video/mp4", 100, "", "").unwrap();
        store.write(&s.upload_id, 0, chunks(&data[..60])).await.unwrap();
        match store.complete(&s.upload_id, &sha(&data)).await {
            Err(UploadError::Conflict(s)) => assert_eq!(s.offset, 60, "short: send the rest"),
            other => panic!("expected a conflict, got {other:?}"),
        }
        store.write(&s.upload_id, 60, chunks(&data[60..])).await.unwrap();
        assert!(matches!(store.complete(&s.upload_id, &sha(b"other")).await, Err(UploadError::HashMismatch)));
        assert!(store.get(&s.upload_id).is_err(), "a corrupt upload is dropped");
        assert!(!store.landed_path(&s.upload_id, "a.mp4").exists());
    }

    /// Over the file's size, over a chunk's size, or over the cap: refused.
    #[tokio::test]
    async fn sizes_are_held_to() {
        let (_tmp, store) = store();
        assert!(matches!(store.create("big.mp4", "video/mp4", MAX_BYTES + 1, "", ""), Err(UploadError::TooLarge(_))));
        assert!(store.create("big.mp4", "video/mp4", MAX_BYTES, "", "").is_ok(), "2 GB is allowed");
        assert!(matches!(store.create("none.mp4", "video/mp4", 0, "", ""), Err(UploadError::Invalid(_))));
        let s = store.create("a.mp4", "video/mp4", 10, "", "").unwrap();
        assert!(matches!(store.write(&s.upload_id, 0, chunks(&file(11))).await, Err(UploadError::TooLarge(_))));
        assert_eq!(store.get(&s.upload_id).unwrap().offset, 0);
    }

    /// The name names a file on disk: only its last component is kept, and
    /// an id that is not a uuid finds nothing.
    #[test]
    fn names_and_ids_cannot_reach_outside_the_store() {
        let (_tmp, store) = store();
        let s = store.create("../../etc/passwd", "text/plain", 5, "", "").unwrap();
        assert_eq!(s.filename, "passwd");
        assert!(matches!(store.get("../x"), Err(UploadError::NotFound)));
    }

    /// A session untouched for a day is removed; a live one stays.
    #[test]
    fn idle_sessions_are_swept() {
        let (_tmp, store) = store();
        let old = store.create("a.mp4", "video/mp4", 10, "", "").unwrap();
        let live = store.create("b.mp4", "video/mp4", 10, "", "").unwrap();
        let later = now() + IDLE_EXPIRY_SECS + 10;
        let mut fresh = store.get(&live.upload_id).unwrap();
        fresh.updated_at = later;
        store.save(&fresh).unwrap();
        assert_eq!(store.sweep(later), 1);
        assert!(store.get(&old.upload_id).is_err());
        assert!(!store.part_path(&old.upload_id).exists());
        assert!(store.get(&live.upload_id).is_ok());
    }
}
