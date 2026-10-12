use crate::migrate;
use crate::pool::DbPool;
use types::NeboError;

/// Database store wrapping a connection pool.
/// Provides typed query methods matching the Go sqlc-generated Store.
pub struct Store {
    pool: DbPool,
    /// Where the database lives; the snapshot ring sits beside it.
    pub(crate) path: String,
}

impl Store {
    /// Create a new Store, running migrations on first connection.
    pub fn new(db_path: &str) -> Result<Self, NeboError> {
        let pool = crate::create_pool(db_path)?;

        // Run migrations on a dedicated connection
        {
            let conn = pool
                .get()
                .map_err(|e| NeboError::Database(format!("failed to get connection: {e}")))?;
            migrate::run_migrations(&conn)?;
        }

        let store = Self { pool, path: db_path.to_string() };
        // The migrator's copies are backups too; one list, not a folder.
        if let Err(e) = store.adopt_pre_migration_copies() {
            tracing::warn!(error = %e, "could not adopt pre-migration copies into the backup ring");
        }
        Ok(store)
    }

    /// Get a connection from the pool, waiting at most `pool::POOL_WAIT`.
    /// A wait that runs out is logged with the query that asked and the
    /// pool's state, so a stall names its callers instead of hiding.
    #[track_caller]
    pub(crate) fn conn(
        &self,
    ) -> Result<r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>, NeboError> {
        let caller = std::panic::Location::caller();
        self.pool.get().map_err(|e| {
            let (connections, idle) = self.pool_state();
            tracing::error!(
                caller = %caller,
                connections,
                idle,
                wait_secs = crate::pool::POOL_WAIT.as_secs(),
                error = %e,
                "database: no connection free; the pool is held"
            );
            NeboError::Database(format!("failed to get connection: {e}"))
        })
    }

    /// The pool's open and idle connection counts: all open and none idle
    /// means every connection is held.
    pub fn pool_state(&self) -> (u32, u32) {
        let state = self.pool.state();
        (state.connections, state.idle_connections)
    }

    /// One read through the pool, as any query makes it: a connection, then
    /// a page of the schema under a shared lock. Ok means the database
    /// answers now (`server::liveness` probes with it).
    pub fn ping(&self) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| row.get::<_, i64>(0))
            .map_err(|e| NeboError::Database(format!("ping: {e}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// A pool with every connection held answers the next caller with an
    /// error after `POOL_WAIT`, never by blocking it forever; the database
    /// answers again once a connection is free.
    #[test]
    fn exhausted_pool_errors_after_bounded_wait() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::new(dir.path().join("nebo.db").to_str().unwrap()).expect("store");
        store.ping().expect("ping on an idle pool");

        let max = store.pool.max_size();
        let held: Vec<_> = (0..max).map(|_| store.conn().expect("conn")).collect();
        assert_eq!(store.pool_state(), (max, 0));

        let started = Instant::now();
        let err = store.ping().expect_err("no connection is free");
        let waited = started.elapsed();
        assert!(err.to_string().contains("failed to get connection"), "{err}");
        assert!(waited >= crate::pool::POOL_WAIT - Duration::from_millis(100), "{waited:?}");
        assert!(waited < crate::pool::POOL_WAIT + Duration::from_secs(5), "{waited:?}");

        drop(held);
        store.ping().expect("ping once connections are back");
    }
}
