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

/// The segment that marks a project's thread inside a team's key space.
const PROJECT_SEGMENT: &str = ":project:";

/// The ONE builder for a project's thread key: `team:<team>:project:<id>`.
/// A project thread is a team-style thread, so it carries the team's prefix
/// (read state and the roster treat it as one); a one-off project, with no
/// team, is `team::project:<id>`.
pub fn project_thread_key(team_id: &str, project_id: &str) -> String {
    format!("{TEAM_THREAD_PREFIX}{team_id}{PROJECT_SEGMENT}{project_id}")
}

/// Which thread a `team:` key is: the team's own, or one of its projects'.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TeamThread<'a> {
    Team(&'a str),
    Project { team_id: &'a str, project_id: &'a str },
}

/// The ONE reading of a team-style thread key; `None` when `key` is not one.
pub fn team_thread_of(key: &str) -> Option<TeamThread<'_>> {
    let rest = key.strip_prefix(TEAM_THREAD_PREFIX)?;
    Some(match rest.split_once(PROJECT_SEGMENT) {
        Some((team_id, project_id)) => TeamThread::Project { team_id, project_id },
        None => TeamThread::Team(rest),
    })
}

/// One member of a team: which machine runs them, and who they are on it.
///
/// A member used to be a bare local agent id, which assumed every member lives
/// on this machine. The owner's workforce is spread across machines, so the
/// pair is stored from the first line and a cross-bot team is a feature rather
/// than a migration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamMember {
    /// Empty means THIS machine — also what an unlinked Nebo has, so it is the
    /// only honest default. A remote member names the bot that runs it.
    #[serde(default)]
    pub bot_id: String,
    /// Local agent id for a local member; the HUB agent id for a remote one,
    /// because that is the id a mention token must carry for the far machine
    /// to resolve it.
    pub agent_id: String,
    /// A label, not an identity. A local member's name is resolved live from
    /// the roster; a remote member has no local roster to ask, so this is what
    /// gets shown.
    #[serde(default)]
    pub name: String,
}

impl TeamMember {
    /// A member on this machine.
    pub fn local(agent_id: impl Into<String>) -> Self {
        Self { bot_id: String::new(), agent_id: agent_id.into(), name: String::new() }
    }

    /// True when this member runs on this machine.
    pub fn is_local(&self) -> bool {
        self.bot_id.is_empty()
    }
}

/// One team row, frontend-shaped (genapi emits this).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Team {
    pub id: String,
    pub name: String,
    pub mission: String,
    /// Everyone in this team, local and remote.
    pub members: Vec<TeamMember>,
    /// The employee that created the team; empty when the owner did.
    pub organizer_agent_id: String,
    /// The NeboAI hub channel mirroring this team, once mirrored.
    pub hub_channel_id: Option<String>,
    pub created_at: i64,
    /// What doesn't change for this team (brand kit, rules, who does what):
    /// every member's briefing carries it. Empty until the owner sets it.
    #[serde(default)]
    pub standing: String,
}

/// One project: a piece of work a lead owns, with its own thread
/// (`project_thread_key`). Frontend-shaped (genapi emits this).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    /// '' for a one-off project.
    pub team_id: String,
    pub lead_agent_id: String,
    pub title: String,
    /// What was asked, in the asker's words.
    pub ask: String,
    /// The conversation the work came from ('' = the owner, in the team).
    pub asker_session_key: String,
    /// The employee that asked ('' = the owner).
    pub asker_agent_id: String,
    pub thread_session_key: String,
    /// The lead's workflow binding a recurring project runs.
    pub playbook_binding: Option<String>,
    /// `live`, `paused` or `done`.
    pub state: String,
    pub limit_steps: i64,
    pub limit_spend_cents: i64,
    pub created_at: i64,
    pub closed_at: Option<i64>,
}

/// What `create_project` needs.
pub struct NewProject<'a> {
    pub id: &'a str,
    pub team_id: &'a str,
    pub lead_agent_id: &'a str,
    pub title: &'a str,
    pub ask: &'a str,
    pub asker_session_key: &'a str,
    pub asker_agent_id: &'a str,
    pub playbook_binding: Option<&'a str>,
}

