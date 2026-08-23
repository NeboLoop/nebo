//! Workrooms — the owner's registry of mission rooms.
//!
//! A room IS a loop channel; the hub owns the conversation and its history.
//! This registry records which channels are rooms, the mission, and which of
//! this bot's employees participate (the sidebar list and the dispatch
//! filter both read it).

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::DbErrExt;
use crate::Store;
use types::NeboError;

/// One workroom row, frontend-shaped (genapi emits this).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workroom {
    pub channel_id: String,
    pub name: String,
    pub mission: String,
    /// Local agent ids participating in this room.
    pub member_agent_ids: Vec<String>,
    pub created_at: i64,
}

fn row_to_workroom(row: &rusqlite::Row) -> rusqlite::Result<Workroom> {
    let members_json: String = row.get(3)?;
    Ok(Workroom {
        channel_id: row.get(0)?,
        name: row.get(1)?,
        mission: row.get(2)?,
        member_agent_ids: serde_json::from_str(&members_json).unwrap_or_default(),
        created_at: row.get(4)?,
    })
}

impl Store {
    pub fn create_workroom(
        &self,
        channel_id: &str,
        name: &str,
        mission: &str,
        member_agent_ids: &[String],
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        let members = serde_json::to_string(member_agent_ids)
            .map_err(|e| NeboError::Internal(format!("serialize workroom members: {e}")))?;
        conn.execute(
            "INSERT INTO workrooms (channel_id, name, mission, member_agent_ids)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(channel_id) DO UPDATE SET
                 name = excluded.name,
                 mission = excluded.mission,
                 member_agent_ids = excluded.member_agent_ids",
            params![channel_id, name, mission, members],
        )
        .db_err("create_workroom")?;
        Ok(())
    }

    pub fn list_workrooms(&self) -> Result<Vec<Workroom>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT channel_id, name, mission, member_agent_ids, created_at
                 FROM workrooms ORDER BY created_at DESC",
            )
            .db_err("list_workrooms prepare")?;
        let rows = stmt
            .query_map([], row_to_workroom)
            .db_err("list_workrooms query")?;
        rows.collect::<Result<Vec<_>, _>>()
            .db_err("list_workrooms collect")
    }

    /// The room for a channel, if the channel is a registered room. The comm
    /// dispatch path uses this to tag room traffic for the frontend.
    pub fn get_workroom(&self, channel_id: &str) -> Result<Option<Workroom>, NeboError> {
        use crate::OptionalExt;
        let conn = self.conn()?;
        conn.query_row(
            "SELECT channel_id, name, mission, member_agent_ids, created_at
             FROM workrooms WHERE channel_id = ?1",
            params![channel_id],
            row_to_workroom,
        )
        .optional()
        .db_err("get_workroom")
    }

    pub fn delete_workroom(&self, channel_id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM workrooms WHERE channel_id = ?1",
            params![channel_id],
        )
        .db_err("delete_workroom")?;
        Ok(())
    }
}
