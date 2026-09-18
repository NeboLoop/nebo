//! A verified copy leaves the machine.
//!
//! The ring (`db::backup`) is a local safety net: it lives on the same disk
//! as the database it copies. A cloud bot's disk can be lost whole, so every
//! verified copy is also packed — gzipped, then encrypted with the bot's own
//! key — and uploaded through the ONE file door the hub has
//! (`POST /api/v1/files/upload`, `purpose=backup`). The hub catalogues it
//! under the bot; the key's durable copy is wrapped in the hub's catalogue,
//! so a bot can be rebuilt from Spaces without this pod.
//!
//! The key arrives as `NEBO_BACKUP_KEY` (hex, 32 bytes) in the pod's Secret.
//! Without it nothing ships — a desktop keeps its ring local.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;

use flate2::write::GzEncoder;
use flate2::Compression;
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::state::AppState;
use db::Store;
use types::NeboError;

/// The wrapping scheme the hub records beside each copy; a key rotation bumps it.
const KEY_VERSION: u32 = 1;
/// The newest verified copy should be in Spaces within this long, or the owner hears.
const UNSHIPPED_ALARM_SECS: i64 = 2 * 3600;

/// One packed copy, ready to upload.
pub struct Packed {
    pub tar: Vec<u8>,
    /// Of the plaintext database file — the hub's idempotency key.
    pub sha256: String,
    pub name: String,
}

/// Pack a verified copy: gzip while hashing, encrypt (AES-256-GCM, 12-byte
/// nonce first), and tar it beside a manifest that says what it is.
// ponytail: whole copy in memory (compressed ~3x: gz + ciphertext + tar, ~80 MB
// for the fleet's largest database today); stream to a temp file if a bot
// outgrows the pod.
pub fn pack(db_copy: &Path, bot_id: &str, taken_at: i64, key: &[u8; 32]) -> Result<Packed, NeboError> {
    let mut file = std::fs::File::open(db_copy)
        .map_err(|e| NeboError::Internal(format!("open {}: {e}", db_copy.display())))?;
    let mut hasher = Sha256::new();
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| NeboError::Internal(format!("read {}: {e}", db_copy.display())))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        gz.write_all(&buf[..n])
            .map_err(|e| NeboError::Internal(format!("gzip: {e}")))?;
    }
    let gz_bytes = gz
        .finish()
        .map_err(|e| NeboError::Internal(format!("gzip finish: {e}")))?;
    let sha256 = hex::encode(hasher.finalize());
    let sealed = mcp::crypto::Encryptor::new(*key)
        .encrypt(&gz_bytes)
        .map_err(|e| NeboError::Internal(format!("encrypt: {e}")))?;
    drop(gz_bytes);

    let stamp = chrono::DateTime::from_timestamp(taken_at, 0)
        .map(|t| t.format("%Y%m%dT%H%M%SZ").to_string())
        .unwrap_or_else(|| taken_at.to_string());
    let inner_name = format!("nebo-{stamp}.db.gz.enc");
    let manifest = serde_json::json!({
        "bot_id": bot_id,
        "taken_at": taken_at,
        "sha256": sha256,
        "key_version": KEY_VERSION,
        "cipher": "aes-256-gcm",
        "layout": "nonce(12) || ciphertext",
        "compression": "gzip",
        "file": inner_name,
        "nebo_version": env!("CARGO_PKG_VERSION"),
    })
    .to_string();

    let mut ar = tar::Builder::new(Vec::new());
    for (name, bytes) in [("manifest.json", manifest.as_bytes()), (inner_name.as_str(), sealed.as_slice())] {
        let mut h = tar::Header::new_gnu();
        h.set_size(bytes.len() as u64);
        h.set_mode(0o600);
        h.set_mtime(taken_at.max(0) as u64);
        h.set_cksum();
        ar.append_data(&mut h, name, bytes)
            .map_err(|e| NeboError::Internal(format!("tar {name}: {e}")))?;
    }
    let tar = ar
        .into_inner()
        .map_err(|e| NeboError::Internal(format!("tar finish: {e}")))?;
    Ok(Packed { tar, sha256, name: format!("nebo-{stamp}.tar") })
}

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