const PROJECT_COLUMNS: &str = "id, team_id, lead_agent_id, title, ask, asker_session_key, asker_agent_id, \
     thread_session_key, playbook_binding, state, limit_steps, limit_spend_cents, created_at, closed_at";

fn row_to_project(row: &rusqlite::Row) -> rusqlite::Result<Project> {
    Ok(Project {
        id: row.get(0)?,
        team_id: row.get(1)?,
        lead_agent_id: row.get(2)?,
        title: row.get(3)?,
        ask: row.get(4)?,
        asker_session_key: row.get(5)?,
        asker_agent_id: row.get(6)?,
        thread_session_key: row.get(7)?,
        playbook_binding: row.get(8)?,
        state: row.get(9)?,
        limit_steps: row.get(10)?,
        limit_spend_cents: row.get(11)?,
        created_at: row.get(12)?,
        closed_at: row.get(13)?,
    })
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
    /// The untrusted content the post's words hold (its run's provenance):
    /// for the model and the gates that read the post, never for a person,
    /// so it stays off the wire.
    #[serde(skip)]
    pub provenance: Vec<types::provenance::ProvenanceClass>,
    /// Not a post: the divider where the owner cleared the team's
    /// conversation (`/clear`). Everything before it stays in the thread;
    /// no member is briefed with it again.
    #[serde(default)]
    pub cleared: bool,
    /// The card this post hands out, when it is a project's card line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card: Option<TeamCard>,
}

/// A card a project's lead hands out (`team::card_lines`): the hand-off
/// that carries it, who does it, the work, and when it counts as done. It
/// rides the post that hands it out, so the app shows that post as the
/// card and reads its state from the hand-off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamCard {
    pub handoff_id: String,
    /// The member doing it (local agent id).
    pub agent_id: String,
    pub task: String,
    /// Empty when the lead gave none.
    pub done_when: String,
}

fn row_to_team(row: &rusqlite::Row) -> rusqlite::Result<Team> {
    let members_json: String = row.get(3)?;
    Ok(Team {
        id: row.get(0)?,
        name: row.get(1)?,
        mission: row.get(2)?,
        members: serde_json::from_str(&members_json).unwrap_or_default(),
        organizer_agent_id: row.get(4)?,
        hub_channel_id: row.get(5)?,
        created_at: row.get(6)?,
        standing: row.get(7)?,
    })
}

const TEAM_COLUMNS: &str =
    "id, name, mission, members, organizer_agent_id, hub_channel_id, created_at, standing";

fn message_from_row(m: ChatMessage) -> TeamMessage {
    let meta: serde_json::Value = m
        .metadata
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(serde_json::Value::Null);
    // A post's provenance is metadata; a row written before it was still
    // begins with the label it carried then, which comes off here, so every
    // reader — a person's view, a member's briefing — gets the words alone.
    let split = types::labels::split(&m.content);
    let mut provenance: Vec<types::provenance::ProvenanceClass> =
        serde_json::from_value(meta["provenance"].clone()).unwrap_or_default();
    for class in split.provenance {
        if !provenance.contains(&class) {
            provenance.push(class);
        }
    }
    let cleared = m.role == "system" && meta["compactBoundary"] == true && meta["reason"] == "cleared";
    let card = serde_json::from_value::<Option<TeamCard>>(meta["card"].clone()).unwrap_or(None);
    TeamMessage {
        id: m.id.clone(),
        from: meta["senderName"].as_str().unwrap_or("").to_string(),
        from_agent_id: meta["fromAgentId"].as_str().unwrap_or("").to_string(),
        role: m.role.clone(),
        content: split.text.to_string(),
        attachments: meta["attachments"].as_array().cloned().unwrap_or_default(),
        created_at: m.created_at,
        provenance,
        cleared,
        card,
    }
}

