//! The one-time upgrade conversions: their ledger, and the stored cells the
//! tool-name conversion reads and rewrites.

use rusqlite::{OptionalExtension, params};

use crate::{DbErrExt, Store};
use types::NeboError;

/// A stored place that can name tools: `table.column`, the column that keys
/// its rows, and which rows are read (live work only, where the rest is
/// history).
struct Place {
    name: &'static str,
    table: &'static str,
    column: &'static str,
    key: &'static str,
    rows: &'static str,
}

const PLACES: &[Place] = &[
    Place { name: "agents.agent_md", table: "agents", column: "agent_md", key: "id", rows: "1" },
    Place { name: "agents.soul", table: "agents", column: "soul", key: "id", rows: "1" },
    Place { name: "agents.rules", table: "agents", column: "rules", key: "id", rows: "1" },
    Place { name: "agents.frontmatter", table: "agents", column: "frontmatter", key: "id", rows: "1" },
    Place { name: "agent_workflows.activities", table: "agent_workflows", column: "activities", key: "id", rows: "1" },
    Place { name: "agent_workflows.description", table: "agent_workflows", column: "description", key: "id", rows: "1" },
    Place { name: "workflows.definition", table: "workflows", column: "definition", key: "id", rows: "1" },
    Place { name: "workflows.skill_md", table: "workflows", column: "skill_md", key: "id", rows: "1" },
    Place { name: "cron_jobs.command", table: "cron_jobs", column: "command", key: "id", rows: "1" },
    Place { name: "cron_jobs.message", table: "cron_jobs", column: "message", key: "id", rows: "1" },
    Place { name: "cron_jobs.instructions", table: "cron_jobs", column: "instructions", key: "id", rows: "1" },
    Place { name: "api_keys.tools", table: "api_keys", column: "tools", key: "id", rows: "revoked_at IS NULL" },
    Place { name: "entity_config.heartbeat_content", table: "entity_config", column: "heartbeat_content", key: "id", rows: "1" },
    Place { name: "entity_config.personality_snippet", table: "entity_config", column: "personality_snippet", key: "id", rows: "1" },
    Place { name: "agent_profile.custom_personality", table: "agent_profile", column: "custom_personality", key: "id", rows: "1" },
    Place { name: "agent_profile.agent_rules", table: "agent_profile", column: "agent_rules", key: "id", rows: "1" },
    Place { name: "agent_profile.tool_notes", table: "agent_profile", column: "tool_notes", key: "id", rows: "1" },
    Place { name: "user_profiles.context", table: "user_profiles", column: "context", key: "user_id", rows: "1" },
    Place { name: "advisors.persona", table: "advisors", column: "persona", key: "id", rows: "1" },
    Place { name: "teams.mission", table: "teams", column: "mission", key: "id", rows: "1" },
    // The definition a live run resumes from.
    Place {
        name: "engine_runs.definition",
        table: "engine_runs",
        column: "definition",
        key: "id",
        rows: "state IN ('queued', 'running', 'waiting', 'interrupted')",
    },
    // What a live run was started with: a task's prompt rides here.
    Place {
        name: "engine_runs.inputs",
        table: "engine_runs",
        column: "inputs",
        key: "id",
        rows: "state IN ('queued', 'running', 'waiting', 'interrupted')",
    },
    // A checklist item not yet done.
    Place { name: "pending_tasks.prompt", table: "pending_tasks", column: "prompt", key: "id", rows: "status = 'pending'" },
];

/// Every place the tool-name conversion reads, by name.
pub fn tool_naming_places() -> impl Iterator<Item = &'static str> {
    PLACES.iter().map(|p| p.name)
}

fn place(name: &str) -> Result<&'static Place, NeboError> {
    PLACES
        .iter()
        .find(|p| p.name == name)
        .ok_or_else(|| NeboError::Validation(format!("{name} is not a place that names tools")))
}

/// One stored value that can name tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolNamingCell {
    /// Its row's key.
    pub key: String,
    pub value: String,
}

impl Store {
    /// Whether the named one-time conversion has run on this install.
    pub fn upgrade_conversion_done(&self, name: &str) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        conn.query_row("SELECT 1 FROM upgrade_conversions WHERE name = ?1", params![name], |_| Ok(()))
            .optional()
            .map(|r| r.is_some())
            .db_err("upgrade_conversion_done")
    }

    /// Record a conversion as done, with what it converted.
    pub fn record_upgrade_conversion(&self, name: &str, report: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO upgrade_conversions (name, report, applied_at)
             VALUES (?1, ?2, unixepoch())",
            params![name, report],
        )
        .db_err("record_upgrade_conversion")?;
        Ok(())
    }

    /// The non-empty values of one place that can name tools.
    pub fn tool_naming_cells(&self, place_name: &str) -> Result<Vec<ToolNamingCell>, NeboError> {
        let p = place(place_name)?;
        let conn = self.conn()?;
        let sql = format!(
            "SELECT CAST({key} AS TEXT), {col} FROM {table} WHERE {col} IS NOT NULL AND {col} != '' AND ({rows})",
            key = p.key,
            col = p.column,
            table = p.table,
            rows = p.rows,
        );
        let mut stmt = conn.prepare(&sql).db_err("tool_naming_cells")?;
        let rows = stmt
            .query_map([], |row| Ok(ToolNamingCell { key: row.get(0)?, value: row.get(1)? }))
            .db_err("tool_naming_cells")?;
        rows.collect::<Result<Vec<_>, _>>().db_err("tool_naming_cells")
    }

    /// Write one place's value for the row `key`.
    pub fn set_tool_naming_cell(&self, place_name: &str, key: &str, value: &str) -> Result<(), NeboError> {
        let p = place(place_name)?;
        let conn = self.conn()?;
        let sql = format!(
            "UPDATE {table} SET {col} = ?1 WHERE CAST({key} AS TEXT) = ?2",
            table = p.table,
            col = p.column,
            key = p.key,
        );
        conn.execute(&sql, params![value, key]).db_err("set_tool_naming_cell")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_place_reads_and_writes_on_a_migrated_database() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(&dir.path().join("t.db").to_string_lossy()).unwrap();
        for name in tool_naming_places() {
            let cells = store.tool_naming_cells(name).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(cells.is_empty(), "{name}");
            store.set_tool_naming_cell(name, "none", "x").unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        assert!(store.tool_naming_cells("chat_messages.content").is_err(), "only the listed places");
    }

    #[test]
    fn a_conversion_is_recorded_once_done() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(&dir.path().join("t.db").to_string_lossy()).unwrap();
        assert!(!store.upgrade_conversion_done("x_v1").unwrap());
        store.record_upgrade_conversion("x_v1", "{}").unwrap();
        assert!(store.upgrade_conversion_done("x_v1").unwrap());
    }
}
