//! The bot's state leaves the machine: BotState, committed per generation.
//!
//! The ring (`db::backup`) is a local safety net: it lives on the same disk
//! as the database it copies. A cloud bot's disk can be lost whole, so its
//! durable set — a verified copy of the database, `files/`, `sessions/`,
//! `nebo/` and the rest (see `pack::sources`) — is packed per object,
//! encrypted with the bot's own key, uploaded in chunks through the ONE file
//! door the hub has (`POST /api/v1/files/upload`, `purpose=backup`,
//! `kind=chunk`), and committed as generation N+1 of the bot's state
//! (`POST /api/v1/bots/self/state`). The hub moves a bot forward only from
//! N to N+1, so one writer wins and a failed commit leaves the previous
//! generation authoritative. The key's durable copy is wrapped in the hub's
//! catalogue, so a bot can be rebuilt from Spaces without this pod.
//!
//! Cadence: the scheduler asks every minute. A commit happens when any
//! object's files changed and the last commit is at least 15 minutes old, or
//! when a day has passed regardless — an unchanged object is not re-packed
//! and an unchanged chunk is not re-uploaded, so a quiet day costs one small
//! request. The graceful drain commits whatever changed as its last act
//! before the lease is handed back (`commit_on_drain`). A commit carries the
//! lease epoch it was made under and is made only while this process holds
//! the bot's lease (`comm::lease`). On boot, a server whose database is
//! missing restores the latest generation before anything opens it
//! (`restore_on_boot`).
//!
//! The key arrives as `NEBO_BACKUP_KEY` (hex, 32 bytes) in the pod's Secret.
//! Without it nothing ships — a desktop keeps its ring local.

mod pack;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tracing::{info, warn};

use comm::CommError;
use comm::api::NeboAIApi;
use db::Store;

pub use pack::Manifest;
use pack::{ChunkMeta, ChunkRef, Fingerprint, KEY_VERSION, ObjectEntry, Source};

use crate::state::AppState;

/// Commits may keep failing this long before the owner hears.
const UNSHIPPED_ALARM_SECS: i64 = 2 * 3600;
/// A changed durable set is committed at most this often.
const COMMIT_SPACING_SECS: i64 = 15 * 60;
/// And an unchanged one at least this often: proof the bot is still backed
/// up, and what the hub's stale-backup alarm reads.
const COMMIT_AT_LEAST_SECS: i64 = 24 * 3600;
/// The files door admits 20 uploads a minute per uploader; a refused upload
/// waits out the window this many times before the commit gives up.
const UPLOAD_RATE_RETRIES: usize = 5;

/// What a commit is of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    /// The durable set. What a restore reads.
    State,
    /// The whole data directory, kept 30 days whatever the retention rule:
    /// the copy taken before a disk is retired.
    Archive,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::State => "state",
            Role::Archive => "archive",
        }
    }
}

/// Where the database copy comes from.
#[derive(Clone)]
enum DbSource {
    /// The running server's store.
    Store(Arc<Store>),
    /// A database another process has open (the archive command, run beside
    /// the server): read through its own read-only connection.
    File(PathBuf),
}

impl DbSource {
    /// Make a verified copy at `dest` with the ONE copy primitive. Blocking.
    fn copy_to(&self, dest: &Path) -> Result<(), String> {
        match self {
            DbSource::Store(store) => store.copy_verified(dest).map_err(|e| e.to_string()),
            DbSource::File(path) => {
                let conn = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                    .map_err(|e| format!("open {}: {e}", path.display()))?;
                conn.busy_timeout(std::time::Duration::from_secs(30)).map_err(|e| e.to_string())?;
                db::backup::vacuum_into(&conn, dest).map(|_| ()).map_err(|e| e.to_string())
            }
        }
    }
}

/// What this process knows of the hub's state for this bot.
#[derive(Debug, Clone)]
struct Committed {
    /// The generation a next commit must follow.
    head: i64,
    /// When the latest role=state generation was committed (unix seconds).
    at: i64,
    /// Its objects: what is reused when unchanged.
    objects: Vec<ObjectEntry>,
}

static LAST: std::sync::Mutex<Option<Committed>> = std::sync::Mutex::new(None);
static IN_FLIGHT: AtomicBool = AtomicBool::new(false);
/// When commits started failing (unix seconds); 0 while they succeed.
static FAILING_SINCE: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// The bot's backup key from the environment, if this is a bot that ships.
/// `Some(Err)` is a key that is set but unusable — loud, not silent.
fn backup_key() -> Option<Result<[u8; 32], String>> {
    let hex_key = std::env::var("NEBO_BACKUP_KEY").ok()?;
    let bytes = match hex::decode(hex_key.trim()) {
        Ok(b) => b,
        Err(e) => return Some(Err(format!("NEBO_BACKUP_KEY is not hex: {e}"))),
    };
    let key: [u8; 32] = match bytes.try_into() {
        Ok(k) => k,
        Err(b) => return Some(Err(format!("NEBO_BACKUP_KEY is {} bytes, not 32", b.len()))),
    };
    Some(Ok(key))
}

