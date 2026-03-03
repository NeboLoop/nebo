use rusqlite::params;

use crate::models::AgentProfile;
use crate::Store;
use types::NeboError;

impl Store {
    pub fn get_agent_profile(&self) -> Result<Option<AgentProfile>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, name, personality_preset, custom_personality, voice_style, response_length,
                    emoji_usage, formality, proactivity, emoji, creature, vibe, role, avatar,
                    agent_rules, tool_notes, quiet_hours_start, quiet_hours_end, created_at, updated_at
             FROM agent_profile WHERE id = 1",
            [],
            row_to_agent_profile,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn ensure_agent_profile(&self) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO agent_profile (id, name, created_at, updated_at)
             VALUES (1, 'Nebo', unixepoch(), unixepoch())
             ON CONFLICT(id) DO NOTHING",
            [],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn update_agent_profile(
        &self,
        name: Option<&str>,
        personality_preset: Option<&str>,
        custom_personality: Option<&str>,
        voice_style: Option<&str>,
        response_length: Option<&str>,
        emoji_usage: Option<&str>,
        formality: Option<&str>,
        proactivity: Option<&str>,
        emoji: Option<&str>,
        creature: Option<&str>,
        vibe: Option<&str>,
        role: Option<&str>,
        avatar: Option<&str>,
        agent_rules: Option<&str>,
        tool_notes: Option<&str>,
        quiet_hours_start: Option<&str>,
        quiet_hours_end: Option<&str>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE agent_profile SET
                name = COALESCE(?1, name),
                personality_preset = COALESCE(?2, personality_preset),
                custom_personality = COALESCE(?3, custom_personality),
                voice_style = COALESCE(?4, voice_style),
                response_length = COALESCE(?5, response_length),
                emoji_usage = COALESCE(?6, emoji_usage),
                formality = COALESCE(?7, formality),
                proactivity = COALESCE(?8, proactivity),
                emoji = COALESCE(?9, emoji),
                creature = COALESCE(?10, creature),
                vibe = COALESCE(?11, vibe),
                role = COALESCE(?12, role),
                avatar = COALESCE(?13, avatar),
                agent_rules = COALESCE(?14, agent_rules),
                tool_notes = COALESCE(?15, tool_notes),
                quiet_hours_start = COALESCE(?16, quiet_hours_start),
                quiet_hours_end = COALESCE(?17, quiet_hours_end),
                updated_at = unixepoch()
             WHERE id = 1",
            params![
                name,
                personality_preset,
                custom_personality,
                voice_style,
                response_length,
                emoji_usage,
                formality,
                proactivity,
                emoji,
                creature,
                vibe,
                role,
                avatar,
                agent_rules,
                tool_notes,
                quiet_hours_start,
                quiet_hours_end,
            ],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }
}

fn row_to_agent_profile(row: &rusqlite::Row) -> rusqlite::Result<AgentProfile> {
    Ok(AgentProfile {
        id: row.get("id")?,
        name: row.get("name")?,
        personality_preset: row.get("personality_preset")?,
        custom_personality: row.get("custom_personality")?,
        voice_style: row.get("voice_style")?,
        response_length: row.get("response_length")?,
        emoji_usage: row.get("emoji_usage")?,
        formality: row.get("formality")?,
        proactivity: row.get("proactivity")?,
        emoji: row.get("emoji")?,
        creature: row.get("creature")?,
        vibe: row.get("vibe")?,
        role: row.get("role")?,
        avatar: row.get("avatar")?,
        agent_rules: row.get("agent_rules")?,
        tool_notes: row.get("tool_notes")?,
        quiet_hours_start: row.get("quiet_hours_start")?,
        quiet_hours_end: row.get("quiet_hours_end")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
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
