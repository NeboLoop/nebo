//! Teams — local objects on this Nebo: a name, a mission, the employees in
//! it, and the team's own local thread (session key `team:<id>`).
//!
//! A team needs no hub. `hub_channel_id` is NULL until (if ever) the team is
//! mirrored to a NeboAI hub loop channel; the comm dispatch path looks teams
//! up by that id to tag mirrored traffic.

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::models::ChatMessage;
use crate::DbErrExt;
use crate::OptionalExt;
use crate::Store;
use types::NeboError;

/// Session-key prefix of a team's local thread.
pub const TEAM_THREAD_PREFIX: &str = "team:";

/// The ONE builder for a team's thread key: `team:<id>`.
pub fn team_thread_key(team_id: &str) -> String {
    format!("{TEAM_THREAD_PREFIX}{team_id}")
}

/// One team row, frontend-shaped (genapi emits this).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Team {
    pub id: String,
    pub name: String,
    pub mission: String,
    /// Local agent ids in this team.
    pub member_agent_ids: Vec<String>,
    /// The employee that created the team; empty when the owner did.
    pub organizer_agent_id: String,
    /// The NeboAI hub channel mirroring this team, once mirrored.
    pub hub_channel_id: Option<String>,
    pub created_at: i64,
}

/// One message in a team's local thread, frontend-shaped (genapi emits this).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamMessage {
    pub id: String,
    /// Sender display name ("Owner" for the owner's own posts).
    pub from: String,
    /// Local agent id of the sender; empty for the owner.
    pub from_agent_id: String,
    /// `user` for the owner, `assistant` for an employee.
    pub role: String,
    pub content: String,
    /// Uploaded files on the post (upload metadata, camelCase), if any.
    pub attachments: Vec<serde_json::Value>,
    pub created_at: i64,
}

fn row_to_team(row: &rusqlite::Row) -> rusqlite::Result<Team> {
    let members_json: String = row.get(3)?;
    Ok(Team {
        id: row.get(0)?,
        name: row.get(1)?,
        mission: row.get(2)?,
        member_agent_ids: serde_json::from_str(&members_json).unwrap_or_default(),
        organizer_agent_id: row.get(4)?,
        hub_channel_id: row.get(5)?,
        created_at: row.get(6)?,
    })
}

const TEAM_COLUMNS: &str =
    "id, name, mission, member_agent_ids, organizer_agent_id, hub_channel_id, created_at";

fn message_from_row(m: ChatMessage) -> TeamMessage {
    let meta: serde_json::Value = m
        .metadata
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(serde_json::Value::Null);
    TeamMessage {
        id: m.id,
        from: meta["senderName"].as_str().unwrap_or("").to_string(),
        from_agent_id: meta["fromAgentId"].as_str().unwrap_or("").to_string(),
        role: m.role,
        content: m.content,
        attachments: meta["attachments"].as_array().cloned().unwrap_or_default(),
        created_at: m.created_at,
    }
}

impl Store {
    pub fn create_team(
        &self,
        id: &str,
        name: &str,
        mission: &str,
        member_agent_ids: &[String],
        organizer_agent_id: &str,
        hub_channel_id: Option<&str>,
    ) -> Result<Team, NeboError> {
        let conn = self.conn()?;
        let members = serde_json::to_string(member_agent_ids)
            .map_err(|e| NeboError::Internal(format!("serialize team members: {e}")))?;
        conn.query_row(
            &format!(
                "INSERT INTO teams (id, name, mission, member_agent_ids, organizer_agent_id, hub_channel_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 RETURNING {TEAM_COLUMNS}"
            ),
            params![id, name, mission, members, organizer_agent_id, hub_channel_id],
            row_to_team,
        )
        .db_err("create_team")
    }

