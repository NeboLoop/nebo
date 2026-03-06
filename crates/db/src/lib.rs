pub mod migrate;
pub mod models;
mod pool;
mod store;

// Query modules
mod queries;

pub use pool::create_pool;
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