impl Store {
    pub fn create_team(
        &self,
        id: &str,
        name: &str,
        mission: &str,
        members: &[TeamMember],
        organizer_agent_id: &str,
        hub_channel_id: Option<&str>,
    ) -> Result<Team, NeboError> {
        let conn = self.conn()?;
        let members = serde_json::to_string(members)
            .map_err(|e| NeboError::Internal(format!("serialize team members: {e}")))?;
        conn.query_row(
            &format!(
                "INSERT INTO teams (id, name, mission, members, organizer_agent_id, hub_channel_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 RETURNING {TEAM_COLUMNS}"
            ),
            params![id, name, mission, members, organizer_agent_id, hub_channel_id],
            row_to_team,
        )
        .db_err("create_team")
    }

    /// Every team, A to Z by name: the one order every client shows, so the
    /// desktop and the phone never disagree and a list never reorders under
    /// the owner's finger.
    pub fn list_teams(&self) -> Result<Vec<Team>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {TEAM_COLUMNS} FROM teams ORDER BY name COLLATE NOCASE, id"
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

    /// Replace a team's name, mission, members, lead and standing rules in
    /// one write. Callers validate (the tools crate's `team::update` is the
    /// ONE rule set).
    pub fn update_team(
        &self,
        id: &str,
        name: &str,
        mission: &str,
        members: &[TeamMember],
        organizer_agent_id: &str,
        standing: &str,
    ) -> Result<Option<Team>, NeboError> {
        let conn = self.conn()?;
        let members = serde_json::to_string(members)
            .map_err(|e| NeboError::Internal(format!("serialize team members: {e}")))?;
        conn.query_row(
            &format!(
                "UPDATE teams SET name = ?2, mission = ?3, members = ?4, organizer_agent_id = ?5, standing = ?6
                 WHERE id = ?1 RETURNING {TEAM_COLUMNS}"
            ),
            params![id, name, mission, members, organizer_agent_id, standing],
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
        // A removed team is no longer temporary work either.
        conn.execute("DELETE FROM temporary_work WHERE kind = 'team' AND name = ?1", params![id])
            .db_err("delete_team temporary")?;
        Ok(())
    }

    /// Get-or-create a team-style thread (`team_thread_key` or
    /// `project_thread_key`): a session plus a real chat row titled after the
    /// team or project, so the thread renders with a readable name. Returns
    /// the chat id messages are stored under.
    pub fn ensure_team_thread(&self, key: &str, title: &str) -> Result<String, NeboError> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let session = self.get_or_create_scoped_session(&session_id, key, "agent", "", None)?;
        if let Some(chat_id) = session.active_chat_id.filter(|c| !c.is_empty()) {
            return Ok(chat_id);
        }
        let chat_id = uuid::Uuid::new_v4().to_string();
        self.create_chat_for_session(&chat_id, key, title, None)?;
        self.set_session_active_chat_id(&session.id, &chat_id)?;
        Ok(chat_id)
    }

    /// Append one post to a team-style thread: the team's own
    /// (`team_thread_key`) or one of its projects' (`project_thread_key`,
    /// `project` named). `from_agent_id` is empty for the owner. The sender,
    /// the post's provenance and the card it hands out (if any) ride message
    /// metadata, so the transcript labels every row correctly after a reload
    /// and the words stay the words.
    pub fn append_team_message(
        &self,
        team: &Team,
        project: Option<&Project>,
        role: &str,
        content: &str,
        sender_name: &str,
        from_agent_id: &str,
        attachments: &serde_json::Value,
        provenance: &[types::provenance::ProvenanceClass],
        card: Option<&TeamCard>,
    ) -> Result<TeamMessage, NeboError> {
        let (key, title) = match project {
            Some(p) => (p.thread_session_key.clone(), p.title.as_str()),
            None => (team_thread_key(&team.id), team.name.as_str()),
        };
        let chat_id = self.ensure_team_thread(&key, title)?;
        let mut meta = serde_json::json!({
            "senderName": sender_name,
            "fromAgentId": from_agent_id,
            "teamId": team.id,
            "attachments": attachments,
            "provenance": provenance,
        });
        if let Some(p) = project {
            meta["projectId"] = serde_json::json!(p.id);
        }
        if let Some(c) = card {
            meta["card"] = serde_json::json!(c);
        }
        let meta = meta.to_string();
        let msg = self.create_chat_message_for_runner(
            &uuid::Uuid::new_v4().to_string(),
            &chat_id,
            role,
            content,
            None,
            None,
            None,
            Some(&meta),
            Some(&key),
            None,
        )?;
        Ok(message_from_row(msg))
    }