    pub fn list_teams(&self) -> Result<Vec<Team>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {TEAM_COLUMNS} FROM teams ORDER BY created_at DESC, id"
            ))
            .db_err("list_teams prepare")?;
        let rows = stmt.query_map([], row_to_team).db_err("list_teams query")?;
        rows.collect::<Result<Vec<_>, _>>()
            .db_err("list_teams collect")
    }

    pub fn get_team(&self, id: &str) -> Result<Option<Team>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            &format!("SELECT {TEAM_COLUMNS} FROM teams WHERE id = ?1"),
            params![id],
            row_to_team,
        )
        .optional()
        .db_err("get_team")
    }

    /// A team by its (case-insensitive) name — the "taken name" check and the
    /// tool's `team: "<name>"` addressing.
    pub fn get_team_by_name(&self, name: &str) -> Result<Option<Team>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            &format!(
                "SELECT {TEAM_COLUMNS} FROM teams WHERE lower(name) = lower(?1)
                 ORDER BY created_at DESC LIMIT 1"
            ),
            params![name.trim()],
            row_to_team,
        )
        .optional()
        .db_err("get_team_by_name")
    }

    /// The team mirrored to a hub channel, if any. The comm dispatch path uses
    /// this to tag mirrored traffic for the frontend.
    pub fn get_team_by_hub_channel(&self, hub_channel_id: &str) -> Result<Option<Team>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            &format!("SELECT {TEAM_COLUMNS} FROM teams WHERE hub_channel_id = ?1"),
            params![hub_channel_id],
            row_to_team,
        )
        .optional()
        .db_err("get_team_by_hub_channel")
    }

    /// Replace a team's name, mission, and members in one write. Callers
    /// validate (the tools crate's `team::update` is the ONE rule set).
    pub fn update_team(
        &self,
        id: &str,
        name: &str,
        mission: &str,
        member_agent_ids: &[String],
        organizer_agent_id: &str,
    ) -> Result<Option<Team>, NeboError> {
        let conn = self.conn()?;
        let members = serde_json::to_string(member_agent_ids)
            .map_err(|e| NeboError::Internal(format!("serialize team members: {e}")))?;
        conn.query_row(
            &format!(
                "UPDATE teams SET name = ?2, mission = ?3, member_agent_ids = ?4, organizer_agent_id = ?5
                 WHERE id = ?1 RETURNING {TEAM_COLUMNS}"
            ),
            params![id, name, mission, members, organizer_agent_id],
            row_to_team,
        )
        .optional()
        .db_err("update_team")
    }

    pub fn set_team_hub_channel(&self, id: &str, hub_channel_id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE teams SET hub_channel_id = ?2 WHERE id = ?1",
            params![id, hub_channel_id],
        )
        .db_err("set_team_hub_channel")?;
        Ok(())
    }

    pub fn delete_team(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM teams WHERE id = ?1", params![id])
            .db_err("delete_team")?;
        Ok(())
    }

    /// Get-or-create the team's local thread (`team:<id>`): a session plus a
    /// real chat row titled after the team, so the thread renders with a
    /// readable name. Returns the chat id messages are stored under.
    pub fn ensure_team_thread(&self, team_id: &str, title: &str) -> Result<String, NeboError> {
        let key = team_thread_key(team_id);
        let session_id = uuid::Uuid::new_v4().to_string();
        let session = self.get_or_create_scoped_session(&session_id, &key, "agent", "", None)?;
        if let Some(chat_id) = session.active_chat_id.filter(|c| !c.is_empty()) {
            return Ok(chat_id);
        }
        let chat_id = uuid::Uuid::new_v4().to_string();
        self.create_chat_for_session(&chat_id, &key, title, None)?;
        self.set_session_active_chat_id(&session.id, &chat_id)?;
        Ok(chat_id)
    }

    /// Append one post to the team's local thread. `from_agent_id` is empty
    /// for the owner. The sender rides message metadata so the transcript
    /// labels every row correctly after a reload.
    pub fn append_team_message(
        &self,
        team: &Team,
        role: &str,
        content: &str,
        sender_name: &str,
        from_agent_id: &str,
        attachments: &serde_json::Value,
    ) -> Result<TeamMessage, NeboError> {
        let chat_id = self.ensure_team_thread(&team.id, &team.name)?;
        let meta = serde_json::json!({
            "senderName": sender_name,
            "fromAgentId": from_agent_id,
            "teamId": team.id,
            "attachments": attachments,
        })
        .to_string();
        let msg = self.create_chat_message_for_runner(
            &uuid::Uuid::new_v4().to_string(),
            &chat_id,
            role,
            content,
            None,
            None,
            None,
            Some(&meta),
            Some(&team_thread_key(&team.id)),
        )?;
        Ok(message_from_row(msg))
    }

    /// The team's transcript, oldest first; `limit` keeps the newest rows.
    pub fn list_team_messages(&self, team_id: &str, limit: usize) -> Result<Vec<TeamMessage>, NeboError> {
        let key = team_thread_key(team_id);
        let Some(session) = self.get_session_by_name(&key)? else {
            return Ok(Vec::new());
        };
        let Some(chat_id) = session.active_chat_id.filter(|c| !c.is_empty()) else {
            return Ok(Vec::new());
        };
        let mut rows = self.get_chat_messages(&chat_id)?;
        if limit > 0 && rows.len() > limit {
            rows.drain(..rows.len() - limit);
        }
        Ok(rows.into_iter().map(message_from_row).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        let path = std::env::temp_dir().join(format!("nebo-teams-test-{}.db", uuid::Uuid::new_v4()));
        Store::new(&path.to_string_lossy()).expect("store")
    }

    fn members() -> Vec<String> {
        vec!["chief".to_string(), "ea".to_string()]
    }

    /// A team is a local row: created with no hub channel, listed, fetched by
    /// id and by name (case-insensitively), and deleted.
    #[test]
    fn team_is_a_local_row() {
        let s = store();
        let team = s
            .create_team("t-1", "Operations", "Run the office", &members(), "chief", None)
            .unwrap();
        assert_eq!(team.id, "t-1");
        assert_eq!(team.hub_channel_id, None);
        assert_eq!(team.organizer_agent_id, "chief");
        assert_eq!(team.member_agent_ids, members());

        assert_eq!(s.list_teams().unwrap().len(), 1);
        assert_eq!(s.get_team("t-1").unwrap().unwrap().name, "Operations");
        assert_eq!(s.get_team_by_name("operations").unwrap().unwrap().id, "t-1");
        assert!(s.get_team("nope").unwrap().is_none());
        assert!(s.get_team_by_hub_channel("ch-1").unwrap().is_none());

        s.delete_team("t-1").unwrap();
        assert!(s.list_teams().unwrap().is_empty());
    }

    /// The hub mirror is optional and additive: set later, found by channel.
    #[test]
    fn hub_channel_is_optional_and_lookupable() {
        let s = store();
        s.create_team("t-1", "Ops", "", &members(), "", None).unwrap();
        s.set_team_hub_channel("t-1", "ch-9").unwrap();
        let by_channel = s.get_team_by_hub_channel("ch-9").unwrap().unwrap();
        assert_eq!(by_channel.id, "t-1");
        assert_eq!(by_channel.hub_channel_id.as_deref(), Some("ch-9"));

        let mirrored = s
            .create_team("t-2", "Sales", "", &members(), "", Some("ch-2"))
            .unwrap();
        assert_eq!(mirrored.hub_channel_id.as_deref(), Some("ch-2"));
    }

    /// Posts live in the team's own thread (`team:<id>`), keep their sender,
    /// and read back oldest-first with a newest-N limit.
    #[test]
    fn messages_live_in_the_team_thread() {
        let s = store();
        let team = s.create_team("t-1", "Ops", "m", &members(), "chief", None).unwrap();
        assert!(s.list_team_messages("t-1", 50).unwrap().is_empty());

        let first = s.append_team_message(&team, "user", "hello team", "Owner", "", &serde_json::Value::Array(vec![])).unwrap();
        assert_eq!(first.from, "Owner");
        assert_eq!(first.role, "user");
        assert_eq!(first.from_agent_id, "");
        s.append_team_message(&team, "assistant", "on it", "Chief of Staff", "chief", &serde_json::Value::Array(vec![])).unwrap();
        s.append_team_message(&team, "assistant", "booked", "Executive Assistant", "ea", &serde_json::Value::Array(vec![])).unwrap();

        let all = s.list_team_messages("t-1", 0).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].content, "hello team");
        assert_eq!(all[1].from_agent_id, "chief");

        let newest = s.list_team_messages("t-1", 2).unwrap();
        assert_eq!(newest.len(), 2);
        assert_eq!(newest[0].content, "on it");
        assert_eq!(newest[1].content, "booked");

        // The thread is one session keyed `team:<id>` with one chat.
        let session = s.get_session_by_name(&team_thread_key("t-1")).unwrap().unwrap();
        assert_eq!(session.active_chat_id.as_deref(), Some(s.ensure_team_thread("t-1", "Ops").unwrap().as_str()));
    }
}
