use rusqlite::params;
use types::NeboError;

use crate::Store;

impl Store {
    /// The owner's answers for an app's permissions: `(permission, allowed)`.
    /// A permission with no row has not been answered.
    pub fn app_permission_answers(&self, app_id: &str) -> Result<Vec<(String, bool)>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT permission, allowed FROM app_permission_grants WHERE app_id = ?1 ORDER BY permission")
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![app_id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? != 0)))
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Record the owner's answer for one of an app's permissions: allowed,
    /// or not (asked and refused, or taken back).
    pub fn answer_app_permission(&self, app_id: &str, permission: &str, allowed: bool) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO app_permission_grants (app_id, permission, allowed, decided_at) VALUES (?1, ?2, ?3, unixepoch()) \
             ON CONFLICT(app_id, permission) DO UPDATE SET allowed = excluded.allowed, decided_at = excluded.decided_at",
            params![app_id, permission, allowed as i64],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::Store;

    #[test]
    fn an_answer_is_kept_and_can_be_changed() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("t.db").to_str().unwrap()).unwrap();
        assert!(store.app_permission_answers("app-1").unwrap().is_empty());
        store.answer_app_permission("app-1", "filesystem:read:Media", true).unwrap();
        store.answer_app_permission("app-1", "network:api.example.com", false).unwrap();
        store.answer_app_permission("app-2", "network:x.com", true).unwrap();
        assert_eq!(
            store.app_permission_answers("app-1").unwrap(),
            vec![("filesystem:read:Media".to_string(), true), ("network:api.example.com".to_string(), false)]
        );
        store.answer_app_permission("app-1", "filesystem:read:Media", false).unwrap();
        assert_eq!(store.app_permission_answers("app-1").unwrap()[0], ("filesystem:read:Media".to_string(), false));
    }
}
