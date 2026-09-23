//! The durable set on disk and in chunks: which objects it holds, whether
//! they changed, and how each one is packed and unpacked.
//!
//! An object is a tar of its files (sorted, owner-neutral, mtimes kept),
//! gzipped, then cut into pieces of at most [`CHUNK_PLAIN_MAX`] bytes. Each
//! piece is sealed with AES-256-GCM under the bot's key (12-byte nonce first,
//! the same layout the ring's single-copy shipping used) and wrapped in a
//! `.tar` beside a `manifest.json` that says what it is — the files door
//! accepts a `.tar`, and a chunk found in Spaces alone still names itself.
//!
//! The tar and the gzip are deterministic, so an object whose files did not
//! change packs to the same pieces with the same SHA-256, and the hub already
//! has them. Everything here is blocking; callers run it in `spawn_blocking`.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Plaintext bytes per chunk before sealing. A sealed chunk is this plus 28
/// bytes of nonce and tag plus a small tar envelope, well under the files
/// door's 100 MB backup ceiling (the spec's bound is 90 MB). It is kept
/// below that bound because a chunk is held in memory twice while it is
/// sealed, on a pod whose memory is the scarce resource.
pub const CHUNK_PLAIN_MAX: usize = 64 << 20;

/// The key-wrapping scheme the hub records beside every uploaded copy.
pub const KEY_VERSION: u32 = 1;

/// The database, as the durable set names it.
pub const DATABASE_PATH: &str = "data/nebo.db";

/// Loose files at the top of the data directory that are part of the bot.
const TOP_FILES: &[&str] = &[
    "settings.json",
    "bot_id",
    "neboai_token.cache",
    ".setup-complete",
];

/// Directories that are part of the bot. Everything else in the data
/// directory is scratch: rebuilt, re-downloaded or disposable.
const DURABLE_TREES: &[&str] = &[
    "files",
    "user",
    "appdata",
    "packs",
    "sessions",
    "learned",
    "nebo",
    CHROMIUM_PROFILE,
];

/// The built-in browser's profile. Durable only while no Chromium holds it:
/// a live profile is being written as it is read.
const CHROMIUM_PROFILE: &str = "chromium-profile";

/// The files by which a Chromium claims its profile. Never state: restored
/// on another machine, they name a Chromium there that does not exist, and
/// Chromium refuses a profile "in use on another computer".
const CHROMIUM_LOCKS: &[&str] = &["SingletonLock", "SingletonSocket", "SingletonCookie"];

/// Where a commit stages its chunks, inside the data directory (same disk,
/// scratch by the rule above).
pub const STAGING_DIR: &str = "cache/state-commit";

/// Where a restore stages what it unpacks before moving it into place.
pub const RESTORE_DIR: &str = ".state-restore";

/// What an object's files looked like when last scanned. Equal fingerprints
/// mean nothing in the object was written, created, deleted or renamed
/// (directory mtimes move on create, delete and rename).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub entries: u64,
    pub bytes: u64,
    pub max_mtime_ns: i64,
}

/// One uploaded piece of an object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkRef {
    pub file_id: String,
    /// Of the plaintext piece: the hub's idempotency key for the upload.
    pub sha256: String,
    /// Of the uploaded `.tar`.
    pub bytes: u64,
}

/// One object of a committed generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectEntry {
    /// `database`, `top` (loose top-level files) or `tree`.
    pub role: String,
    /// Where it lives in the data directory.
    pub root: String,
    /// Of the object's plaintext tar.
    pub sha256: String,
    /// Of the object's plaintext tar.
    pub bytes: u64,
    pub fingerprint: Fingerprint,
    pub chunks: Vec<ChunkRef>,
}

/// The manifest a generation commits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub bot_id: String,
    pub generation: i64,
    /// The lease epoch this instance held when it committed; 0 when the bot
    /// holds no lease.
    pub lease_epoch: i64,
    /// `state` (the durable set) or `archive` (the whole data directory).
    pub role: String,
    pub taken_at: String,
    pub nebo_version: String,
    /// When the earliest durable timer fires (RFC 3339), if any.
    pub next_wake: Option<String>,
    pub key_version: u32,
    pub objects: Vec<ObjectEntry>,
    #[serde(default)]
    pub residency_signals: Option<serde_json::Value>,
}

