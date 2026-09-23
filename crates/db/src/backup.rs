//! The snapshot ring — every copy this Nebo takes of its own database.
//!
//! One primitive, `vacuum_into`, is the ONE way a copy is made: SQLite's own
//! `VACUUM INTO`, which is consistent under WAL without stopping anything.
//! The migrator uses it before every migration; the scheduler uses it every
//! night; the owner uses it from Settings. A copy is never made by copying
//! the live file, because a live WAL file copied by hand is exactly how a
//! backup turns out to be nothing.
//!
//! A copy is not a backup until it has been opened and passed
//! `PRAGMA integrity_check`. A copy that fails is deleted and the failure
//! is the caller's to announce — a broken copy of a broken database is the
//! moment the owner needs to know, not the day they try to restore.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::{DbErrExt, Store};
use types::NeboError;

/// One row of the ring, frontend-shaped (genapi emits this).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Backup {
    pub id: String,
    pub path: String,
    pub taken_at: i64,
    pub reason: String,
    pub bytes: i64,
    pub integrity: String,
    pub shipped_at: Option<i64>,
    pub file_id: Option<String>,
}

/// Where the ring lives: `<database dir>/backups/`.
pub fn ring_dir(db_path: &str) -> PathBuf {
    Path::new(db_path)
        .parent()
        .map(|p| p.join("backups"))
        .unwrap_or_else(|| PathBuf::from("backups"))
}

/// Make a consistent copy of the open database at `dest`, then prove it.
/// Returns what `integrity_check` said, which is `ok` or an error.
pub fn vacuum_into(conn: &Connection, dest: &Path) -> Result<String, NeboError> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| NeboError::Database(format!("create {}: {e}", parent.display())))?;
    }
    let _ = std::fs::remove_file(dest);
    let dest_s = dest.to_string_lossy().to_string();
    conn.execute("VACUUM INTO ?1", params![dest_s])
        .map_err(|e| NeboError::Database(format!("vacuum into {dest_s}: {e}")))?;
    match integrity_of(dest) {
        Ok(v) if v == "ok" => Ok(v),
        Ok(v) => {
            let _ = std::fs::remove_file(dest);
            Err(NeboError::Database(format!("copy failed integrity check: {v}")))
        }
        Err(e) => {
            let _ = std::fs::remove_file(dest);
            Err(e)
        }
    }
}

/// Open a file read-only and ask SQLite whether it is whole.
pub fn integrity_of(path: &Path) -> Result<String, NeboError> {
    let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| NeboError::Database(format!("open {}: {e}", path.display())))?;
    conn.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
        .map_err(|e| NeboError::Database(format!("integrity_check {}: {e}", path.display())))
}

/// The retention rule: the last 7 daily, the last 4 weekly, the last 3
/// monthly, plus every copy taken for a migration, which is a version
/// boundary and not a day. Each tier keeps the newest copy in each of its
/// newest N buckets, so "4 weekly" is four weeks, not "whatever fell in a
/// four-week window". Returns the rows that no longer earn their place.
pub fn retain(rows: &[Backup], _now: i64) -> Vec<Backup> {
    let day = 86_400i64;
    let mut sorted: Vec<&Backup> = rows.iter().collect();
    sorted.sort_by_key(|b| std::cmp::Reverse(b.taken_at));
    let mut keep = std::collections::HashSet::new();
    for b in &sorted {
        if b.reason == "pre-migration" {
            keep.insert(b.id.clone());
        }
    }
    // Newest copy per bucket, per tier; a copy claimed by a finer tier is not
    // offered to a coarser one, so the tiers cover successively older ground.
    let mut claimed = std::collections::HashSet::new();
    for (width, cap) in [(day, 7usize), (7 * day, 4usize), (30 * day, 3usize)] {
        let mut buckets: Vec<i64> = Vec::new();
        for b in &sorted {
            if claimed.contains(&b.id) {
                continue;
            }
            let bucket = b.taken_at / width;
            if buckets.contains(&bucket) {
                claimed.insert(b.id.clone());
                continue; // an older copy in a bucket already represented
            }
            if buckets.len() == cap {
                continue; // this tier is full; leave it for the next
            }
            buckets.push(bucket);
            keep.insert(b.id.clone());
            claimed.insert(b.id.clone());
        }
    }
    rows.iter().filter(|b| !keep.contains(&b.id)).cloned().collect()
}