    /// A team-style thread's transcript (`team_thread_key` or
    /// `project_thread_key`), oldest first; `limit` keeps the newest rows.
    /// Nebo's own rows (`isMeta`, such as the boundary a clear writes for
    /// the model) are not part of it.
    pub fn list_team_messages(&self, key: &str, limit: usize) -> Result<Vec<TeamMessage>, NeboError> {
        let Some(session) = self.get_session_by_name(key)? else {
            return Ok(Vec::new());
        };
        let Some(chat_id) = session.active_chat_id.filter(|c| !c.is_empty()) else {
            return Ok(Vec::new());
        };
        let mut rows = self.get_chat_messages(&chat_id)?;
        rows.retain(|m| {
            let meta: serde_json::Value = m.metadata.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
            meta["isMeta"] != true && meta["isMeta"] != "true"
        });
        if limit > 0 && rows.len() > limit {
            rows.drain(..rows.len() - limit);
        }
        Ok(rows.into_iter().map(message_from_row).collect())
    }

    /// The newest post across a team's threads, its own and its projects':
    /// the roster row's latest line. A clear's divider and Nebo's own rows
    /// (`isMeta`) are not posts.
    pub fn latest_team_message(&self, team_id: &str) -> Result<Option<TeamMessage>, NeboError> {
        let own = team_thread_key(team_id);
        let projects = format!("{own}{PROJECT_SEGMENT}");
        let conn = self.conn()?;
        conn.query_row(
            "SELECT m.* FROM chat_messages m JOIN chats c ON c.id = m.chat_id
             WHERE (c.session_name = ?1 OR substr(c.session_name, 1, length(?2)) = ?2)
               AND m.role IN ('user', 'assistant')
               AND COALESCE(CASE WHEN json_valid(m.metadata) THEN json_extract(m.metadata, '$.isMeta') END, 0) NOT IN (1, 'true')
             ORDER BY m.created_at DESC, m.rowid DESC
             LIMIT 1",
            params![own, projects],
            super::chats::row_to_chat_message,
        )
        .optional()
        .db_err("latest_team_message")
        .map(|m| m.map(message_from_row))
    }
}