/// An object as found on disk: what to read and what to leave out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub role: String,
    pub root: String,
    /// Paths relative to the data directory, walked recursively. For the
    /// database these are the live file and its journals (what the
    /// fingerprint reads); what is packed is a verified copy.
    pub members: Vec<String>,
    /// Paths relative to the data directory never included.
    pub exclude: Vec<String>,
}

/// A packed object: its chunks sit in the staging directory, ready to upload.
#[derive(Debug)]
pub struct PackedObject {
    pub sha256: String,
    pub bytes: u64,
    pub chunks: Vec<PackedChunk>,
}

#[derive(Debug)]
pub struct PackedChunk {
    pub path: PathBuf,
    pub sha256: String,
    pub bytes: u64,
}

/// What each chunk's `manifest.json` repeats about it.
#[derive(Debug, Clone)]
pub struct ChunkMeta<'a> {
    pub bot_id: &'a str,
    pub taken_at: i64,
}

/// True while a Chromium on this machine holds the profile. Chromium keeps a
/// `SingletonLock` symlink naming `<hostname>-<pid>` for as long as it runs.
/// A lock Chromium could not remove — it was killed, or the profile came from
/// another machine — names a process not running here, and holds nothing.
pub fn chromium_running(data_dir: &Path) -> bool {
    match fs::read_link(data_dir.join(CHROMIUM_PROFILE).join("SingletonLock")) {
        Ok(target) => lock_holder_running(&target.to_string_lossy()),
        Err(_) => false,
    }
}

/// Whether the `<hostname>-<pid>` a `SingletonLock` names is a process
/// running on this machine. A lock that cannot be read that way counts as
/// held: it is never packed as state while it might be live.
#[cfg(unix)]
fn lock_holder_running(target: &str) -> bool {
    let Some((host, pid)) = target.rsplit_once('-') else {
        return true;
    };
    let Ok(pid) = pid.parse::<libc::pid_t>() else {
        return true;
    };
    let mut buf = [0u8; 256];
    // SAFETY: the buffer outlives the call and its length is passed with it.
    if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
        return true;
    }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    if host.as_bytes() != &buf[..len] {
        return false;
    }
    // Signal 0 checks the process exists; EPERM means it exists as another user.
    // SAFETY: kill with signal 0 sends nothing.
    let signalled = unsafe { libc::kill(pid, 0) } == 0;
    signalled || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn lock_holder_running(_target: &str) -> bool {
    true
}

/// What a commit of the Chromium profile leaves out: its locks.
fn chromium_excludes() -> Vec<String> {
    CHROMIUM_LOCKS.iter().map(|f| format!("{CHROMIUM_PROFILE}/{f}")).collect()
}

/// The objects of a commit. `archive` is the whole data directory (the
/// one-shot copy taken before a disk is retired); otherwise the durable set.
/// The Chromium profile is left out while a Chromium holds it.
pub fn sources(data_dir: &Path, archive: bool) -> Vec<Source> {
    let browser_busy = chromium_running(data_dir);
    let database = Source {
        role: "database".into(),
        root: DATABASE_PATH.into(),
        members: vec![
            DATABASE_PATH.into(),
            format!("{DATABASE_PATH}-journal"),
            format!("{DATABASE_PATH}-wal"),
        ],
        exclude: vec![],
    };
    let mut out = vec![database];
    if !archive {
        out.push(Source {
            role: "top".into(),
            root: ".".into(),
            members: TOP_FILES.iter().map(|s| s.to_string()).collect(),
            exclude: vec![],
        });
        for tree in DURABLE_TREES {
            if *tree == CHROMIUM_PROFILE && browser_busy {
                continue;
            }
            if data_dir.join(tree).is_dir() {
                out.push(Source {
                    role: "tree".into(),
                    root: tree.to_string(),
                    members: vec![tree.to_string()],
                    exclude: if *tree == CHROMIUM_PROFILE { chromium_excludes() } else { vec![] },
                });
            }
        }
        return out;
    }
    let mut names: Vec<(String, bool)> = fs::read_dir(data_dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    let ft = e.file_type().ok()?;
                    Some((name, ft.is_dir()))
                })
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    let mut loose = Vec::new();
    for (name, is_dir) in names {
        if name == RESTORE_DIR || (name == CHROMIUM_PROFILE && browser_busy) {
            continue;
        }
        if !is_dir {
            loose.push(name);
            continue;
        }
        let exclude = match name.as_str() {
            "data" => vec![
                DATABASE_PATH.into(),
                format!("{DATABASE_PATH}-journal"),
                format!("{DATABASE_PATH}-wal"),
                format!("{DATABASE_PATH}-shm"),
            ],
            "cache" => vec![STAGING_DIR.into()],
            CHROMIUM_PROFILE => chromium_excludes(),
            _ => vec![],
        };
        out.push(Source {
            role: "tree".into(),
            root: name.clone(),
            members: vec![name],
            exclude,
        });
    }
    out.insert(
        1,
        Source {
            role: "top".into(),
            root: ".".into(),
            members: loose,
            exclude: vec![],
        },
    );
    out
}

