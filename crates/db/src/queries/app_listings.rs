use rusqlite::params;
use serde::{Deserialize, Serialize};
use types::NeboError;

use crate::{OptionalExt, Store};

/// An app's marketplace listing (`app_listings`): the draft the owner
/// shaped in conversation, the hub artifact it was published as, and where
/// its review stands. `draft` is the listing as JSON (the tools crate owns
/// its shape); `status` is draft, submitted, in_review, approved or
/// rejected.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppListing {
    pub app_id: String,
    pub artifact_id: String,
    pub draft: String,
    pub status: String,
    pub version: String,
    pub notes: String,
    pub chat_session: String,
    pub updated_at: i64,
}

fn row_to_listing(row: &rusqlite::Row) -> rusqlite::Result<AppListing> {
    Ok(AppListing {
        app_id: row.get("app_id")?,
        artifact_id: row.get("artifact_id")?,
        draft: row.get("draft")?,
        status: row.get("status")?,
        version: row.get("version")?,
        notes: row.get("notes")?,
        chat_session: row.get("chat_session")?,
        updated_at: row.get("updated_at")?,
    })
}

impl Store {
    pub fn get_app_listing(&self, app_id: &str) -> Result<Option<AppListing>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT * FROM app_listings WHERE app_id = ?1",
            params![app_id],
            row_to_listing,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// The listing published as this hub artifact.
    pub fn app_listing_by_artifact(
        &self,
        artifact_id: &str,
    ) -> Result<Option<AppListing>, NeboError> {
        if artifact_id.is_empty() {
            return Ok(None);
        }
        let conn = self.conn()?;
        conn.query_row(
            "SELECT * FROM app_listings WHERE artifact_id = ?1",
            params![artifact_id],
            row_to_listing,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Write the whole listing row, replacing the one the app had.
    pub fn put_app_listing(&self, listing: &AppListing) -> Result<AppListing, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "INSERT INTO app_listings (app_id, artifact_id, draft, status, version, notes, chat_session)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(app_id) DO UPDATE SET
                artifact_id = excluded.artifact_id, draft = excluded.draft,
                status = excluded.status, version = excluded.version,
                notes = excluded.notes, chat_session = excluded.chat_session,
                updated_at = unixepoch()
             RETURNING *",
            params![
                listing.app_id,
                listing.artifact_id,
                listing.draft,
                listing.status,
                listing.version,
                listing.notes,
                listing.chat_session
            ],
            row_to_listing,
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> Store {
        let path = std::env::temp_dir().join(format!(
            "nebo-app-listings-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        Store::new(path.to_str().unwrap()).unwrap()
    }

    /// A listing round-trips, is found by its artifact once published, and
    /// a second write replaces the first.
    #[test]
    fn a_listing_is_kept_per_app_and_found_by_its_artifact() {
        let store = temp_store();
        assert!(store.get_app_listing("app-1").unwrap().is_none());
        assert!(store.app_listing_by_artifact("").unwrap().is_none());
        let draft = AppListing {
            app_id: "app-1".into(),
            draft: r#"{"name":"Kart Racer"}"#.into(),
            status: "draft".into(),
            ..Default::default()
        };
        store.put_app_listing(&draft).unwrap();
        assert!(store.app_listing_by_artifact("art-1").unwrap().is_none());
        let submitted = AppListing {
            artifact_id: "art-1".into(),
            status: "in_review".into(),
            version: "1.0.0".into(),
            chat_session: "agent:app-1:web".into(),
            ..draft
        };
        store.put_app_listing(&submitted).unwrap();
        let found = store.app_listing_by_artifact("art-1").unwrap().unwrap();
        assert_eq!(
            (
                found.app_id.as_str(),
                found.status.as_str(),
                found.version.as_str()
            ),
            ("app-1", "in_review", "1.0.0")
        );
        assert_eq!(found.draft, r#"{"name":"Kart Racer"}"#);
    }
}