/// The scheduler's minute tick: commit the durable set if it is due. The
/// commit runs in its own task, one at a time, so a slow upload never holds
/// up the housekeeping loop.
pub async fn commit_if_due(store: &Arc<Store>, state: &AppState) {
    let key = match backup_key() {
        None => return,
        Some(Ok(k)) => k,
        Some(Err(e)) => {
            warn!(error = %e, "backups cannot leave this computer");
            alarm(store, state, "backup-key-invalid", "Backups cannot leave this computer", &e);
            return;
        }
    };
    if IN_FLIGHT.swap(true, Ordering::AcqRel) {
        return;
    }
    let store = store.clone();
    let state = state.clone();
    tokio::spawn(async move {
        let result = commit_tick(&store, &state, &key, false).await.map(|_| ());
        IN_FLIGHT.store(false, Ordering::Release);
        let Err(e) = result else {
            FAILING_SINCE.store(0, Ordering::Relaxed);
            return;
        };
        warn!(error = %e, "state commit");
        let now = now_secs();
        let since = match FAILING_SINCE.compare_exchange(0, now, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => now,
            Err(t) => t,
        };
        if now - since > UNSHIPPED_ALARM_SECS {
            alarm(
                &store,
                &state,
                "backup-unshipped",
                "The latest backup has not left this computer",
                &format!("Saving this bot's state to NeboAI keeps failing: {e}. It is safe locally; it is not yet safe elsewhere."),
            );
        }
    });
}

/// The graceful drain's commit: once runs have stopped, whatever changed
/// since the last generation is committed now, without the 15-minute
/// spacing, and the hub's answer is the proof it holds it. Waits for a
/// commit the minute tick already started. A bot that ships no state
/// (no `NEBO_BACKUP_KEY`) has nothing to do.
pub async fn commit_on_drain(store: &Arc<Store>, state: &AppState) {
    let key = match backup_key() {
        None => return,
        Some(Ok(k)) => k,
        Some(Err(e)) => {
            warn!(error = %e, "drain: bot state not committed");
            return;
        }
    };
    while IN_FLIGHT.swap(true, Ordering::AcqRel) {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    let result = commit_tick(store, state, &key, true).await;
    IN_FLIGHT.store(false, Ordering::Release);
    match result {
        Ok(Some(m)) => info!(generation = m.generation, lease_epoch = m.lease_epoch, "drain: bot state committed and accepted by the hub"),
        Ok(None) => info!("drain: bot state unchanged since the last committed generation"),
        Err(e) => warn!(error = %e, "drain: bot state NOT committed; the last committed generation stands"),
    }
}

/// The lease epoch a commit is made under. A commit is made only while this
/// process holds the bot's lease: any other process may be the bot now, and
/// its state is the one that counts.
fn commit_epoch(lease: &comm::lease::Lease) -> Result<i64, String> {
    match lease.state() {
        comm::lease::LeaseState::Held { epoch, .. } => Ok(epoch as i64),
        other => Err(format!("this process does not hold the bot's lease ({other:?}); nothing committed")),
    }
}

/// One state commit if it is due; `drain` commits anything changed, without
/// the spacing. The committed manifest, or `None` when nothing was due.
async fn commit_tick(store: &Arc<Store>, state: &AppState, key: &[u8; 32], drain: bool) -> Result<Option<Manifest>, String> {
    let bot_id = config::read_bot_id().ok_or("this Nebo has no bot id")?;
    let token = crate::codes::neboai_token(state).ok_or("this Nebo is not signed in to NeboAI")?;
    let api = NeboAIApi::new(state.config.neboai.api_url.clone(), bot_id.clone(), token);
    let data_dir = config::data_dir().map_err(|e| e.to_string())?;

    let last = match known() {
        Some(l) => l,
        None => load_committed(&api).await?,
    };
    let sources = pack::sources(&data_dir, false);
    let prints = {
        let dir = data_dir.clone();
        let sources = sources.clone();
        tokio::task::spawn_blocking(move || fingerprints(&dir, &sources))
            .await
            .map_err(|e| e.to_string())??
    };
    let now = now_secs();
    if !commit_due(&last, &sources, &prints, now, drain) {
        return Ok(None);
    }
    let lease_epoch = commit_epoch(comm::lease::process())?;

    let residency = serde_json::json!({
        "idle": state.run_registry.list_all().await.is_empty(),
        "channel_bridges": state.channel_bridges.read().await.len(),
        "watch_bindings": agent::running_watchers(),
        "chromium_running": pack::chromium_running(&data_dir),
    });
    let next_wake = store.engine_next_timer_due().map_err(|e| e.to_string())?;
    let started = std::time::Instant::now();
    let manifest = commit(&CommitRequest {
        api: &api,
        key,
        bot_id: &bot_id,
        role: Role::State,
        data_dir: &data_dir,
        db: DbSource::Store(store.clone()),
        sources,
        prints,
        last: &last,
        lease_epoch,
        next_wake,
        residency: Some(residency),
    })
    .await?;
    info!(
        generation = manifest.generation,
        objects = manifest.objects.len(),
        ms = started.elapsed().as_millis() as u64,
        "bot state committed to the hub"
    );
    Ok(Some(manifest))
}

/// Due when nothing was ever committed, when something changed and the
/// last commit is 15 minutes old (at once, on the drain), or when a day has
/// passed.
fn commit_due(last: &Committed, sources: &[Source], prints: &[Fingerprint], now: i64, drain: bool) -> bool {
    if last.objects.is_empty() {
        return true;
    }
    let age = now - last.at;
    if age >= COMMIT_AT_LEAST_SECS {
        return true;
    }
    let changed = sources.len() != last.objects.len()
        || sources.iter().zip(prints).any(|(s, p)| reusable(&last.objects, s, p).is_none());
    changed && (drain || age >= COMMIT_SPACING_SECS)
}

/// The committed entry for this object, if its files have not changed.
fn reusable<'a>(objects: &'a [ObjectEntry], source: &Source, print: &Fingerprint) -> Option<&'a ObjectEntry> {
    objects.iter().find(|o| o.role == source.role && o.root == source.root && &o.fingerprint == print)
}

fn fingerprints(data_dir: &Path, sources: &[Source]) -> Result<Vec<Fingerprint>, String> {
    sources
        .iter()
        .map(|s| pack::fingerprint(data_dir, s).map_err(|e| format!("scan {}: {e}", s.root)))
        .collect()
}

