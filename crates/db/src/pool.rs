use std::path::Path;
use std::time::Duration;

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;

use types::NeboError;

pub type DbPool = Pool<SqliteConnectionManager>;

/// Create a connection pool for SQLite with WAL mode and recommended pragmas.
pub fn create_pool(db_path: &str) -> Result<DbPool, NeboError> {
    // Ensure parent directory exists
    if let Some(parent) = Path::new(db_path).parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            NeboError::Database(format!("failed to create database directory: {e}"))
        })?;
    }

    let manager = SqliteConnectionManager::file(db_path);
    let pool = Pool::builder()
        .max_size(10)
        .min_idle(Some(1))
        .connection_timeout(Duration::from_secs(30))
        .connection_customizer(Box::new(SqlitePragmas))
        .build(manager)
        .map_err(|e| NeboError::Database(format!("failed to create pool: {e}")))?;

    Ok(pool)
}

/// The journal mode every connection runs with.
///
/// WAL is right on a local disk and is the default. It is wrong on a
/// filesystem without coherent shared memory: WAL keeps its index in a
/// memory-mapped `-shm` file, and on a FUSE mount (a cloud bot's `/data`
/// arrives over virtiofs) that mapping is a page cache the kernel may drop
/// and re-read behind SQLite's back. Two cloud bots lost writes that way on
/// 2026-09-17 — zero-filled root pages of hot tables, no crash anywhere.
/// A rollback journal has no shared memory, so `NEBO_SQLITE_JOURNAL=delete`
/// is what the provisioner sets on cloud pods. Anything else is WAL.
pub fn journal_mode() -> &'static str {
    match std::env::var("NEBO_SQLITE_JOURNAL").as_deref() {
        Ok("delete") | Ok("DELETE") => "DELETE",
        _ => "WAL",
    }
}

/// Applies SQLite pragmas on each new connection.
#[derive(Debug)]
struct SqlitePragmas;

impl r2d2::CustomizeConnection<rusqlite::Connection, rusqlite::Error> for SqlitePragmas {
    fn on_acquire(&self, conn: &mut rusqlite::Connection) -> Result<(), rusqlite::Error> {
        conn.execute_batch(&format!(
            "PRAGMA journal_mode = {};
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;
             PRAGMA cache_size = -20000;
             PRAGMA temp_store = MEMORY;",
            journal_mode()
        ))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every pooled connection runs with WAL journaling and the recommended
    /// pragmas (foreign_keys ON is what makes chat_messages' ON DELETE CASCADE
    /// real; busy_timeout is what keeps concurrent writers from erroring).
    /// With `NEBO_SQLITE_JOURNAL=delete` — a cloud pod on virtiofs — the
    /// same pool runs a rollback journal instead, and no `-shm` file exists.
    #[test]
    fn pool_connections_get_wal_and_pragmas() {
        let dir = tempfile::tempdir().expect("tempdir");

        // The env var is process-wide, so both modes are proven in one test.
        // SAFETY: single test touches this var; no other thread reads it here.
        unsafe { std::env::set_var("NEBO_SQLITE_JOURNAL", "delete") };
        let cloud = dir.path().join("cloud").join("nebo-test.db");
        {
            let pool = create_pool(&cloud.to_string_lossy()).expect("pool");
            let conn = pool.get().expect("conn");
            let journal_mode: String = conn
                .query_row("PRAGMA journal_mode", [], |row| row.get(0))
                .unwrap();
            assert_eq!(journal_mode.to_lowercase(), "delete");
            conn.execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES(1);")
                .unwrap();
        }
        assert!(
            !cloud.with_extension("db-shm").exists(),
            "a rollback journal never creates a shared-memory file"
        );
        unsafe { std::env::remove_var("NEBO_SQLITE_JOURNAL") };

        // Nested path also proves create_pool creates missing parent dirs.
        let path = dir.path().join("nested").join("nebo-test.db");
        let pool = create_pool(&path.to_string_lossy()).expect("pool");
        let conn = pool.get().expect("conn");

        let journal_mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal_mode.to_lowercase(), "wal");

        let foreign_keys: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(foreign_keys, 1, "foreign keys must be enforced");

        let busy_timeout: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();
        assert_eq!(busy_timeout, 5000);
    }
}