impl Store {
    /// Open a project: its row and its thread's key. Its thread is created
    /// with the first post (`append_team_message`).
    pub fn create_project(&self, p: &NewProject<'_>) -> Result<Project, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            &format!(
                "INSERT INTO projects (id, team_id, lead_agent_id, title, ask, asker_session_key, asker_agent_id,
                                       thread_session_key, playbook_binding)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 RETURNING {PROJECT_COLUMNS}"
            ),
            params![
                p.id,
                p.team_id,
                p.lead_agent_id,
                p.title,
                p.ask,
                p.asker_session_key,
                p.asker_agent_id,
                project_thread_key(p.team_id, p.id),
                p.playbook_binding,
            ],
            row_to_project,
        )
        .db_err("create_project")
    }

    pub fn get_project(&self, id: &str) -> Result<Option<Project>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(&format!("SELECT {PROJECT_COLUMNS} FROM projects WHERE id = ?1"), params![id], row_to_project)
            .optional()
            .db_err("get_project")
    }

    /// A team's projects: live and paused ones first, newest first within
    /// each, then the done ones, newest first.
    pub fn list_team_projects(&self, team_id: &str) -> Result<Vec<Project>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {PROJECT_COLUMNS} FROM projects WHERE team_id = ?1
                 ORDER BY (state = 'done'), COALESCE(closed_at, created_at) DESC, id"
            ))
            .db_err("list_team_projects prepare")?;
        let rows = stmt.query_map(params![team_id], row_to_project).db_err("list_team_projects query")?;
        rows.collect::<Result<Vec<_>, _>>().db_err("list_team_projects collect")
    }

    /// Move a project to `live`, `paused` or `done`; `done` stamps when it
    /// closed, any other state clears it (a post reopens a done project).
    pub fn set_project_state(&self, id: &str, state: &str, now: i64) -> Result<Option<Project>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            &format!(
                "UPDATE projects SET state = ?2, closed_at = CASE WHEN ?2 = 'done' THEN ?3 ELSE NULL END
                 WHERE id = ?1 RETURNING {PROJECT_COLUMNS}"
            ),
            params![id, state, now],
            row_to_project,
        )
        .optional()
        .db_err("set_project_state")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        let path = std::env::temp_dir().join(format!("nebo-teams-test-{}.db", uuid::Uuid::new_v4()));
        Store::new(&path.to_string_lossy()).expect("store")
    }

    fn members() -> Vec<TeamMember> {
        vec![TeamMember::local("chief"), TeamMember::local("ea")]
    }

    /// Teams list A to Z, case-insensitively, whatever order they were made
    /// in: the one order the desktop and the phone both show.
    #[test]
    fn teams_list_a_to_z() {
        let s = store();
        for (id, name) in [("t-1", "operations"), ("t-2", "Customer Support"), ("t-3", "Market Research")] {
            s.create_team(id, name, "", &members(), "chief", None).unwrap();
        }
        let names: Vec<String> = s.list_teams().unwrap().into_iter().map(|t| t.name).collect();
        assert_eq!(names, ["Customer Support", "Market Research", "operations"]);
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
        assert_eq!(team.members, members());

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
        assert!(s.list_team_messages(&team_thread_key("t-1"), 50).unwrap().is_empty());

        let first = s.append_team_message(&team, None, "user", "hello team", "Owner", "", &serde_json::Value::Array(vec![]), &[], None).unwrap();
        assert_eq!(first.from, "Owner");
        assert_eq!(first.role, "user");
        assert_eq!(first.from_agent_id, "");
        s.append_team_message(&team, None, "assistant", "on it", "Chief of Staff", "chief", &serde_json::Value::Array(vec![]), &[], None).unwrap();
        s.append_team_message(&team, None, "assistant", "booked", "Executive Assistant", "ea", &serde_json::Value::Array(vec![]), &[], None).unwrap();

        let all = s.list_team_messages(&team_thread_key("t-1"), 0).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].content, "hello team");
        assert_eq!(all[1].from_agent_id, "chief");

        let newest = s.list_team_messages(&team_thread_key("t-1"), 2).unwrap();
        assert_eq!(newest.len(), 2);
        assert_eq!(newest[0].content, "on it");
        assert_eq!(newest[1].content, "booked");

        // The thread is one session keyed `team:<id>` with one chat.
        let session = s.get_session_by_name(&team_thread_key("t-1")).unwrap().unwrap();
        assert_eq!(session.active_chat_id.as_deref(), Some(s.ensure_team_thread(&team_thread_key("t-1"), "Ops").unwrap().as_str()));
    }

    /// A project lives with its team: its thread is a team-style thread
    /// keyed by the team and the project, its rows carry the project, the
    /// team's own thread never sees them, and its list puts live work first.
    #[test]
    fn a_project_is_a_thread_of_its_team() {
        let s = store();
        let team = s.create_team("t-1", "Launch", "m", &members(), "chief", None).unwrap();
        let new = |id: &'static str| NewProject {
            id,
            team_id: "t-1",
            lead_agent_id: "chief",
            title: "PH launch",
            ask: "launch us",
            asker_session_key: "",
            asker_agent_id: "",
            playbook_binding: None,
        };
        let first = s.create_project(&new("p-1")).unwrap();
        assert_eq!((first.state.as_str(), first.closed_at), ("live", None));
        assert_eq!(first.thread_session_key, "team:t-1:project:p-1");
        assert_eq!(team_thread_of(&first.thread_session_key), Some(TeamThread::Project { team_id: "t-1", project_id: "p-1" }));
        assert_eq!(team_thread_of("team:t-1"), Some(TeamThread::Team("t-1")));
        assert_eq!(team_thread_of("agent:a:web"), None);

        s.append_team_message(&team, Some(&first), "user", "launch us", "Owner", "", &serde_json::json!([]), &[], None).unwrap();
        let rows = s.list_team_messages(&first.thread_session_key, 0).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(s.list_team_messages(&team_thread_key("t-1"), 0).unwrap().is_empty(), "the team's own thread is untouched");

        s.create_project(&new("p-2")).unwrap();
        let done = s.set_project_state("p-1", "done", 50).unwrap().unwrap();
        assert_eq!((done.state.as_str(), done.closed_at), ("done", Some(50)));
        let order: Vec<String> = s.list_team_projects("t-1").unwrap().into_iter().map(|p| p.id).collect();
        assert_eq!(order, ["p-2", "p-1"], "live first, done after");
        let reopened = s.set_project_state("p-1", "live", 60).unwrap().unwrap();
        assert_eq!(reopened.closed_at, None);
        assert!(s.create_project(&new("p-1")).is_err(), "one row per project");
    }

    /// The roster row's latest line is the newest post in any of the
    /// team's threads, its own or a project's, and never another team's.
    /// A card line keeps the card it hands out.
    #[test]
    fn the_latest_line_spans_the_teams_threads() {
        let s = store();
        let team = s.create_team("t-1", "Launch", "m", &members(), "chief", None).unwrap();
        let other = s.create_team("t-10", "Other", "m", &members(), "chief", None).unwrap();
        assert!(s.latest_team_message("t-1").unwrap().is_none());

        s.append_team_message(&team, None, "user", "kick off", "Owner", "", &serde_json::json!([]), &[], None).unwrap();
        assert_eq!(s.latest_team_message("t-1").unwrap().unwrap().content, "kick off");

        let project = s
            .create_project(&NewProject {
                id: "p-1",
                team_id: "t-1",
                lead_agent_id: "chief",
                title: "PH launch",
                ask: "launch us",
                asker_session_key: "",
                asker_agent_id: "",
                playbook_binding: None,
            })
            .unwrap();
        let card = TeamCard {
            handoff_id: "h-1".into(),
            agent_id: "ea".into(),
            task: "hero script".into(),
            done_when: "saved and on brand".into(),
        };
        let posted = s
            .append_team_message(&team, Some(&project), "assistant", "Executive Assistant: hero script", "Chief of Staff", "chief", &serde_json::json!([]), &[], Some(&card))
            .unwrap();
        assert_eq!(posted.card.as_ref(), Some(&card));
        let latest = s.latest_team_message("t-1").unwrap().unwrap();
        assert_eq!((latest.from.as_str(), latest.card.as_ref()), ("Chief of Staff", Some(&card)));
        assert_eq!(s.list_team_messages(&project.thread_session_key, 0).unwrap()[0].card.as_ref(), Some(&card));

        // Another team's post (even one whose id starts the same) is not this team's.
        s.append_team_message(&other, None, "assistant", "elsewhere", "Chief of Staff", "chief", &serde_json::json!([]), &[], None).unwrap();
        assert_eq!(s.latest_team_message("t-1").unwrap().unwrap().content, "Executive Assistant: hero script");
        assert_eq!(s.latest_team_message("t-10").unwrap().unwrap().content, "elsewhere");
    }

    /// The standing rules are the team's own field: empty at first, set with
    /// the rest of the team in one write, and read back.
    #[test]
    fn a_team_keeps_its_standing_rules() {
        let s = store();
        let team = s.create_team("t-1", "Launch", "m", &members(), "chief", None).unwrap();
        assert_eq!(team.standing, "");
        let updated = s.update_team("t-1", "Launch", "m", &members(), "chief", "Never compare to competitors.").unwrap().unwrap();
        assert_eq!(updated.standing, "Never compare to competitors.");
        assert_eq!(s.get_team("t-1").unwrap().unwrap().standing, "Never compare to competitors.");
    }
}
