use crate::migrate;
use crate::pool::DbPool;
use types::NeboError;

/// Database store wrapping a connection pool.
/// Provides typed query methods matching the Go sqlc-generated Store.
pub struct Store {
    pool: DbPool,
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

        Ok(Self { pool })
    }

    /// Get a connection from the pool.
    pub(crate) fn conn(
        &self,
    ) -> Result<r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>, NeboError> {
        self.pool
            .get()
            .map_err(|e| NeboError::Database(format!("failed to get connection: {e}")))
    }
}