/// One entry of an object, in the order it is packed.
struct Entry {
    rel: String,
    meta: fs::Metadata,
}

/// Every entry under the source's members, sorted, symlinks not followed.
/// A member that does not exist is simply absent.
fn walk(data_dir: &Path, source: &Source) -> io::Result<Vec<Entry>> {
    fn visit(
        data_dir: &Path,
        rel: &str,
        exclude: &[String],
        out: &mut Vec<Entry>,
    ) -> io::Result<()> {
        if exclude.iter().any(|x| x == rel) {
            return Ok(());
        }
        let meta = match fs::symlink_metadata(data_dir.join(rel)) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        let ft = meta.file_type();
        if !(ft.is_file() || ft.is_dir() || ft.is_symlink()) {
            return Ok(()); // sockets, fifos: not state
        }
        let is_dir = ft.is_dir();
        out.push(Entry {
            rel: rel.to_string(),
            meta,
        });
        if is_dir {
            let mut names: Vec<String> = match fs::read_dir(data_dir.join(rel)) {
                Ok(rd) => rd
                    .filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect(),
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(e),
            };
            names.sort();
            for name in names {
                visit(data_dir, &format!("{rel}/{name}"), exclude, out)?;
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    let mut members = source.members.clone();
    members.sort();
    for m in &members {
        visit(data_dir, m, &source.exclude, &mut out)?;
    }
    Ok(out)
}

fn mtime_ns(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

/// What the object's files look like now.
pub fn fingerprint(data_dir: &Path, source: &Source) -> io::Result<Fingerprint> {
    let mut f = Fingerprint::default();
    for e in walk(data_dir, source)? {
        f.entries += 1;
        if e.meta.is_file() {
            f.bytes += e.meta.len();
        }
        f.max_mtime_ns = f.max_mtime_ns.max(mtime_ns(&e.meta));
    }
    Ok(f)
}

/// Hashes and counts everything written through it.
struct Hashing<W: Write> {
    inner: W,
    sha: Sha256,
    n: u64,
}

impl<W: Write> Write for Hashing<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write_all(buf)?;
        self.sha.update(buf);
        self.n += buf.len() as u64;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Cuts the gzip stream into pieces, sealing and writing each as it fills.
struct Segmenter<'a> {
    buf: Vec<u8>,
    max: usize,
    dir: &'a Path,
    prefix: String,
    key: &'a [u8; 32],
    meta: &'a ChunkMeta<'a>,
    source: &'a Source,
    chunks: Vec<PackedChunk>,
}

impl Segmenter<'_> {
    fn seal(&mut self) -> io::Result<()> {
        let sha256 = hex::encode(Sha256::digest(&self.buf));
        let sealed = mcp::crypto::Encryptor::new(*self.key)
            .encrypt(&self.buf)
            .map_err(|e| io::Error::other(format!("encrypt: {e}")))?;
        self.buf.clear();
        let index = self.chunks.len();
        let inner_name = format!("{}-{index:04}.gz.enc", self.prefix);
        let manifest = serde_json::json!({
            "bot_id": self.meta.bot_id,
            "taken_at": self.meta.taken_at,
            "sha256": sha256,
            "key_version": KEY_VERSION,
            "cipher": "aes-256-gcm",
            "layout": "nonce(12) || ciphertext",
            "compression": "gzip",
            "file": inner_name,
            "object": {"role": self.source.role, "root": self.source.root},
            "index": index,
            "nebo_version": env!("CARGO_PKG_VERSION"),
        })
        .to_string();
        let path = self.dir.join(format!("{}-{index:04}.tar", self.prefix));
        let mut ar = tar::Builder::new(io::BufWriter::new(File::create(&path)?));
        for (name, bytes) in [
            ("manifest.json", manifest.as_bytes()),
            (inner_name.as_str(), sealed.as_slice()),
        ] {
            let mut h = tar::Header::new_gnu();
            h.set_size(bytes.len() as u64);
            h.set_mode(0o600);
            h.set_mtime(self.meta.taken_at.max(0) as u64);
            ar.append_data(&mut h, name, bytes)?;
        }
        ar.into_inner()?.flush()?;
        let bytes = fs::metadata(&path)?.len();
        self.chunks.push(PackedChunk {
            path,
            sha256,
            bytes,
        });
        Ok(())
    }

    fn finish(mut self) -> io::Result<Vec<PackedChunk>> {
        if !self.buf.is_empty() || self.chunks.is_empty() {
            self.seal()?;
        }
        Ok(self.chunks)
    }
}

impl Write for Segmenter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let room = self.max - self.buf.len();
        let n = room.min(buf.len());
        self.buf.extend_from_slice(&buf[..n]);
        if self.buf.len() == self.max {
            self.seal()?;
        }
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Reads exactly `left` bytes or fails: a file that shrinks while it is
/// packed must fail the pack, never pad the archive with a short entry.
struct Exact<R: Read> {
    inner: R,
    left: u64,
}

impl<R: Read> Read for Exact<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.left == 0 {
            return Ok(0);
        }
        let want = (buf.len() as u64).min(self.left) as usize;
        let n = self.inner.read(&mut buf[..want])?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "a file changed size while it was packed",
            ));
        }
        self.left -= n as u64;
        Ok(n)
    }
}

