use rusqlite::params;

use crate::Store;
use types::NeboError;

impl Store {
    /// The constitution as stored: the CompanyPolicy JSON, or None when the
    /// owner has not written one.
    pub fn get_company_policy(&self) -> Result<Option<String>, NeboError> {
        let conn = self.conn()?;
        match conn.query_row("SELECT policy FROM company_policy WHERE id = 1", [], |row| {
            row.get::<_, String>(0)
        }) {
            Ok(s) => Ok(Some(s)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(NeboError::Database(e.to_string())),
        }
    }

    /// Write the constitution wholesale. The only writer is the owner.
    pub fn set_company_policy(&self, policy_json: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO company_policy (id, policy, updated_at) VALUES (1, ?1, unixepoch())
             ON CONFLICT(id) DO UPDATE SET policy = excluded.policy, updated_at = excluded.updated_at",
            params![policy_json],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nebo-company-policy-test.db");
        let store = Store::new(&path.to_string_lossy()).expect("store");
        (dir, store)
    }

    #[test]
    fn the_constitution_is_one_row_written_wholesale() {
        let (_dir, store) = store();
        assert_eq!(store.get_company_policy().unwrap(), None);
        store.set_company_policy(r#"{"purpose":"a"}"#).unwrap();
        store.set_company_policy(r#"{"purpose":"b"}"#).unwrap();
        assert_eq!(store.get_company_policy().unwrap().as_deref(), Some(r#"{"purpose":"b"}"#));
    }
}