fn known() -> Option<Committed> {
    LAST.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

fn remember(c: Option<Committed>) {
    *LAST.lock().unwrap_or_else(|p| p.into_inner()) = c;
}

/// What the hub holds for this bot: the head, and the latest state's objects.
async fn load_committed(api: &NeboAIApi) -> Result<Committed, String> {
    let resp = api.bot_state(None).await.map_err(|e| format!("read committed state: {e}"))?;
    let (at, objects) = match resp.state {
        Some(st) => {
            let m: Manifest = serde_json::from_value(st.manifest).map_err(|e| format!("committed manifest: {e}"))?;
            let at = chrono::DateTime::parse_from_rfc3339(&st.committed_at).map(|t| t.timestamp()).unwrap_or(0);
            (at, m.objects)
        }
        None => (0, Vec::new()),
    };
    let c = Committed { head: resp.head, at, objects };
    remember(Some(c.clone()));
    Ok(c)
}

/// Everything one commit needs.
struct CommitRequest<'a> {
    api: &'a NeboAIApi,
    key: &'a [u8; 32],
    bot_id: &'a str,
    role: Role,
    data_dir: &'a Path,
    db: DbSource,
    sources: Vec<Source>,
    prints: Vec<Fingerprint>,
    last: &'a Committed,
    /// The lease epoch this commit is made under (0: a process that holds
    /// no lease, the archive command).
    lease_epoch: i64,
    next_wake: Option<i64>,
    residency: Option<serde_json::Value>,
}

/// Pack what changed, upload what the hub does not have, commit generation
/// head+1. Returns the committed manifest.
async fn commit(req: &CommitRequest<'_>) -> Result<Manifest, String> {
    let staging = req.data_dir.join(pack::STAGING_DIR);
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| format!("create {}: {e}", staging.display()))?;
    let result = commit_staged(req, &staging).await;
    let _ = std::fs::remove_dir_all(&staging);
    result
}

async fn commit_staged(req: &CommitRequest<'_>, staging: &Path) -> Result<Manifest, String> {
    let taken_at = now_secs();
    // Every chunk the latest generation names is kept by the hub's retention
    // rule, so naming it again needs no upload.
    let known: HashMap<String, ChunkRef> = req
        .last
        .objects
        .iter()
        .flat_map(|o| o.chunks.iter())
        .map(|c| (c.sha256.clone(), c.clone()))
        .collect();

    let mut objects = Vec::with_capacity(req.sources.len());
    for (source, print) in req.sources.iter().zip(&req.prints) {
        if let Some(prev) = reusable(&req.last.objects, source, print) {
            objects.push(prev.clone());
            continue;
        }
        let db_copy = if source.role == "database" {
            let dest = staging.join("nebo.db");
            let (db, to) = (req.db.clone(), dest.clone());
            tokio::task::spawn_blocking(move || db.copy_to(&to)).await.map_err(|e| e.to_string())??;
            Some(dest)
        } else {
            None
        };
        let packed = {
            let (dir, source, out, key, bot_id) =
                (req.data_dir.to_path_buf(), source.clone(), staging.to_path_buf(), *req.key, req.bot_id.to_string());
            let db_copy = db_copy.clone();
            tokio::task::spawn_blocking(move || {
                pack::pack_object(&dir, &source, db_copy.as_deref(), &out, &key, &ChunkMeta { bot_id: &bot_id, taken_at }, pack::CHUNK_PLAIN_MAX)
            })
            .await
            .map_err(|e| e.to_string())??
        };
        if let Some(copy) = db_copy {
            let _ = std::fs::remove_file(copy);
        }
        let mut chunks = Vec::with_capacity(packed.chunks.len());
        for c in &packed.chunks {
            let file_id = match known.get(&c.sha256) {
                Some(k) => k.file_id.clone(),
                None => upload_chunk(req.api, &c.path, &c.sha256, taken_at).await?,
            };
            let _ = std::fs::remove_file(&c.path);
            chunks.push(ChunkRef { file_id, sha256: c.sha256.clone(), bytes: c.bytes });
        }
        objects.push(ObjectEntry {
            role: source.role.clone(),
            root: source.root.clone(),
            sha256: packed.sha256,
            bytes: packed.bytes,
            fingerprint: print.clone(),
            chunks,
        });
    }

    let manifest = Manifest {
        bot_id: req.bot_id.to_string(),
        generation: req.last.head + 1,
        lease_epoch: req.lease_epoch,
        role: req.role.as_str().to_string(),
        taken_at: rfc3339(taken_at),
        nebo_version: env!("CARGO_PKG_VERSION").to_string(),
        next_wake: req.next_wake.map(rfc3339),
        key_version: KEY_VERSION,
        objects,
        residency_signals: req.residency.clone(),
    };
    let body = serde_json::to_value(&manifest).map_err(|e| e.to_string())?;
    match req.api.commit_bot_state(&body).await {
        Ok(resp) => {
            let mut next = req.last.clone();
            next.head = resp.generation;
            if req.role == Role::State {
                next.at = taken_at;
                next.objects = manifest.objects.clone();
            }
            remember(Some(next));
            Ok(manifest)
        }
        Err(e) => {
            // Whatever the hub now holds (another writer's generation, or
            // this one if only the answer was lost) is read fresh next time.
            remember(None);
            Err(format!("commit generation {}: {e}", manifest.generation))
        }
    }
}