fn header_for(meta: &fs::Metadata, entry_type: tar::EntryType, size: u64) -> tar::Header {
    let mut h = tar::Header::new_gnu();
    h.set_entry_type(entry_type);
    h.set_size(size);
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o7777
    };
    #[cfg(not(unix))]
    let mode = if entry_type == tar::EntryType::Directory {
        0o755
    } else {
        0o644
    };
    h.set_mode(mode);
    h.set_mtime((mtime_ns(meta) / 1_000_000_000).max(0) as u64);
    h.set_uid(0);
    h.set_gid(0);
    h
}

/// Pack one object into sealed chunks of at most `chunk_max` plaintext
/// bytes ([`CHUNK_PLAIN_MAX`] outside tests) in `out_dir`. For the
/// database, `db_copy` is the verified copy packed in the live file's place.
pub fn pack_object(
    data_dir: &Path,
    source: &Source,
    db_copy: Option<&Path>,
    out_dir: &Path,
    key: &[u8; 32],
    meta: &ChunkMeta<'_>,
    chunk_max: usize,
) -> Result<PackedObject, String> {
    let prefix: String = format!("{}-{}", source.role, source.root)
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let seg = Segmenter {
        buf: Vec::with_capacity(chunk_max.min(1 << 20)),
        max: chunk_max,
        dir: out_dir,
        prefix,
        key,
        meta,
        source,
        chunks: Vec::new(),
    };
    let gz = GzEncoder::new(seg, Compression::default());
    let mut ar = tar::Builder::new(Hashing {
        inner: gz,
        sha: Sha256::new(),
        n: 0,
    });

    let add = |ar: &mut tar::Builder<Hashing<GzEncoder<Segmenter<'_>>>>,
               rel: &str,
               path: &Path|
     -> io::Result<()> {
        let meta = match fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()), // gone since the walk
            Err(e) => return Err(e),
        };
        let ft = meta.file_type();
        if ft.is_dir() {
            let mut h = header_for(&meta, tar::EntryType::Directory, 0);
            ar.append_data(&mut h, format!("{rel}/"), io::empty())
        } else if ft.is_symlink() {
            let target = fs::read_link(path)?;
            let mut h = header_for(&meta, tar::EntryType::Symlink, 0);
            ar.append_link(&mut h, rel, target)
        } else {
            let file = match File::open(path) {
                Ok(f) => f,
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(e),
            };
            let meta = file.metadata()?;
            let len = meta.len();
            let mut h = header_for(&meta, tar::EntryType::Regular, len);
            ar.append_data(
                &mut h,
                rel,
                Exact {
                    inner: file.take(len),
                    left: len,
                },
            )
        }
    };

    let result = (|| -> io::Result<()> {
        if source.role == "database" {
            let copy = db_copy
                .ok_or_else(|| io::Error::other("the database is packed from a verified copy"))?;
            add(&mut ar, DATABASE_PATH, copy)?;
        } else {
            for e in walk(data_dir, source)? {
                add(&mut ar, &e.rel, &data_dir.join(&e.rel))?;
            }
        }
        Ok(())
    })();
    result.map_err(|e| format!("pack {}: {e}", source.root))?;

    let hashing = ar
        .into_inner()
        .map_err(|e| format!("tar {}: {e}", source.root))?;
    let sha256 = hex::encode(hashing.sha.finalize());
    let bytes = hashing.n;
    let seg = hashing
        .inner
        .finish()
        .map_err(|e| format!("gzip {}: {e}", source.root))?;
    let chunks = seg
        .finish()
        .map_err(|e| format!("seal {}: {e}", source.root))?;
    Ok(PackedObject {
        sha256,
        bytes,
        chunks,
    })
}

