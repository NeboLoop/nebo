pub mod migrate;
pub mod models;
mod pool;
mod store;

// Query modules
mod queries;

pub use pool::create_pool;
pub use queries::LicenseKeyRow;
pub use store::Store;

/// Extension trait to convert `rusqlite::Error::QueryReturnedNoRows` into `Ok(None)`.
pub(crate) trait OptionalExt<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error>;
}

impl<T> OptionalExt<T> for Result<T, rusqlite::Error> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error> {
        match self {
            Ok(val) => Ok(Some(val)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// Extension trait that logs DB errors before converting to `NeboError::Database`.
/// Use `.db_err("context")` instead of `.map_err(|e| NeboError::Database(e.to_string()))`.
pub(crate) trait DbErrExt<T> {
    fn db_err(self, context: &str) -> Result<T, types::NeboError>;
}

impl<T, E: std::fmt::Display> DbErrExt<T> for Result<T, E> {
    fn db_err(self, context: &str) -> Result<T, types::NeboError> {
        self.map_err(|e| {
            tracing::warn!(context = context, error = %e, "database error");
            types::NeboError::Database(format!("{}: {}", context, e))
        })
    }
}