impl Store {
    /// Take a verified snapshot now. `reason` is nightly, pre-migration or
    /// manual, and is what the ring shows beside the time.
    pub fn snapshot(&self, reason: &str) -> Result<Backup, NeboError> {
        let conn = self.conn()?;
        let now = chrono_now();
        let stamp = chrono::DateTime::from_timestamp(now, 0)
            .map(|t| t.format("%Y%m%dT%H%M%SZ").to_string())
            .unwrap_or_else(|| now.to_string());
        let dest = ring_dir(&self.path).join(format!("nebo-{stamp}.db"));
        let integrity = vacuum_into(&conn, &dest)?;
        let bytes = std::fs::metadata(&dest).map(|m| m.len() as i64).unwrap_or(0);
        let id = uuid::Uuid::new_v4().to_string();
        let path = dest.to_string_lossy().to_string();
        conn.execute(
            "INSERT INTO backups (id, path, taken_at, reason, bytes, integrity) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, path, now, reason, bytes, integrity],
        )
        .db_err("record backup")?;
        info!(path = %path, reason, bytes, "database snapshot taken and verified");
        self.get_backup(&id)?.ok_or_else(|| NeboError::Database("backup row vanished".into()))
    }

    /// A verified copy at `dest` that is not a ring entry: what a BotState
    /// commit packs in the live database's place, then deletes.
    pub fn copy_verified(&self, dest: &Path) -> Result<(), NeboError> {
        let conn = self.conn()?;
        vacuum_into(&conn, dest).map(|_| ())
    }

    pub fn get_backup(&self, id: &str) -> Result<Option<Backup>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, path, taken_at, reason, bytes, integrity, shipped_at, file_id FROM backups WHERE id = ?1",
            params![id],
            row_to_backup,
        )
        .optional()
        .db_err("get_backup")
    }

    /// Newest first.
    pub fn list_backups(&self) -> Result<Vec<Backup>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT id, path, taken_at, reason, bytes, integrity, shipped_at, file_id FROM backups ORDER BY taken_at DESC")
            .db_err("list_backups prepare")?;
        let rows = stmt.query_map([], row_to_backup).db_err("list_backups")?;
        rows.collect::<Result<Vec<_>, _>>().db_err("list_backups collect")
    }

    /// Apply the retention rule: delete the files and rows that no longer
    /// earn their place. A file already gone is not an error; the row goes.
    pub fn retain_backups(&self) -> Result<usize, NeboError> {
        let rows = self.list_backups()?;
        let drop = retain(&rows, chrono_now());
        let conn = self.conn()?;
        for b in &drop {
            let _ = std::fs::remove_file(&b.path);
            conn.execute("DELETE FROM backups WHERE id = ?1", params![b.id]).db_err("retain delete")?;
        }
        Ok(drop.len())
    }

    /// The migrator writes `<db>.pre-vNNNN.bak` before it changes anything.
    /// Those are backups too; this puts them in the ring so there is one list.
    pub fn adopt_pre_migration_copies(&self) -> Result<usize, NeboError> {
        let db = Path::new(&self.path);
        let Some(dir) = db.parent() else { return Ok(0) };
        let stem = db.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let known: std::collections::HashSet<String> = self.list_backups()?.into_iter().map(|b| b.path).collect();
        let mut adopted = 0;
        for entry in std::fs::read_dir(dir).map_err(|e| NeboError::Database(format!("read {}: {e}", dir.display())))? {
            let Ok(entry) = entry else { continue };
            let name = entry.file_name().to_string_lossy().to_string();
            if !(name.starts_with(&format!("{stem}.pre-v")) && name.ends_with(".bak")) {
                continue;
            }
            let path = entry.path();
            let path_s = path.to_string_lossy().to_string();
            if known.contains(&path_s) {
                continue;
            }
            // Only a copy that opens whole is adopted; anything else stays a file.
            let Ok(integrity) = integrity_of(&path) else { continue };
            if integrity != "ok" {
                continue;
            }
            let meta = std::fs::metadata(&path).ok();
            let bytes = meta.as_ref().map(|m| m.len() as i64).unwrap_or(0);
            let taken_at = meta
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or_else(chrono_now);
            let conn = self.conn()?;
            conn.execute(
                "INSERT OR IGNORE INTO backups (id, path, taken_at, reason, bytes, integrity) VALUES (?1, ?2, ?3, 'pre-migration', ?4, ?5)",
                params![uuid::Uuid::new_v4().to_string(), path_s, taken_at, bytes, integrity],
            )
            .db_err("adopt backup")?;
            adopted += 1;
        }
        Ok(adopted)
    }

    /// Ask the live database whether it is whole. Quick, so it can run on
    /// every start; a Nebo must never quietly serve a broken database.
    pub fn quick_check(&self) -> Result<(), String> {
        let conn = match self.conn() {
            Ok(c) => c,
            Err(e) => return Err(e.to_string()),
        };
        match conn.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0)) {
            Ok(v) if v == "ok" => Ok(()),
            Ok(v) => Err(v),
            Err(e) => Err(e.to_string()),
        }
    }
}

