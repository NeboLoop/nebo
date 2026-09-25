//! Drafted employees and drafted job edits, the owner messages that consent
//! to them, and the ceiling an employee made by an employee works under
//! until the owner answers its card. The consent logic lives in
//! `agent::harness::permissions::consent`; this module only reads and writes
//! rows.

use rusqlite::{params, OptionalExtension};
use types::NeboError;

use crate::Store;

fn db_err(e: impl std::fmt::Display) -> NeboError {
    NeboError::Database(e.to_string())
}

/// A drafted employee (or edit) as `employee_drafts` keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmployeeDraftRow {
    pub id: String,
    /// `create` or `edit`.
    pub kind: String,
    /// The employee an edit changes; empty for a create.
    pub agent_id: String,
    /// The employee whose run drafted it; empty for the owner's own pages.
    pub creator_id: String,
    /// The chat the line was shown in; empty for the builder.
    pub chat_id: String,
    pub name: String,
    /// The call's input as drafted (JSON).
    pub input: String,
    /// The worked-out needs (`tools::needs::Needs` JSON).
    pub needs: String,
    /// The consent line the owner saw.
    pub line: String,
    pub shown_at: i64,
    pub status: String,
    pub created_at: i64,
}

/// An employee made by an employee, waiting on the owner's card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmployeeCeilingRow {
    pub agent_id: String,
    pub creator_id: String,
    /// The capabilities beyond the creator's grant (JSON array).
    pub extras: String,
    pub ask_id: String,
    pub created_at: i64,
}

const DRAFT_COLUMNS: &str =
    "id, kind, agent_id, creator_id, chat_id, name, input, needs, line, shown_at, status, created_at";

impl Store {
    pub fn insert_employee_draft(&self, row: &EmployeeDraftRow) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            &format!("INSERT INTO employee_drafts ({DRAFT_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"),
            params![
                row.id,
                row.kind,
                row.agent_id,
                row.creator_id,
                row.chat_id,
                row.name,
                row.input,
                row.needs,
                row.line,
                row.shown_at,
                row.status,
                row.created_at,
            ],
        )
        .map_err(db_err)?;
        Ok(())
    }

    pub fn get_employee_draft(&self, id: &str) -> Result<Option<EmployeeDraftRow>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(&format!("SELECT {DRAFT_COLUMNS} FROM employee_drafts WHERE id = ?1"), params![id], |row| {
            Ok(EmployeeDraftRow {
                id: row.get(0)?,
                kind: row.get(1)?,
                agent_id: row.get(2)?,
                creator_id: row.get(3)?,
                chat_id: row.get(4)?,
                name: row.get(5)?,
                input: row.get(6)?,
                needs: row.get(7)?,
                line: row.get(8)?,
                shown_at: row.get(9)?,
                status: row.get(10)?,
                created_at: row.get(11)?,
            })
        })
        .optional()
        .map_err(db_err)
    }

    /// Mark a draft used. False when it was already used: a draft is acted
    /// on once.
    pub fn use_employee_draft(&self, id: &str) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        let n = conn
            .execute("UPDATE employee_drafts SET status = 'used' WHERE id = ?1 AND status = 'open'", params![id])
            .map_err(db_err)?;
        Ok(n == 1)
    }

    /// How many of the owner's own messages arrived in `chat_id` after
    /// `after` (unix seconds): rows carrying [`OWNER_MARK`]. A platform
    /// prompt, a notification, a reminder, a coworker's, a parent's, or a
    /// message from a chat channel (Slack, Discord, a loop) is not the
    /// owner's.
    pub fn owner_messages_after(&self, chat_id: &str, after: i64) -> Result<usize, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT metadata FROM chat_messages WHERE chat_id = ?1 AND role = 'user' AND created_at > ?2")
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![chat_id, after], |row| row.get::<_, Option<String>>(0))
            .map_err(db_err)?;
        let mut owner = 0;
        for metadata in rows {
            if is_owner_message(metadata.map_err(db_err)?.as_deref()) {
                owner += 1;
            }
        }
        Ok(owner)
    }

    pub fn set_employee_ceiling(&self, row: &EmployeeCeilingRow) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO employee_ceilings (agent_id, creator_id, extras, ask_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(agent_id) DO UPDATE SET creator_id = excluded.creator_id,
               extras = excluded.extras, ask_id = excluded.ask_id",
            params![row.agent_id, row.creator_id, row.extras, row.ask_id, row.created_at],
        )
        .map_err(db_err)?;
        Ok(())
    }

    pub fn employee_ceiling(&self, agent_id: &str) -> Result<Option<EmployeeCeilingRow>, NeboError> {
        if agent_id.is_empty() {
            return Ok(None);
        }
        let conn = self.conn()?;
        conn.query_row(
            "SELECT agent_id, creator_id, extras, ask_id, created_at FROM employee_ceilings WHERE agent_id = ?1",
            params![agent_id],
            |row| {
                Ok(EmployeeCeilingRow {
                    agent_id: row.get(0)?,
                    creator_id: row.get(1)?,
                    extras: row.get(2)?,
                    ask_id: row.get(3)?,
                    created_at: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(db_err)
    }

    /// The employee whose extras card an ask is, if it is one.
    pub fn employee_ceiling_by_ask(&self, ask_id: &str) -> Result<Option<String>, NeboError> {
        let conn = self.conn()?;
        conn.query_row("SELECT agent_id FROM employee_ceilings WHERE ask_id = ?1", params![ask_id], |row| row.get(0))
            .optional()
            .map_err(db_err)
    }

    pub fn clear_employee_ceiling(&self, agent_id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM employee_ceilings WHERE agent_id = ?1", params![agent_id]).map_err(db_err)?;
        Ok(())
    }
}

/// The metadata key on a user-role row the owner wrote in their own app
/// (desktop, web, phone, voice): `"owner": true`. The harness writes it when
/// it stores the owner's own input; nothing else does, so a row without it
/// (a coworker's, a parent run's, a chat channel's, the platform's) is never
/// the owner's word.
pub const OWNER_MARK: &str = "owner";

/// Whether a stored user-role message is one the owner wrote: it carries
/// [`OWNER_MARK`] and is not hidden (`isMeta`).
fn is_owner_message(metadata: Option<&str>) -> bool {
    let Some(meta) = metadata.and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok()) else {
        return false;
    };
    meta.get(OWNER_MARK).and_then(|v| v.as_bool()) == Some(true)
        && meta.get("isMeta").and_then(|v| v.as_bool()) != Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_owners_own_words_count() {
        assert!(is_owner_message(Some(r#"{"owner":true}"#)));
        assert!(is_owner_message(Some(
            r#"{"arrivedMidTurn":true,"via":"chat","owner":true}"#
        )));
        // No mark: a chat channel's message, a coworker's, anything unmarked.
        assert!(!is_owner_message(None));
        assert!(!is_owner_message(Some(r#"{"images":[]}"#)));
        assert!(!is_owner_message(Some(
            r#"{"arrivedMidTurn":true,"via":"slack"}"#
        )));
        assert!(!is_owner_message(Some(
            r#"{"isMeta":true,"hiddenPrompt":true,"owner":true}"#
        )));
        assert!(!is_owner_message(Some(
            r#"{"notification":true,"isMeta":true}"#
        )));
        assert!(!is_owner_message(Some(
            r#"{"arrivedMidTurn":true,"from":"coworker","coworker":"Ops"}"#
        )));
        assert!(!is_owner_message(Some(
            r#"{"arrivedMidTurn":true,"from":"parent"}"#
        )));
    }
}