/// One chunk through the files door. The door admits 20 uploads a minute;
/// a refusal waits the window out and tries again.
async fn upload_chunk(api: &NeboAIApi, path: &Path, sha256: &str, taken_at: i64) -> Result<String, String> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "chunk.tar".into());
    let fields = [
        ("purpose".to_string(), "backup".to_string()),
        ("kind".to_string(), "chunk".to_string()),
        ("taken_at".to_string(), taken_at.to_string()),
        ("sha256".to_string(), sha256.to_string()),
        ("key_version".to_string(), KEY_VERSION.to_string()),
    ];
    let mut attempt = 0;
    loop {
        let data = tokio::fs::read(path).await.map_err(|e| format!("read {}: {e}", path.display()))?;
        match api.upload_file(&name, "application/x-tar", data, &fields).await {
            Ok(att) => return Ok(att.file_id),
            Err(CommError::Http { status: 429, .. }) if attempt < UPLOAD_RATE_RETRIES => {
                attempt += 1;
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            }
            Err(e) => return Err(format!("upload {name}: {e}")),
        }
    }
}

/// How a process that is not the running server reaches the hub as this
/// bot: the id from the environment or the data directory, the freshest
/// token there is (the rotated-token cache, else the provisioned one, else
/// the boot credential). A read refused as stale retries with the boot
/// credential inside `NeboAIApi`.
fn hub_credentials(api_url: &str, data_dir: &Path) -> Option<NeboAIApi> {
    let bot_id = std::env::var("NEBO_BOT_ID").ok().filter(|s| s.len() == 36).or_else(config::read_bot_id)?;
    let token = std::fs::read_to_string(data_dir.join("neboai_token.cache"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("NEBO_BOT_TOKEN").ok().filter(|s| !s.is_empty()))
        .or_else(comm::api::provisioned_credential)?;
    Some(NeboAIApi::new(api_url.to_string(), bot_id, token))
}

/// `nebo state archive`: commit the whole data directory as a role=archive
/// generation the hub keeps for 30 days. Runs beside the live server, which
/// keeps its database open; the copy goes through its own read-only
/// connection.
pub async fn archive(api_url: &str) -> Result<Manifest, String> {
    let key = backup_key().ok_or("NEBO_BACKUP_KEY is not set")??;
    let data_dir = config::data_dir().map_err(|e| e.to_string())?;
    let api = hub_credentials(api_url, &data_dir).ok_or("no bot id and token to reach NeboAI with")?;
    let bot_id = api.bot_id().to_string();
    let last = load_committed(&api).await?;
    let sources = pack::sources(&data_dir, true);
    let prints = {
        let (dir, sources) = (data_dir.clone(), sources.clone());
        tokio::task::spawn_blocking(move || fingerprints(&dir, &sources)).await.map_err(|e| e.to_string())??
    };
    let db_path = data_dir.join(pack::DATABASE_PATH);
    // Reuse the latest state's chunks where they match; an archive otherwise
    // packs everything.
    let base = Committed { head: last.head, at: last.at, objects: last.objects.clone() };
    commit(&CommitRequest {
        api: &api,
        key: &key,
        bot_id: &bot_id,
        role: Role::Archive,
        data_dir: &data_dir,
        db: DbSource::File(db_path),
        sources,
        prints,
        last: &base,
        lease_epoch: 0,
        next_wake: None,
        residency: None,
    })
    .await
}

/// Before the database opens: a server whose database is missing restores
/// the latest committed generation into its data directory. No generation
/// means a fresh bot. Any failure is returned, and the caller must exit
/// non-zero — never start empty over a bot that has state.
pub async fn restore_on_boot(api_url: &str) -> Result<(), String> {
    if std::env::var_os("NEBO_SERVER_MODE").is_none() {
        return Ok(());
    }
    let data_dir = config::data_dir().map_err(|e| e.to_string())?;
    if data_dir.join(pack::DATABASE_PATH).exists() {
        return Ok(());
    }
    let Some(api) = hub_credentials(api_url, &data_dir) else {
        info!("no database and no NeboAI identity: starting as a new Nebo");
        return Ok(());
    };
    let key = backup_key().transpose()?;
    until_released(&api, comm::lease::RENEW_EVERY).await?;
    match restore(&api, None, &data_dir, key).await? {
        Some(generation) => info!(generation, "bot state restored from NeboAI"),
        None => info!("no committed state: starting as a new bot"),
    }
    Ok(())
}

/// Resolves once no running copy holds the bot's lease, asking every
/// `every`. A copy still running may yet commit: a drained pod commits its
/// last generation and only then hands the lease back, and a killed one's
/// lease lapses after its last commit. Restoring before that would start this
/// copy from an older generation, and its commits would bury the newer one.
async fn until_released(api: &NeboAIApi, every: std::time::Duration) -> Result<(), String> {
    loop {
        let st = api.bot_state(None).await.map_err(|e| format!("read committed state: {e}"))?;
        if !st.lease_live {
            return Ok(());
        }
        info!(head = st.head, "another copy of this bot still holds it; restoring once it hands the bot back");
        tokio::time::sleep(every).await;
    }
}

/// `nebo state restore`: restore a generation (the latest state by default)
/// into a directory that is not a live bot's — the proof that a copy opens.
pub async fn restore_into(api_url: &str, generation: Option<i64>, into: &Path) -> Result<i64, String> {
    if into.join(pack::DATABASE_PATH).exists() {
        return Err(format!("{} already holds a database; restore into an empty directory", into.display()));
    }
    let data_dir = config::data_dir().map_err(|e| e.to_string())?;
    let api = hub_credentials(api_url, &data_dir).ok_or("no bot id and token to reach NeboAI with")?;
    std::fs::create_dir_all(into).map_err(|e| format!("create {}: {e}", into.display()))?;
    let key = backup_key().transpose()?;
    restore(&api, generation, into, key).await?.ok_or_else(|| "this bot has no committed state".to_string())
}

/// Download, verify, decrypt and unpack a generation into `into`. `None`
/// when the bot has never committed state.
async fn restore(api: &NeboAIApi, generation: Option<i64>, into: &Path, key: Option<[u8; 32]>) -> Result<Option<i64>, String> {
    let resp = api.bot_state(generation).await.map_err(|e| format!("read committed state: {e}"))?;
    let Some(st) = resp.state else {
        if generation.is_some() {
            return Err(format!("generation {} does not exist", generation.unwrap_or_default()));
        }
        if resp.head > 0 {
            return Err(format!(
                "this bot has {} generation(s) but no state generation to restore; restore an archive by number",
                resp.head
            ));
        }
        return Ok(None);
    };
    let manifest: Manifest = serde_json::from_value(st.manifest).map_err(|e| format!("committed manifest: {e}"))?;
    let Some(key) = key else {
        return Err(format!("generation {} exists but NEBO_BACKUP_KEY is not set", st.generation));
    };
    let staging = into.join(pack::RESTORE_DIR);
    let _ = std::fs::remove_dir_all(&staging);
    let tree = staging.join("tree");
    std::fs::create_dir_all(&tree).map_err(|e| format!("create {}: {e}", tree.display()))?;
    let result = restore_staged(api, &manifest, &key, &staging, &tree, into).await;
    let _ = std::fs::remove_dir_all(&staging);
    result.map(|_| Some(st.generation))
}

async fn restore_staged(api: &NeboAIApi, manifest: &Manifest, key: &[u8; 32], staging: &Path, tree: &Path, into: &Path) -> Result<(), String> {
    for (i, obj) in manifest.objects.iter().enumerate() {
        let gz = staging.join(format!("object-{i}.gz"));
        {
            use std::io::Write;
            let mut out = std::fs::File::create(&gz).map_err(|e| format!("create {}: {e}", gz.display()))?;
            for c in &obj.chunks {
                let bytes = api.download_file(&c.file_id).await.map_err(|e| format!("download chunk {}: {e}", c.file_id))?;
                let (key, sha) = (*key, c.sha256.clone());
                let plain = tokio::task::spawn_blocking(move || pack::open_chunk(&bytes, &key, &sha))
                    .await
                    .map_err(|e| e.to_string())?
                    .map_err(|e| format!("{} {}: {e}", obj.role, obj.root))?;
                out.write_all(&plain).map_err(|e| format!("write {}: {e}", gz.display()))?;
            }
        }
        let (gz2, tree2, sha, bytes) = (gz.clone(), tree.to_path_buf(), obj.sha256.clone(), obj.bytes);
        tokio::task::spawn_blocking(move || pack::unpack_object(&gz2, &tree2, &sha, bytes))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("{} {}: {e}", obj.role, obj.root))?;
        let _ = std::fs::remove_file(&gz);
    }
    let db = tree.join(pack::DATABASE_PATH);
    if db.exists() {
        match db::backup::integrity_of(&db) {
            Ok(v) if v == "ok" => {}
            Ok(v) => return Err(format!("restored database failed its integrity check: {v}")),
            Err(e) => return Err(format!("restored database: {e}")),
        }
    }
    move_into_place(tree, into)
}