fn row_to_backup(row: &rusqlite::Row) -> rusqlite::Result<Backup> {
    Ok(Backup {
        id: row.get(0)?,
        path: row.get(1)?,
        taken_at: row.get(2)?,
        reason: row.get(3)?,
        bytes: row.get(4)?,
        integrity: row.get(5)?,
        shipped_at: row.get(6)?,
        file_id: row.get(7)?,
    })
}

fn chrono_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

use crate::OptionalExt;

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (Store, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nebo.db");
        let s = Store::new(&path.to_string_lossy()).unwrap();
        (s, dir)
    }

    /// A snapshot is a file that opens whole, and a row that says so.
    #[test]
    fn a_snapshot_is_verified_before_it_counts() {
        let (s, _dir) = store();
        let b = s.snapshot("manual").unwrap();
        assert_eq!(b.integrity, "ok");
        assert!(b.bytes > 0);
        assert!(Path::new(&b.path).exists());
        assert_eq!(s.list_backups().unwrap().len(), 1);
        assert_eq!(integrity_of(Path::new(&b.path)).unwrap(), "ok");
    }

    /// Seven daily, four weekly, three monthly; migration copies always stay.
    #[test]
    fn retention_keeps_the_rule_and_every_migration_copy() {
        let day = 86_400;
        let now = 1_800_000_000;
        let mut rows = Vec::new();
        for d in 0..40 {
            rows.push(Backup { id: format!("d{d}"), path: format!("/x/{d}"), taken_at: now - d * day, reason: "nightly".into(), bytes: 1, integrity: "ok".into(), shipped_at: None, file_id: None });
        }
        rows.push(Backup { id: "m".into(), path: "/x/m".into(), taken_at: now - 400 * day, reason: "pre-migration".into(), bytes: 1, integrity: "ok".into(), shipped_at: None, file_id: None });
        let dropped = retain(&rows, now);
        let kept: Vec<&str> = rows.iter().filter(|r| !dropped.iter().any(|d| d.id == r.id)).map(|r| r.id.as_str()).collect();
        assert!(kept.contains(&"m"), "a migration copy is never dropped");
        // 7 daily + 4 weekly + the monthly buckets 40 days can hold (1 or 2),
        // plus the migration copy.
        assert!(kept.len() >= 13 && kept.len() <= 14, "kept {kept:?}");
        assert!(kept.contains(&"d0") && kept.contains(&"d6"), "the newest week stays day by day");
        assert!(!kept.contains(&"d8") || !kept.contains(&"d9"), "older days collapse to one per week");
    }

    /// A migrator's copy sitting beside the database joins the ring.
    #[test]
    fn migration_copies_are_adopted() {
        let (s, dir) = store();
        let bak = dir.path().join("nebo.db.pre-v0158.bak");
        {
            let conn = s.conn().unwrap();
            vacuum_into(&conn, &bak).unwrap();
        }
        assert_eq!(s.adopt_pre_migration_copies().unwrap(), 1);
        assert_eq!(s.adopt_pre_migration_copies().unwrap(), 0, "adoption is idempotent");
        let rows = s.list_backups().unwrap();
        assert_eq!(rows[0].reason, "pre-migration");
    }

    #[test]
    fn quick_check_passes_on_a_healthy_store() {
        let (s, _dir) = store();
        assert!(s.quick_check().is_ok());
    }
}
