use rusqlite::params;

use crate::{DbErrExt, Store};
use crate::models::{InboxView, ItemOption, NewNotification, Notification};
use types::NeboError;

impl Store {
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
        .db_err("notifications")
    }

    /// One page of the Inbox, newest first: `view` picks the part
    /// ([`InboxView`]) and `agent_id` one employee's items. The default view
    /// (`All`) is every item not put away, as the list always was.
    pub fn list_user_notifications(
        &self,
        user_id: &str,
        view: InboxView,
        agent_id: Option<&str>,
        page_size: i64,
        page_offset: i64,
    ) -> Result<Vec<Notification>, NeboError> {
        let part = match view {
            InboxView::All => "archived_at IS NULL",
            InboxView::NeedsYou => "archived_at IS NULL AND needs_you = 1 AND answered_at IS NULL",
            InboxView::Updates => "archived_at IS NULL AND needs_you = 0",
            InboxView::History => "(archived_at IS NOT NULL OR answered_at IS NOT NULL)",
        };
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(&format!(
                "SELECT * FROM notifications WHERE user_id = ?1 AND {part}
                   AND (?2 IS NULL OR agent_id = ?2)
                 ORDER BY COALESCE(answered_at, archived_at, created_at) DESC, created_at DESC
                 LIMIT ?3 OFFSET ?4"
            ))
            .db_err("notifications")?;
        let rows = stmt
            .query_map(params![user_id, agent_id, page_size, page_offset], row_to_notification)
            .db_err("notifications")?;
        rows.collect::<Result<Vec<_>, _>>()
            .db_err("notifications")
    }

    /// How many items wait in each part of the Inbox: (decisions still
    /// open, updates not yet read). What the greeting and the badge say.
    pub fn count_inbox(&self, user_id: &str) -> Result<(i64, i64), NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT
               COALESCE(SUM(needs_you = 1 AND answered_at IS NULL), 0),
               COALESCE(SUM(needs_you = 0 AND read_at IS NULL), 0)
             FROM notifications WHERE user_id = ?1 AND archived_at IS NULL",
            params![user_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .db_err("notifications")
    }

    pub fn mark_notification_read(&self, id: &str, user_id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE notifications SET read_at = strftime('%s', 'now') WHERE id = ?1 AND user_id = ?2",
            params![id, user_id],
        )
        .db_err("notifications")?;
        Ok(())
    }

    /// Bring a notification back to the top of the Inbox, unread: a
    /// reminder about something still waiting on the owner. One the owner
    /// put away comes back too: it is waiting again.
    pub fn resurface_notification(&self, id: &str, user_id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE notifications SET read_at = NULL, archived_at = NULL, created_at = strftime('%s', 'now') WHERE id = ?1 AND user_id = ?2",
            params![id, user_id],
        )
        .db_err("notifications")?;
        Ok(())
    }

    /// Restate a notification's words and link in place, keeping when it
    /// was made and whether it was read. False when there is no such
    /// notification or the owner put it away, so it is never brought back.
    pub fn restate_notification(&self, id: &str, title: &str, body: Option<&str>, action_url: Option<&str>) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        let n = conn
            .execute(
                "UPDATE notifications SET title = ?2, body = ?3, action_url = ?4 WHERE id = ?1 AND archived_at IS NULL",
                params![id, title, body, action_url],
            )
            .db_err("notifications")?;
        Ok(n > 0)
    }

    pub fn mark_all_notifications_read(&self, user_id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE notifications SET read_at = strftime('%s', 'now') WHERE user_id = ?1 AND read_at IS NULL",
            params![user_id],
        )
        .db_err("notifications")?;
        Ok(())
    }

    /// Put an item away: it leaves the list for History. Nothing is
    /// deleted. A decision still waiting stays waiting where it is answered.
    pub fn archive_notification(&self, id: &str, user_id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE notifications SET archived_at = strftime('%s', 'now'), read_at = COALESCE(read_at, strftime('%s', 'now'))
             WHERE id = ?1 AND user_id = ?2 AND archived_at IS NULL",
            params![id, user_id],
        )
        .db_err("notifications")?;
        Ok(())
    }

    /// Settle a needs_you item: the first answer from any surface wins and
    /// the item moves to History with it, read. Returns whether this call
    /// settled it (false: no such item, or it was settled already).
    pub fn settle_notification(&self, id: &str, answer: &str, via: Option<&str>) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        let n = conn
            .execute(
                "UPDATE notifications
                    SET answered_at = strftime('%s', 'now'), answer = ?2, answered_via = ?3,
                        read_at = COALESCE(read_at, strftime('%s', 'now'))
                  WHERE id = ?1 AND answered_at IS NULL",
                params![id, answer, via],
            )
            .db_err("notifications")?;
        Ok(n > 0)
    }

    /// Settle every open item whose id starts with `prefix` but `keep`
    /// (an update notice for a version no longer on offer). Returns the ids
    /// settled.
    pub fn settle_notifications_like(&self, prefix: &str, keep: Option<&str>, answer: &str) -> Result<Vec<String>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "UPDATE notifications
                    SET answered_at = strftime('%s', 'now'), answer = ?3,
                        read_at = COALESCE(read_at, strftime('%s', 'now'))
                  WHERE substr(id, 1, length(?1)) = ?1 AND (?2 IS NULL OR id != ?2) AND answered_at IS NULL
                  RETURNING id",
            )
            .db_err("notifications")?;
        let rows = stmt
            .query_map(params![prefix, keep, answer], |r| r.get::<_, String>(0))
            .db_err("notifications")?;
        rows.collect::<Result<Vec<_>, _>>().db_err("notifications")
    }

    /// The items whose ids start with `prefix`, newest first, put away or
    /// not: what a learned rule handled ("See what it did").
    pub fn notifications_like(&self, prefix: &str, page_size: i64, page_offset: i64) -> Result<Vec<Notification>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT * FROM notifications WHERE substr(id, 1, length(?1)) = ?1
                 ORDER BY created_at DESC, rowid DESC LIMIT ?2 OFFSET ?3",
            )
            .db_err("notifications")?;
        let rows = stmt
            .query_map(params![prefix, page_size, page_offset], row_to_notification)
            .db_err("notifications")?;
        rows.collect::<Result<Vec<_>, _>>().db_err("notifications")
    }

    /// How many items have ids starting with `prefix`, and when the newest
    /// was made: how often a learned rule handled something.
    pub fn count_notifications_like(&self, prefix: &str) -> Result<(i64, Option<i64>), NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT COUNT(*), MAX(created_at) FROM notifications WHERE substr(id, 1, length(?1)) = ?1",
            params![prefix],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .db_err("notifications")
    }

    /// Insert an item unless one with the same `id` exists (ids are
    /// deterministic for retryable events). One the owner put away comes
    /// back fresh and unread when it is told again, as a new one would.
    pub fn create_notification_if_not_exists(&self, n: &NewNotification<'_>) -> Result<(), NeboError> {
        let options = (!n.options.is_empty())
            .then(|| serde_json::to_string(n.options))
            .transpose()
            .db_err("notifications")?;
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO notifications (id, user_id, type, title, body, action_url, agent_id,
                                        needs_you, options, link_label, reply_to, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, strftime('%s', 'now'))
             ON CONFLICT(id) DO UPDATE SET
               archived_at = NULL, read_at = NULL, created_at = excluded.created_at
             WHERE notifications.archived_at IS NOT NULL",
            params![
                n.id, n.user_id, n.notification_type, n.title, n.body, n.action_url, n.agent_id,
                n.needs_you as i64, options, n.link_label, n.reply_to
            ],
        )
        .db_err("notifications")?;
        Ok(())
    }
}

fn row_to_notification(row: &rusqlite::Row) -> rusqlite::Result<Notification> {
    let needs_you = row.get::<_, i64>("needs_you")? != 0;
    let answered_at: Option<i64> = row.get("answered_at")?;
    let archived_at: Option<i64> = row.get("archived_at")?;
    Ok(Notification {
        id: row.get("id")?,
        user_id: row.get("user_id")?,
        notification_type: row.get("type")?,
        title: row.get("title")?,
        body: row.get("body")?,
        action_url: row.get("action_url")?,
        icon: row.get("icon")?,
        agent_id: row.get("agent_id")?,
        read_at: row.get("read_at")?,
        created_at: row.get("created_at")?,
        needs_you,
        options: row
            .get::<_, Option<String>>("options")?
            .and_then(|o| serde_json::from_str::<Vec<ItemOption>>(&o).ok())
            .unwrap_or_default(),
        link_label: row.get("link_label")?,
        reply_to: row.get("reply_to")?,
        answered_at,
        answer: row.get("answer")?,
        answered_via: row.get("answered_via")?,
        archived_at,
        status: Notification::status_of(needs_you, answered_at, archived_at).to_string(),
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