/// Ship the newest verified copy that has not reached the hub. One per call:
/// the scheduler calls this every minute, so a backlog drains in minutes and
/// a failure is retried on the next tick with the same sha256, which the hub
/// treats as the same copy.
pub async fn ship_pending(store: &Arc<Store>, state: &AppState) {
    let key = match backup_key() {
        None => return,
        Some(Ok(k)) => k,
        Some(Err(e)) => {
            warn!(error = %e, "backups cannot leave this computer");
            alarm(store, state, "backup-key-invalid", "Backups cannot leave this computer", &e);
            return;
        }
    };
    let rows = match store.list_backups() {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "list backups for shipping");
            return;
        }
    };
    // Newest first, as the ring lists them.
    let Some(pending) = rows
        .into_iter()
        .find(|b| b.shipped_at.is_none() && b.integrity == "ok" && Path::new(&b.path).exists())
    else {
        return;
    };
    let bot_id = std::env::var("NEBO_BOT_ID").unwrap_or_default();
    let path = std::path::PathBuf::from(&pending.path);
    let taken_at = pending.taken_at;
    let packed = tokio::task::spawn_blocking(move || pack(&path, &bot_id, taken_at, &key)).await;
    let packed = match packed {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            warn!(error = %e, path = %pending.path, "pack backup for shipping");
            return;
        }
        Err(e) => {
            warn!(error = %e, "pack backup task");
            return;
        }
    };
    let fields = [
        ("purpose".to_string(), "backup".to_string()),
        ("taken_at".to_string(), taken_at.to_string()),
        ("sha256".to_string(), packed.sha256.clone()),
        ("key_version".to_string(), KEY_VERSION.to_string()),
    ];
    let bytes = packed.tar.len();
    match state
        .comm_manager
        .upload_file(&packed.name, "application/x-tar", packed.tar, &fields)
        .await
    {
        Ok(att) => {
            if let Err(e) = store.mark_shipped(&pending.id, &att.file_id) {
                warn!(error = %e, "record shipped backup");
            }
            info!(id = %pending.id, file_id = %att.file_id, bytes, "database copy shipped to the hub");
        }
        Err(e) => {
            warn!(error = %e, id = %pending.id, "ship backup");
            if now_secs() - taken_at > UNSHIPPED_ALARM_SECS {
                alarm(
                    store,
                    state,
                    "backup-unshipped",
                    "The latest backup has not left this computer",
                    &format!("Uploading it to NeboAI keeps failing: {e}. It is safe locally; it is not yet safe elsewhere."),
                );
            }
        }
    }
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

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// What a restore does, in the order a restore does it: untar, decrypt, gunzip.
    fn unpack(tar_bytes: &[u8], key: &[u8; 32]) -> (serde_json::Value, Vec<u8>) {
        let mut manifest = None;
        let mut sealed = None;
        for entry in tar::Archive::new(tar_bytes).entries().unwrap() {
            let mut entry = entry.unwrap();
            let name = entry.path().unwrap().to_string_lossy().to_string();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            if name == "manifest.json" {
                manifest = Some(serde_json::from_slice(&bytes).unwrap());
            } else {
                sealed = Some(bytes);
            }
        }
        let gz = mcp::crypto::Encryptor::new(*key).decrypt(&sealed.unwrap()).unwrap();
        let mut plain = Vec::new();
        flate2::read::GzDecoder::new(gz.as_slice()).read_to_end(&mut plain).unwrap();
        (manifest.unwrap(), plain)
    }

    #[test]
    fn a_packed_copy_restores_byte_for_byte_and_names_itself() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("nebo-20260918T010203Z.db");
        // A real database, so the restore script's integrity_check has something to say.
        {
            let conn = rusqlite::Connection::open(&src).unwrap();
            conn.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY, body TEXT);").unwrap();
            for i in 0..2000 {
                conn.execute("INSERT INTO t(body) VALUES (?1)", [format!("row {i} {}", "x".repeat(100))]).unwrap();
            }
        }
        let original = std::fs::read(&src).unwrap();
        let key = [7u8; 32];

        let packed = pack(&src, "bot-1", 1_789_700_000, &key).unwrap();
        // NEBO_PACK_DUMP=<dir>: leave the copy and its key behind so the
        // operator restore script (neboloop scripts/restore-bot-backup.py)
        // can be proven against what this code really writes.
        if let Ok(dump) = std::env::var("NEBO_PACK_DUMP") {
            std::fs::create_dir_all(&dump).unwrap();
            std::fs::write(format!("{dump}/copy.tar"), &packed.tar).unwrap();
            std::fs::write(format!("{dump}/key.hex"), hex::encode(key)).unwrap();
            std::fs::write(format!("{dump}/original.db"), &original).unwrap();
        }
        assert_eq!(packed.name, "nebo-20260918T025320Z.tar");
        assert_eq!(packed.sha256, hex::encode(Sha256::digest(&original)));
        assert!(packed.tar.len() < original.len(), "a database copy compresses");

        let (manifest, plain) = unpack(&packed.tar, &key);
        assert_eq!(plain, original);
        assert_eq!(manifest["sha256"], packed.sha256);
        assert_eq!(manifest["bot_id"], "bot-1");
        assert_eq!(manifest["key_version"], KEY_VERSION);

        // The wrong key opens nothing.
        let wrong = [8u8; 32];
        let r = std::panic::catch_unwind(|| unpack(&packed.tar, &wrong));
        assert!(r.is_err());
    }
}
