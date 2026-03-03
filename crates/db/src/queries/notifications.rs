use rusqlite::params;

use crate::models::Notification;
use crate::Store;
use types::NeboError;

impl Store {
    pub fn create_notification(
        &self,
        id: &str,
        user_id: &str,
        notification_type: &str,
        title: &str,
        body: Option<&str>,
        action_url: Option<&str>,
        icon: Option<&str>,
    ) -> Result<Notification, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "INSERT INTO notifications (id, user_id, type, title, body, action_url, icon, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, strftime('%s', 'now')) RETURNING *",
            params![id, user_id, notification_type, title, body, action_url, icon],
            row_to_notification,
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_notification(
        &self,
        id: &str,
        user_id: &str,
    ) -> Result<Option<Notification>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT * FROM notifications WHERE id = ?1 AND user_id = ?2 LIMIT 1",
            params![id, user_id],
            row_to_notification,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn list_user_notifications(
        &self,
        user_id: &str,
        page_size: i64,
        page_offset: i64,
    ) -> Result<Vec<Notification>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT * FROM notifications WHERE user_id = ?1
                 ORDER BY created_at DESC LIMIT ?2 OFFSET ?3",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![user_id, page_size, page_offset], |row| {
                row_to_notification(row)
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn list_unread_notifications(
        &self,
        user_id: &str,
        page_size: i64,
    ) -> Result<Vec<Notification>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT * FROM notifications WHERE user_id = ?1 AND read_at IS NULL
                 ORDER BY created_at DESC LIMIT ?2",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![user_id, page_size], |row| {
                row_to_notification(row)
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn count_unread_notifications(&self, user_id: &str) -> Result<i64, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT COUNT(*) FROM notifications WHERE user_id = ?1 AND read_at IS NULL",
            params![user_id],
            |row| row.get(0),
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn mark_notification_read(&self, id: &str, user_id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE notifications SET read_at = strftime('%s', 'now') WHERE id = ?1 AND user_id = ?2",
            params![id, user_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn mark_all_notifications_read(&self, user_id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE notifications SET read_at = strftime('%s', 'now') WHERE user_id = ?1 AND read_at IS NULL",
            params![user_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn delete_notification(&self, id: &str, user_id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM notifications WHERE id = ?1 AND user_id = ?2",
            params![id, user_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn delete_old_notifications(&self, before: i64) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM notifications WHERE created_at < ?1",
            params![before],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }
}

fn row_to_notification(row: &rusqlite::Row) -> rusqlite::Result<Notification> {
    Ok(Notification {
        id: row.get("id")?,
        user_id: row.get("user_id")?,
        notification_type: row.get("type")?,
        title: row.get("title")?,
        body: row.get("body")?,
        action_url: row.get("action_url")?,
        icon: row.get("icon")?,
        read_at: row.get("read_at")?,
        created_at: row.get("created_at")?,
    })
}

trait OptionalExt<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error>;
}

impl<T> OptionalExt<T> for rusqlite::Result<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error> {
        match self {
            Ok(val) => Ok(Some(val)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}
