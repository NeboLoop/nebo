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

/// Applies SQLite pragmas on each new connection.
#[derive(Debug)]
struct SqlitePragmas;

impl r2d2::CustomizeConnection<rusqlite::Connection, rusqlite::Error> for SqlitePragmas {
    fn on_acquire(&self, conn: &mut rusqlite::Connection) -> Result<(), rusqlite::Error> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;
             PRAGMA cache_size = -20000;
             PRAGMA temp_store = MEMORY;",
        )?;
        Ok(())
    }
}