/// Open one downloaded chunk: find the sealed piece beside its manifest,
/// decrypt it, and prove it is the piece the generation names.
pub fn open_chunk(
    chunk_tar: &[u8],
    key: &[u8; 32],
    expected_sha256: &str,
) -> Result<Vec<u8>, String> {
    let mut sealed = None;
    let mut ar = tar::Archive::new(chunk_tar);
    for entry in ar
        .entries()
        .map_err(|e| format!("chunk is not a tar: {e}"))?
    {
        let mut entry = entry.map_err(|e| format!("chunk tar: {e}"))?;
        let name = entry
            .path()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        if name == "manifest.json" {
            continue;
        }
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|e| format!("chunk tar: {e}"))?;
        sealed = Some(bytes);
    }
    let sealed = sealed.ok_or("chunk holds no sealed piece")?;
    let plain = mcp::crypto::Encryptor::new(*key)
        .decrypt(&sealed)
        .map_err(|e| format!("chunk does not decrypt with this bot's key: {e}"))?;
    let got = hex::encode(Sha256::digest(&plain));
    if got != expected_sha256 {
        return Err(format!(
            "chunk checksum mismatch: expected {expected_sha256}, got {got}"
        ));
    }
    Ok(plain)
}

/// Hashes and counts everything read through it.
struct HashingReader<R: Read> {
    inner: R,
    sha: Sha256,
    n: u64,
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.sha.update(&buf[..n]);
        self.n += n as u64;
        Ok(n)
    }
}