/// Move the unpacked tree into the data directory, the database last: until
/// it lands, the directory does not look like a bot, and a restore that is
/// interrupted simply runs again on the next boot.
fn move_into_place(tree: &Path, into: &Path) -> Result<(), String> {
    let mut names: Vec<String> = std::fs::read_dir(tree)
        .map_err(|e| format!("read {}: {e}", tree.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    let place = |from: &Path, to: &Path| -> Result<(), String> {
        if let Ok(meta) = std::fs::symlink_metadata(to) {
            let removed = if meta.is_dir() { std::fs::remove_dir_all(to) } else { std::fs::remove_file(to) };
            removed.map_err(|e| format!("clear {}: {e}", to.display()))?;
        }
        std::fs::rename(from, to).map_err(|e| format!("move {} into place: {e}", to.display()))
    };
    for name in &names {
        if name == "data" {
            continue;
        }
        place(&tree.join(name), &into.join(name))?;
    }
    let data = tree.join("data");
    if data.is_dir() {
        std::fs::create_dir_all(into.join("data")).map_err(|e| format!("create data: {e}"))?;
        let mut inner: Vec<String> = std::fs::read_dir(&data)
            .map_err(|e| format!("read {}: {e}", data.display()))?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n != "nebo.db")
            .collect();
        inner.sort();
        for name in inner {
            place(&data.join(&name), &into.join("data").join(&name))?;
        }
        if data.join("nebo.db").exists() {
            place(&data.join("nebo.db"), &into.join(pack::DATABASE_PATH))?;
        }
    }
    Ok(())
}

/// Tell the owner, once a day per problem.
fn alarm(store: &Arc<Store>, state: &AppState, what: &str, title: &str, body: &str) {
    let day = now_secs() / 86_400;
    tools::owner_notify::emit(
        store,
        Some(&|name: &str, payload: serde_json::Value| state.hub.broadcast(name, payload)),
        &tools::owner_notify::OwnerNotification {
            id: &format!("{what}:{day}"),
            kind: "error",
            title,
            body: Some(body),
            action_url: None,
            agent_id: None,
            loud: true,
        },
    );
}

fn rfc3339(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Mutex;

    use axum::extract::{Multipart, Path as UrlPath, Query, State};
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::routing::{get, post};
    use axum::{Json, Router};

    const KEY: [u8; 32] = [9u8; 32];
    const BOT: &str = "00000000-0000-4000-8000-00000000b075";

    /// The hub's side of BotState, in memory: the files door (idempotent on
    /// sha256), the file read, and the state routes with the head CAS.
    #[derive(Default)]
    struct Hub {
        files: HashMap<String, Vec<u8>>,
        by_sha: HashMap<String, String>,
        uploads: usize,
        head: i64,
        gens: BTreeMap<i64, (String, serde_json::Value)>,
        lease_live: bool,
    }
    type Shared = Arc<Mutex<Hub>>;

    async fn upload(State(hub): State<Shared>, mut form: Multipart) -> impl IntoResponse {
        let mut fields = HashMap::new();
        let mut data = Vec::new();
        while let Some(field) = form.next_field().await.unwrap() {
            let name = field.name().unwrap_or_default().to_string();
            if name == "file" {
                data = field.bytes().await.unwrap().to_vec();
            } else {
                fields.insert(name, field.text().await.unwrap());
            }
        }
        assert_eq!(fields.get("purpose").map(String::as_str), Some("backup"));
        assert_eq!(fields.get("kind").map(String::as_str), Some("chunk"));
        assert!(data.len() < 100 << 20, "the door's ceiling");
        let sha = fields["sha256"].clone();
        let mut h = hub.lock().unwrap();
        let id = match h.by_sha.get(&sha) {
            Some(id) => id.clone(),
            None => {
                let id = uuid::Uuid::new_v4().to_string();
                h.files.insert(id.clone(), data.clone());
                h.by_sha.insert(sha, id.clone());
                h.uploads += 1;
                id
            }
        };
        (StatusCode::CREATED, Json(serde_json::json!({"fileId": id, "filename": "c.tar", "mimeType": "application/x-tar", "size": data.len(), "url": ""})))
    }

    async fn file(State(hub): State<Shared>, UrlPath(id): UrlPath<String>) -> impl IntoResponse {
        match hub.lock().unwrap().files.get(&id) {
            Some(b) => (StatusCode::OK, b.clone()).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    }

    async fn read_state(State(hub): State<Shared>, Query(q): Query<HashMap<String, String>>) -> impl IntoResponse {
        let h = hub.lock().unwrap();
        let pick = match q.get("generation") {
            Some(g) => h.gens.get_key_value(&g.parse::<i64>().unwrap()),
            None => h.gens.iter().rev().find(|(_, (role, _))| role == "state"),
        };
        let state = pick.map(|(g, (role, m))| {
            serde_json::json!({"generation": g, "role": role, "lease_epoch": 0, "committed_at": "2026-09-22T12:00:00Z", "manifest": m})
        });
        Json(serde_json::json!({"head": h.head, "state": state, "lease_live": h.lease_live}))
    }

    async fn commit_state(State(hub): State<Shared>, Json(m): Json<serde_json::Value>) -> impl IntoResponse {
        let mut h = hub.lock().unwrap();
        let g = m["generation"].as_i64().unwrap();
        if g != h.head + 1 {
            return (StatusCode::CONFLICT, Json(serde_json::json!({"error": "generation conflict"})));
        }
        for o in m["objects"].as_array().unwrap() {
            for c in o["chunks"].as_array().unwrap() {
                if !h.files.contains_key(c["file_id"].as_str().unwrap()) {
                    return (StatusCode::UNPROCESSABLE_ENTITY, Json(serde_json::json!({"error": "not a stored chunk"})));
                }
            }
        }
        h.head = g;
        h.gens.insert(g, (m["role"].as_str().unwrap().to_string(), m));
        (StatusCode::CREATED, Json(serde_json::json!({"generation": g, "committed_at": "2026-09-22T12:00:00Z"})))
    }

    async fn fake_hub() -> (String, Shared) {
        let hub: Shared = Arc::default();
        let app = Router::new()
            .route("/api/v1/files/upload", post(upload))
            .route("/api/v1/files/{id}", get(file))
            .route("/api/v1/bots/self/state", get(read_state).post(commit_state))
            .layer(axum::extract::DefaultBodyLimit::max(128 << 20))
            .with_state(hub.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), hub)
    }

    /// A bot's data directory: the durable set, some scratch, a real database.
    fn bot_dir() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        for dir in ["files/docs", "sessions", "nebo/plugin-profiles/gws", "user/agents", "data/backups", "logs", "home/go/pkg"] {
            std::fs::create_dir_all(p.join(dir)).unwrap();
        }
        std::fs::write(p.join("files/docs/a.txt"), b"a document").unwrap();
        std::fs::write(p.join("sessions/cp.json"), b"{\"step\":3}").unwrap();
        std::fs::write(p.join("nebo/plugin-profiles/gws/creds.json"), b"{\"refresh\":\"r\"}").unwrap();
        std::fs::write(p.join("user/agents/AGENT.md"), b"# Bookkeeper").unwrap();
        std::fs::write(p.join("settings.json"), b"{\"access_secret\":\"s\"}").unwrap();
        std::fs::write(p.join("bot_id"), BOT).unwrap();
        std::fs::write(p.join("logs/nebo.log"), b"scratch").unwrap();
        std::fs::write(p.join("home/go/pkg/mod.zip"), b"scratch").unwrap();
        let conn = rusqlite::Connection::open(p.join(pack::DATABASE_PATH)).unwrap();
        conn.execute_batch("PRAGMA journal_mode=DELETE; CREATE TABLE t(id INTEGER PRIMARY KEY, body TEXT);").unwrap();
        for i in 0..500 {
            conn.execute("INSERT INTO t(body) VALUES (?1)", [format!("row {i}")]).unwrap();
        }
        d
    }

    async fn commit_as(api: &NeboAIApi, dir: &Path, role: Role) -> Result<Manifest, String> {
        let last = load_committed(api).await?;
        let sources = pack::sources(dir, role == Role::Archive);
        let prints = fingerprints(dir, &sources)?;
        commit(&CommitRequest {
            api,
            key: &KEY,
            bot_id: BOT,
            role,
            data_dir: dir,
            db: DbSource::File(dir.join(pack::DATABASE_PATH)),
            sources,
            prints,
            last: &last,
            lease_epoch: 7,
            next_wake: Some(1_789_800_000),
            residency: Some(serde_json::json!({"idle": true})),
        })
        .await
    }

    fn read(p: &Path) -> Vec<u8> {
        std::fs::read(p).unwrap_or_default()
    }

    /// Commit, commit again unchanged, change one file, restore: the restored
    /// directory holds the durable set byte for byte, and nothing else.
    #[tokio::test]
    async fn a_restore_waits_for_the_running_copy_to_hand_the_bot_back() {
        let (url, hub) = fake_hub().await;
        let api = NeboAIApi::new(url.clone(), BOT.into(), "tok".into());
        let dir = bot_dir();
        commit_as(&api, dir.path(), Role::State).await.unwrap();
        hub.lock().unwrap().lease_live = true;

        // The running copy drains: its last generation lands, then the lease
        // is handed back.
        let draining = {
            let (api, hub, dir) = (NeboAIApi::new(url.clone(), BOT.into(), "tok".into()), hub.clone(), dir.path().to_path_buf());
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                std::fs::write(dir.join("files/last-words.txt"), b"written before the drain").unwrap();
                let m = commit_as(&api, &dir, Role::State).await.unwrap();
                hub.lock().unwrap().lease_live = false;
                m.generation
            })
        };
        until_released(&api, std::time::Duration::from_millis(20)).await.unwrap();
        let last = draining.await.unwrap();

        let into = tempfile::tempdir().unwrap();
        let restored = restore(&api, None, into.path(), Some(KEY)).await.unwrap();
        assert_eq!(restored, Some(last), "the restore starts from the drain's generation");
        assert_eq!(read(&into.path().join("files/last-words.txt")), b"written before the drain");
    }

    #[tokio::test]
    async fn commit_then_restore_round_trips_the_durable_set() {
        let (url, hub) = fake_hub().await;
        let api = NeboAIApi::new(url, BOT.into(), "test-token".into());
        let src = bot_dir();

        let m1 = commit_as(&api, src.path(), Role::State).await.unwrap();
        assert_eq!(m1.generation, 1);
        assert_eq!(m1.next_wake.as_deref(), Some("2026-09-19T06:40:00Z"));
        let roots: Vec<&str> = m1.objects.iter().map(|o| o.root.as_str()).collect();
        assert_eq!(roots, vec!["data/nebo.db", ".", "files", "user", "sessions", "nebo"]);
        let first_uploads = hub.lock().unwrap().uploads;
        assert_eq!(first_uploads, 6, "one chunk per object");

        // Nothing changed: every object is reused, nothing is uploaded.
        let m2 = commit_as(&api, src.path(), Role::State).await.unwrap();
        assert_eq!(m2.generation, 2);
        assert_eq!(hub.lock().unwrap().uploads, first_uploads);
        assert_eq!(m2.objects, m1.objects);

        // One file changed: only its object is packed and uploaded again.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(src.path().join("files/docs/a.txt"), b"a document, revised").unwrap();
        let m3 = commit_as(&api, src.path(), Role::State).await.unwrap();
        assert_eq!(hub.lock().unwrap().uploads, first_uploads + 1);
        let changed: Vec<&str> = m3.objects.iter().zip(&m2.objects).filter(|(a, b)| a != b).map(|(a, _)| a.root.as_str()).collect();
        assert_eq!(changed, vec!["files"]);

        // Restore into an empty directory.
        let dst = tempfile::tempdir().unwrap();
        assert_eq!(restore(&api, None, dst.path(), Some(KEY)).await.unwrap(), Some(3));
        for f in ["files/docs/a.txt", "sessions/cp.json", "nebo/plugin-profiles/gws/creds.json", "user/agents/AGENT.md", "settings.json", "bot_id"] {
            assert_eq!(read(&dst.path().join(f)), read(&src.path().join(f)), "{f}");
        }
        assert!(!dst.path().join("logs").exists() && !dst.path().join("home").exists(), "scratch is not state");
        assert!(!dst.path().join(pack::RESTORE_DIR).exists(), "staging is cleaned up");
        let db = dst.path().join(pack::DATABASE_PATH);
        assert_eq!(db::backup::integrity_of(&db).unwrap(), "ok");
        let rows: i64 = rusqlite::Connection::open(&db).unwrap().query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(rows, 500);
    }

    /// Two writers from the same head: the second is refused and the head
    /// stays where the first put it.
    #[tokio::test]
    async fn a_stale_writer_loses_the_cas() {
        let (url, hub) = fake_hub().await;
        let api = NeboAIApi::new(url, BOT.into(), "test-token".into());
        let src = bot_dir();
        let stale = load_committed(&api).await.unwrap();
        commit_as(&api, src.path(), Role::State).await.unwrap();
        let sources = pack::sources(src.path(), false);
        let prints = fingerprints(src.path(), &sources).unwrap();
        let err = commit(&CommitRequest {
            api: &api,
            key: &KEY,
            bot_id: BOT,
            role: Role::State,
            data_dir: src.path(),
            db: DbSource::File(src.path().join(pack::DATABASE_PATH)),
            sources,
            prints,
            last: &stale,
            lease_epoch: 7,
            next_wake: None,
            residency: None,
        })
        .await
        .unwrap_err();
        assert!(err.contains("409"), "{err}");
        assert_eq!(hub.lock().unwrap().head, 1);
    }

    /// A wrong key, a tampered chunk, or a missing key fails the restore,
    /// and a failed restore never leaves a database behind.
    #[tokio::test]
    async fn a_restore_that_cannot_verify_fails_and_leaves_no_database() {
        let (url, hub) = fake_hub().await;
        let api = NeboAIApi::new(url, BOT.into(), "test-token".into());
        let src = bot_dir();
        commit_as(&api, src.path(), Role::State).await.unwrap();

        let dst = tempfile::tempdir().unwrap();
        let err = restore(&api, None, dst.path(), Some([1u8; 32])).await.unwrap_err();
        assert!(err.contains("decrypt"), "{err}");
        assert!(!dst.path().join(pack::DATABASE_PATH).exists());

        let err = restore(&api, None, dst.path(), None).await.unwrap_err();
        assert!(err.contains("NEBO_BACKUP_KEY"), "{err}");

        // Flip a byte in every stored chunk's sealed piece.
        for bytes in hub.lock().unwrap().files.values_mut() {
            let at = pack::tests::sealed_offset(bytes) + 20;
            bytes[at] ^= 0x55;
        }
        let err = restore(&api, None, dst.path(), Some(KEY)).await.unwrap_err();
        assert!(err.contains("decrypt") || err.contains("checksum"), "{err}");
        assert!(!dst.path().join(pack::DATABASE_PATH).exists());
        assert!(!dst.path().join(pack::RESTORE_DIR).exists());
    }

    /// A bot that never committed restores to nothing: a fresh bot.
    #[tokio::test]
    async fn no_committed_state_is_a_fresh_bot() {
        let (url, _hub) = fake_hub().await;
        let api = NeboAIApi::new(url, BOT.into(), "test-token".into());
        let dst = tempfile::tempdir().unwrap();
        assert_eq!(restore(&api, None, dst.path(), Some(KEY)).await.unwrap(), None);
        assert!(std::fs::read_dir(dst.path()).unwrap().next().is_none(), "nothing written");
    }

    /// An archive holds the whole data directory, is not what a boot
    /// restore reads, and restores by number.
    #[tokio::test]
    async fn an_archive_is_everything_and_restores_by_number() {
        let (url, _hub) = fake_hub().await;
        let api = NeboAIApi::new(url, BOT.into(), "test-token".into());
        let src = bot_dir();
        commit_as(&api, src.path(), Role::State).await.unwrap();
        let archive = commit_as(&api, src.path(), Role::Archive).await.unwrap();
        assert_eq!((archive.generation, archive.role.as_str()), (2, "archive"));

        let latest = api.bot_state(None).await.unwrap();
        assert_eq!((latest.head, latest.state.unwrap().generation), (2, 1), "a restore reads the state, not the archive");

        let dst = tempfile::tempdir().unwrap();
        assert_eq!(restore(&api, Some(2), dst.path(), Some(KEY)).await.unwrap(), Some(2));
        for f in ["logs/nebo.log", "home/go/pkg/mod.zip", "files/docs/a.txt", "settings.json"] {
            assert_eq!(read(&dst.path().join(f)), read(&src.path().join(f)), "{f}");
        }
        assert_eq!(db::backup::integrity_of(&dst.path().join(pack::DATABASE_PATH)).unwrap(), "ok");
    }

    /// Due when never committed; when changed and 15 minutes old; after a
    /// day regardless — and not otherwise.
    #[test]
    fn the_commit_cadence() {
        let src = bot_dir();
        let sources = pack::sources(src.path(), false);
        let prints = fingerprints(src.path(), &sources).unwrap();
        let objects: Vec<ObjectEntry> = sources
            .iter()
            .zip(&prints)
            .map(|(s, p)| ObjectEntry { role: s.role.clone(), root: s.root.clone(), sha256: String::new(), bytes: 0, fingerprint: p.clone(), chunks: vec![] })
            .collect();
        let now = 1_800_000_000;
        let at = |age: i64| Committed { head: 1, at: now - age, objects: objects.clone() };
        assert!(commit_due(&Committed { head: 0, at: 0, objects: vec![] }, &sources, &prints, now, false), "never committed");
        assert!(!commit_due(&at(20 * 60), &sources, &prints, now, false), "unchanged");
        assert!(commit_due(&at(25 * 3600), &sources, &prints, now, false), "a day has passed");
        let mut changed = prints.clone();
        changed[2].max_mtime_ns += 1;
        assert!(!commit_due(&at(5 * 60), &sources, &changed, now, false), "changed, but committed 5 minutes ago");
        assert!(commit_due(&at(16 * 60), &sources, &changed, now, false), "changed and 16 minutes old");
        // The drain commits anything changed at once, and nothing unchanged.
        assert!(commit_due(&at(5 * 60), &sources, &changed, now, true), "drain: changed 5 minutes after a commit");
        assert!(!commit_due(&at(5 * 60), &sources, &prints, now, true), "drain: unchanged");
    }

    /// A commit is made only under a held lease, and carries its epoch.
    #[test]
    fn a_commit_needs_the_lease_and_carries_its_epoch() {
        use comm::lease::Lease;
        let lease = Lease::new();
        assert!(commit_epoch(&lease).is_err(), "unclaimed");
        lease.set_fenced(true);
        lease.claim();
        assert!(commit_epoch(&lease).is_err(), "claimed, not yet granted");
        lease.granted(42, std::time::Duration::from_secs(60), std::time::Instant::now());
        assert_eq!(commit_epoch(&lease), Ok(42));
        lease.lost();
        assert!(commit_epoch(&lease).is_err(), "lost");
        let unleased = Lease::new();
        unleased.claim();
        unleased.granted(0, std::time::Duration::from_secs(60), std::time::Instant::now());
        assert!(commit_epoch(&unleased).is_err(), "a hub that issues no leases fences no commit");
    }
}