/// Unpack an object's reassembled gzip stream into `into`, and prove the
/// tar it held is the one the generation names.
pub fn unpack_object(
    gz_path: &Path,
    into: &Path,
    expected_sha256: &str,
    expected_bytes: u64,
) -> Result<(), String> {
    let file = File::open(gz_path).map_err(|e| format!("open {}: {e}", gz_path.display()))?;
    let reader = HashingReader {
        inner: GzDecoder::new(io::BufReader::new(file)),
        sha: Sha256::new(),
        n: 0,
    };
    let mut ar = tar::Archive::new(reader);
    ar.set_preserve_mtime(true);
    ar.set_preserve_permissions(true);
    ar.set_overwrite(true);
    ar.unpack(into)
        .map_err(|e| format!("unpack into {}: {e}", into.display()))?;
    let mut reader = ar.into_inner();
    io::copy(&mut reader, &mut io::sink()).map_err(|e| format!("read object: {e}"))?;
    let got = hex::encode(reader.sha.finalize());
    if got != expected_sha256 || reader.n != expected_bytes {
        return Err(format!(
            "object checksum mismatch: expected {expected_sha256} ({expected_bytes} bytes), got {got} ({} bytes)",
            reader.n
        ));
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    const KEY: [u8; 32] = [7u8; 32];

    fn meta() -> ChunkMeta<'static> {
        ChunkMeta {
            bot_id: "bot-1",
            taken_at: 1_789_700_000,
        }
    }

    fn tree_source(root: &str) -> Source {
        Source {
            role: "tree".into(),
            root: root.into(),
            members: vec![root.into()],
            exclude: vec![],
        }
    }

    /// Every file under `dir`, relative path → bytes (symlinks → target).
    fn snapshot(dir: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
        let mut out = std::collections::BTreeMap::new();
        fn go(base: &Path, dir: &Path, out: &mut std::collections::BTreeMap<String, Vec<u8>>) {
            for e in fs::read_dir(dir).unwrap() {
                let e = e.unwrap();
                let p = e.path();
                let rel = p.strip_prefix(base).unwrap().to_string_lossy().to_string();
                let ft = fs::symlink_metadata(&p).unwrap().file_type();
                if ft.is_symlink() {
                    out.insert(
                        rel,
                        fs::read_link(&p)
                            .unwrap()
                            .to_string_lossy()
                            .as_bytes()
                            .to_vec(),
                    );
                } else if ft.is_dir() {
                    out.insert(format!("{rel}/"), vec![]);
                    go(base, &p, out);
                } else {
                    out.insert(rel, fs::read(&p).unwrap());
                }
            }
        }
        go(dir, dir, &mut out);
        out
    }

    /// Where the sealed piece starts inside a chunk's tar.
    pub(crate) fn sealed_offset(chunk_tar: &[u8]) -> usize {
        let mut ar = tar::Archive::new(chunk_tar);
        for entry in ar.entries().unwrap() {
            let entry = entry.unwrap();
            if entry.path().unwrap().to_string_lossy() != "manifest.json" {
                return entry.raw_file_position() as usize;
            }
        }
        panic!("no sealed piece");
    }

    fn reassemble(chunks: &[PackedChunk], key: &[u8; 32], gz: &Path) -> Result<(), String> {
        let mut out = File::create(gz).unwrap();
        for c in chunks {
            let bytes = fs::read(&c.path).unwrap();
            out.write_all(&open_chunk(&bytes, key, &c.sha256)?).unwrap();
        }
        Ok(())
    }

    fn fill(dir: &Path) {
        fs::create_dir_all(dir.join("files/deep/er")).unwrap();
        fs::create_dir_all(dir.join("files/empty")).unwrap();
        fs::write(dir.join("files/a.txt"), b"alpha").unwrap();
        fs::write(
            dir.join("files/deep/er/b.bin"),
            (0..200_000u32)
                .flat_map(|i| i.to_le_bytes())
                .collect::<Vec<_>>(),
        )
        .unwrap();
        fs::write(
            dir.join(format!("files/{}.txt", "long-name-".repeat(20))),
            b"a path past 100 bytes",
        )
        .unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("a.txt", dir.join("files/link")).unwrap();
    }

    /// pack → chunks → open → unpack gives back the same bytes, the same
    /// tree, and the same mtimes.
    #[test]
    fn a_tree_round_trips_byte_for_byte() {
        let src = tempfile::tempdir().unwrap();
        fill(src.path());
        let out = tempfile::tempdir().unwrap();
        let packed = pack_object(
            src.path(),
            &tree_source("files"),
            None,
            out.path(),
            &KEY,
            &meta(),
            CHUNK_PLAIN_MAX,
        )
        .unwrap();
        assert_eq!(packed.chunks.len(), 1);

        let dst = tempfile::tempdir().unwrap();
        let gz = out.path().join("files.gz");
        reassemble(&packed.chunks, &KEY, &gz).unwrap();
        unpack_object(&gz, dst.path(), &packed.sha256, packed.bytes).unwrap();
        assert_eq!(snapshot(src.path()), snapshot(dst.path()));
        let m = |d: &Path| {
            fs::metadata(d.join("files/a.txt"))
                .unwrap()
                .modified()
                .unwrap()
        };
        let secs =
            |t: std::time::SystemTime| t.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        assert_eq!(secs(m(src.path())), secs(m(dst.path())), "mtimes survive");
    }

    /// Unchanged files pack to the same chunks: the hub already has them.
    #[test]
    fn packing_is_deterministic() {
        let src = tempfile::tempdir().unwrap();
        fill(src.path());
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let pa = pack_object(
            src.path(),
            &tree_source("files"),
            None,
            a.path(),
            &KEY,
            &meta(),
            CHUNK_PLAIN_MAX,
        )
        .unwrap();
        let pb = pack_object(
            src.path(),
            &tree_source("files"),
            None,
            b.path(),
            &KEY,
            &meta(),
            CHUNK_PLAIN_MAX,
        )
        .unwrap();
        assert_eq!(pa.sha256, pb.sha256);
        let shas = |p: &PackedObject| {
            p.chunks
                .iter()
                .map(|c| c.sha256.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(shas(&pa), shas(&pb));
    }

    /// An object bigger than a chunk is cut into several, each within the
    /// bound, and still round-trips; and the production bound fits the
    /// files door's ceiling with room to spare.
    #[test]
    fn a_large_object_is_cut_into_bounded_chunks() {
        // Sealed = plaintext + 12-byte nonce + 16-byte tag, padded to a tar
        // block, beside two headers, a manifest block and the two-block end
        // marker: at most seven blocks of envelope.
        const ENVELOPE: usize = 28 + 7 * 512;
        assert!(CHUNK_PLAIN_MAX + ENVELOPE < 90_000_000);

        let src = tempfile::tempdir().unwrap();
        fs::create_dir_all(src.path().join("files")).unwrap();
        // Incompressible, so the gzip stream really spans several chunks.
        let mut noise = vec![0u8; 300_000];
        let mut x: u64 = 0x9E3779B97F4A7C15;
        for b in noise.iter_mut() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *b = x as u8;
        }
        fs::write(src.path().join("files/noise.bin"), &noise).unwrap();
        let out = tempfile::tempdir().unwrap();
        let max = 64 << 10;
        let packed = pack_object(
            src.path(),
            &tree_source("files"),
            None,
            out.path(),
            &KEY,
            &meta(),
            max,
        )
        .unwrap();
        assert_eq!(packed.chunks.len(), 5);
        for c in &packed.chunks {
            assert!(
                c.bytes <= (max + ENVELOPE) as u64,
                "chunk of {} bytes",
                c.bytes
            );
        }
        let dst = tempfile::tempdir().unwrap();
        let gz = out.path().join("files.gz");
        reassemble(&packed.chunks, &KEY, &gz).unwrap();
        unpack_object(&gz, dst.path(), &packed.sha256, packed.bytes).unwrap();
        assert_eq!(fs::read(dst.path().join("files/noise.bin")).unwrap(), noise);
    }

    /// The wrong key opens nothing.
    #[test]
    fn the_wrong_key_fails() {
        let src = tempfile::tempdir().unwrap();
        fill(src.path());
        let out = tempfile::tempdir().unwrap();
        let packed = pack_object(
            src.path(),
            &tree_source("files"),
            None,
            out.path(),
            &KEY,
            &meta(),
            CHUNK_PLAIN_MAX,
        )
        .unwrap();
        let bytes = fs::read(&packed.chunks[0].path).unwrap();
        let err = open_chunk(&bytes, &[8u8; 32], &packed.chunks[0].sha256).unwrap_err();
        assert!(err.contains("decrypt"), "{err}");
    }

    /// A chunk that is not the piece the manifest names fails its checksum,
    /// and a flipped ciphertext byte fails authentication.
    #[test]
    fn a_tampered_chunk_fails() {
        let src = tempfile::tempdir().unwrap();
        fill(src.path());
        let out = tempfile::tempdir().unwrap();
        let packed = pack_object(
            src.path(),
            &tree_source("files"),
            None,
            out.path(),
            &KEY,
            &meta(),
            CHUNK_PLAIN_MAX,
        )
        .unwrap();
        let bytes = fs::read(&packed.chunks[0].path).unwrap();

        let wrong_sha = "0".repeat(64);
        assert!(
            open_chunk(&bytes, &KEY, &wrong_sha)
                .unwrap_err()
                .contains("checksum")
        );

        // Re-seal different plaintext under the same key: decrypts, wrong sha.
        let other = mcp::crypto::Encryptor::new(KEY)
            .encrypt(b"not this chunk")
            .unwrap();
        let mut forged = tar::Builder::new(Vec::new());
        let mut h = tar::Header::new_gnu();
        h.set_size(other.len() as u64);
        forged
            .append_data(&mut h, "x.gz.enc", other.as_slice())
            .unwrap();
        let forged = forged.into_inner().unwrap();
        assert!(
            open_chunk(&forged, &KEY, &packed.chunks[0].sha256)
                .unwrap_err()
                .contains("checksum")
        );

        // Flip one byte of the sealed piece inside the tar.
        let mut flipped = bytes.clone();
        flipped[sealed_offset(&bytes) + 20] ^= 0xFF;
        assert!(
            open_chunk(&flipped, &KEY, &packed.chunks[0].sha256)
                .unwrap_err()
                .contains("decrypt")
        );
    }

    /// The object checksum catches a stream that decrypts but is not the
    /// object the generation names.
    #[test]
    fn an_object_checksum_mismatch_fails_the_unpack() {
        let src = tempfile::tempdir().unwrap();
        fill(src.path());
        let out = tempfile::tempdir().unwrap();
        let packed = pack_object(
            src.path(),
            &tree_source("files"),
            None,
            out.path(),
            &KEY,
            &meta(),
            CHUNK_PLAIN_MAX,
        )
        .unwrap();
        let gz = out.path().join("files.gz");
        reassemble(&packed.chunks, &KEY, &gz).unwrap();
        let dst = tempfile::tempdir().unwrap();
        assert!(
            unpack_object(&gz, dst.path(), &"f".repeat(64), packed.bytes)
                .unwrap_err()
                .contains("checksum")
        );
    }

    /// A write, a new file, a deletion: each moves the fingerprint.
    #[test]
    fn the_fingerprint_sees_writes_creates_and_deletes() {
        let src = tempfile::tempdir().unwrap();
        fill(src.path());
        let s = tree_source("files");
        let f0 = fingerprint(src.path(), &s).unwrap();
        assert_eq!(
            fingerprint(src.path(), &s).unwrap(),
            f0,
            "a scan changes nothing"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(src.path().join("files/a.txt"), b"alpha2").unwrap();
        let f1 = fingerprint(src.path(), &s).unwrap();
        assert_ne!(f1, f0);
        fs::write(src.path().join("files/new.txt"), b"n").unwrap();
        let f2 = fingerprint(src.path(), &s).unwrap();
        assert_ne!(f2, f1);
        fs::remove_file(src.path().join("files/new.txt")).unwrap();
        assert_ne!(fingerprint(src.path(), &s).unwrap(), f2);
    }

    /// The durable set is exactly the spec's; the browser profile only while
    /// no Chromium holds it; an archive is everything but the live database
    /// files and the commit's own staging.
    #[test]
    fn the_durable_set_and_the_archive() {
        let d = tempfile::tempdir().unwrap();
        for dir in [
            "files",
            "user",
            "nebo/plugin-profiles",
            "sessions",
            "home/go",
            "logs",
            "cache/state-commit",
            "data/backups",
            "chromium-profile",
        ] {
            fs::create_dir_all(d.path().join(dir)).unwrap();
        }
        for f in [
            "settings.json",
            "bot_id",
            "data/nebo.db",
            "data/nebo.db-journal",
        ] {
            fs::write(d.path().join(f), b"x").unwrap();
        }
        let roots = |v: &[Source]| v.iter().map(|s| s.root.clone()).collect::<Vec<_>>();
        let state = sources(d.path(), false);
        assert_eq!(
            roots(&state),
            vec![
                "data/nebo.db",
                ".",
                "files",
                "user",
                "sessions",
                "nebo",
                "chromium-profile"
            ]
        );

        #[cfg(unix)]
        {
            let lock = d.path().join("chromium-profile/SingletonLock");
            // Killed, or restored from another machine: the lock names a
            // Chromium that is not running here. The profile is state; its
            // locks are not.
            std::os::unix::fs::symlink("another-host-123", &lock).unwrap();
            let profile = sources(d.path(), false)
                .into_iter()
                .find(|s| s.root == "chromium-profile")
                .expect("a stale lock does not hold the profile");
            assert!(profile.exclude.contains(&"chromium-profile/SingletonLock".to_string()));
            assert!(
                !walk(d.path(), &profile).unwrap().iter().any(|e| e.rel.contains("Singleton")),
                "a profile's locks are never packed"
            );

            // A live Chromium on this machine: this process stands in for it.
            fs::remove_file(&lock).unwrap();
            let mut host = [0u8; 256];
            assert_eq!(unsafe { libc::gethostname(host.as_mut_ptr().cast(), host.len()) }, 0);
            let len = host.iter().position(|&b| b == 0).unwrap();
            let live = format!("{}-{}", String::from_utf8_lossy(&host[..len]), std::process::id());
            std::os::unix::fs::symlink(&live, &lock).unwrap();
            assert!(
                !roots(&sources(d.path(), false)).contains(&"chromium-profile".to_string()),
                "a running browser's profile is skipped"
            );
        }

        let archive = sources(d.path(), true);
        assert_eq!(
            roots(&archive),
            vec![
                "data/nebo.db",
                ".",
                "cache",
                "data",
                "files",
                "home",
                "logs",
                "nebo",
                "sessions",
                "user"
            ]
        );
        let data = archive.iter().find(|s| s.root == "data").unwrap();
        assert!(data.exclude.contains(&"data/nebo.db".to_string()));
        let cache = archive.iter().find(|s| s.root == "cache").unwrap();
        assert_eq!(cache.exclude, vec![STAGING_DIR.to_string()]);
        assert_eq!(
            archive[1].members,
            vec!["bot_id".to_string(), "settings.json".to_string()]
        );
    }
}
