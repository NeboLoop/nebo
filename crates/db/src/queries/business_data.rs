//! What belongs to the business when an employee goes. Deleting an
//! employee removes what is the employee's own; the cases it worked, the
//! turns it ran, what it sent, what was approved — the business history —
//! stays, attributed to the employee's id with the name it had. Two
//! explicit operations exist beside deletion: EXPORT the business history
//! as one JSON document, and PURGE it, which is destructive and never
//! implied.

use rusqlite::params;

use crate::Store;
use types::NeboError;

impl Store {
    /// Keep the name an employee id had, so history can still say who.
    pub fn tombstone_agent(&self, id: &str, name: &str, now: i64) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO deleted_agents (id, name, deleted_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, deleted_at = excluded.deleted_at",
            params![id, name, now],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// The name an employee id has or had.
    pub fn agent_display_name(&self, id: &str) -> Result<Option<String>, NeboError> {
        if let Some(a) = self.get_agent(id)? {
            return Ok(Some(a.name));
        }
        let conn = self.conn()?;
        conn.query_row("SELECT name FROM deleted_agents WHERE id = ?1", params![id], |r| r.get(0))
            .optional()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// One JSON document of everything the business keeps about an
    /// employee's work: cases with their history, waits, turns and
    /// effects; every other run; the subjects those cases were about.
    pub fn export_agent_business_data(&self, agent_id: &str) -> Result<serde_json::Value, NeboError> {
        let conn = self.conn()?;
        let rows = |sql: &str| -> Result<Vec<serde_json::Value>, NeboError> {
            let mut stmt = conn.prepare(sql).map_err(|e| NeboError::Database(e.to_string()))?;
            let cols: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
            let out = stmt
                .query_map(params![agent_id], |row| {
                    let mut obj = serde_json::Map::new();
                    for (i, c) in cols.iter().enumerate() {
                        let v: rusqlite::types::Value = row.get(i)?;
                        obj.insert(c.clone(), sql_value_to_json(v));
                    }
                    Ok(serde_json::Value::Object(obj))
                })
                .map_err(|e| NeboError::Database(e.to_string()))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| NeboError::Database(e.to_string()))?;
            Ok(out)
        };
        let runs = rows("SELECT * FROM engine_runs WHERE agent_id = ?1 ORDER BY created_at, rowid")?;
        let events = rows(
            "SELECT e.* FROM engine_events e WHERE e.target_type = 'run' AND e.target_id IN (SELECT id FROM engine_runs WHERE agent_id = ?1)
             OR e.target_id IN (SELECT k.key_type || ':' || k.key_value FROM engine_run_keys k JOIN engine_runs r ON r.id = k.run_id WHERE r.agent_id = ?1)
             ORDER BY e.id",
        )?;
        let waits = rows("SELECT w.* FROM engine_waits w WHERE w.run_id IN (SELECT id FROM engine_runs WHERE agent_id = ?1) ORDER BY w.id")?;
        let keys = rows("SELECT k.* FROM engine_run_keys k WHERE k.run_id IN (SELECT id FROM engine_runs WHERE agent_id = ?1) ORDER BY k.id")?;
        let effects = rows("SELECT f.* FROM engine_effects f WHERE f.run_id IN (SELECT id FROM engine_runs WHERE agent_id = ?1) OR f.run_id LIKE 'agent:' || ?1 || ':%' ORDER BY f.id")?;
        let workflow_runs = rows("SELECT w.* FROM workflow_runs w WHERE w.workflow_id = 'agent:' || ?1 ORDER BY w.started_at, w.rowid")?;
        let activity_results = rows("SELECT a.* FROM workflow_activity_results a WHERE a.run_id IN (SELECT id FROM workflow_runs WHERE workflow_id = 'agent:' || ?1) ORDER BY a.rowid")?;
        let subjects = rows(
            "SELECT s.*, (SELECT json_group_array(json_object('kind', a.kind, 'value', a.value, 'source', a.source)) FROM engine_subject_aliases a WHERE a.subject_id = s.id) AS aliases
             FROM engine_subjects s WHERE s.id IN (SELECT k.key_value FROM engine_run_keys k JOIN engine_runs r ON r.id = k.run_id WHERE r.agent_id = ?1 AND k.key_type LIKE 'case:%')",
        )?;
        let notifications = rows("SELECT * FROM notifications WHERE agent_id = ?1 ORDER BY created_at")?;
        Ok(serde_json::json!({
            "format": "nebo-employee-business-data/1",
            "exported_at": chrono::Utc::now().to_rfc3339(),
            "employee": { "id": agent_id, "name": self.agent_display_name(agent_id)?, "deleted": self.get_agent(agent_id)?.is_none() },
            "runs": runs,
            "events": events,
            "waits": waits,
            "keys": keys,
            "effects": effects,
            "workflow_runs": workflow_runs,
            "activity_results": activity_results,
            "subjects": subjects,
            "notifications": notifications,
        }))
    }

    /// Destroy the business history of an employee: its runs and
    /// everything hanging off them, its workflow runs, its cards. The
    /// subjects stay (other employees may know the same people). Returns
    /// what was removed.
    pub fn purge_agent_business_data(&self, agent_id: &str, now: i64) -> Result<serde_json::Value, NeboError> {
        let mut conn = self.conn()?;
        let tx = conn.transaction().map_err(|e| NeboError::Database(e.to_string()))?;
        let mut counts = serde_json::Map::new();
        let mut del = |name: &str, sql: &str| -> Result<(), NeboError> {
            let n = tx.execute(sql, params![agent_id]).map_err(|e| NeboError::Database(e.to_string()))?;
            counts.insert(name.to_string(), serde_json::json!(n));
            Ok(())
        };
        del("effects", "DELETE FROM engine_effects WHERE run_id IN (SELECT id FROM engine_runs WHERE agent_id = ?1) OR run_id LIKE 'agent:' || ?1 || ':%'")?;
        del("events", "DELETE FROM engine_events WHERE (target_type = 'run' AND target_id IN (SELECT id FROM engine_runs WHERE agent_id = ?1))
             OR target_id IN (SELECT k.key_type || ':' || k.key_value FROM engine_run_keys k JOIN engine_runs r ON r.id = k.run_id WHERE r.agent_id = ?1)")?;
        del("waits", "DELETE FROM engine_waits WHERE run_id IN (SELECT id FROM engine_runs WHERE agent_id = ?1)")?;
        del("keys", "DELETE FROM engine_run_keys WHERE run_id IN (SELECT id FROM engine_runs WHERE agent_id = ?1)")?;
        del("activity_results", "DELETE FROM workflow_activity_results WHERE run_id IN (SELECT id FROM workflow_runs WHERE workflow_id = 'agent:' || ?1)")?;
        del("workflow_runs", "DELETE FROM workflow_runs WHERE workflow_id = 'agent:' || ?1")?;
        del("runs", "DELETE FROM engine_runs WHERE agent_id = ?1")?;
        del("notifications", "DELETE FROM notifications WHERE agent_id = ?1")?;
        tx.execute("UPDATE deleted_agents SET purged_at = ?2 WHERE id = ?1", params![agent_id, now])
            .map_err(|e| NeboError::Database(e.to_string()))?;
        tx.commit().map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(serde_json::Value::Object(counts))
    }
}

fn sql_value_to_json(v: rusqlite::types::Value) -> serde_json::Value {
    use rusqlite::types::Value as V;
    match v {
        V::Null => serde_json::Value::Null,
        V::Integer(i) => serde_json::json!(i),
        V::Real(f) => serde_json::json!(f),
        V::Text(s) => serde_json::from_str::<serde_json::Value>(&s)
            .ok()
            .filter(|j| j.is_object() || j.is_array())
            .unwrap_or(serde_json::Value::String(s)),
        V::Blob(b) => serde_json::json!(format!("<{} bytes>", b.len())),
    }
}

trait OptionalExt<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error>;
}

impl<T> OptionalExt<T> for rusqlite::Result<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::queries::engine::{NewEvent, NewRun};
    use crate::Store;

    fn store() -> Store {
        let path = std::env::temp_dir().join(format!("nebo-bizdata-{}.db", uuid::Uuid::new_v4()));
        Store::new(&path.to_string_lossy()).expect("store")
    }

    /// Deleting the employee keeps the business history under its id and
    /// name; export returns all of it; purge removes exactly it and nothing
    /// of another employee's.
    #[test]
    fn business_history_survives_deletion_exports_whole_and_purges_exactly() {
        let s = store();
        for agent in ["a", "b"] {
            s.engine_create_run(&NewRun { id: &format!("case-{agent}"), kind: "case", session_key: "k", agent_id: agent, lane: "main", ..Default::default() }).unwrap();
            s.engine_bind_key(&format!("case-{agent}"), "case:lead", &format!("subj-{agent}")).unwrap();
            s.engine_enqueue_event(&NewEvent { kind: "turn_result", target_type: "run", target_id: &format!("case-{agent}"), payload: "sent the email", idem_key: &format!("t-{agent}"), durable: true, ..Default::default() }).unwrap();
            s.create_workflow_run(&format!("wf-{agent}"), &format!("agent:{agent}"), "case", Some("work-lead"), None, None, Some("{}")).unwrap();
            s.engine_effect_pending(&format!("wf-{agent}"), "messaging", &format!("send-{agent}"), "sms", "", "").unwrap();
        }
        s.tombstone_agent("a", "Intake Coordinator", 1_000).unwrap();
        assert_eq!(s.agent_display_name("a").unwrap().as_deref(), Some("Intake Coordinator"));

        let export = s.export_agent_business_data("a").unwrap();
        assert_eq!(export["employee"]["name"], "Intake Coordinator");
        assert_eq!(export["employee"]["deleted"], true);
        assert_eq!(export["runs"].as_array().unwrap().len(), 2, "the case and the workflow run are both engine runs of this employee");
        assert_eq!(export["events"].as_array().unwrap().len(), 1);
        assert_eq!(export["workflow_runs"].as_array().unwrap().len(), 1);
        assert_eq!(export["effects"].as_array().unwrap().len(), 1);
        assert!(export["runs"].as_array().unwrap().iter().any(|r| r["id"] == "case-a"));

        let removed = s.purge_agent_business_data("a", 2_000).unwrap();
        assert_eq!((removed["runs"].as_i64(), removed["events"].as_i64(), removed["effects"].as_i64(), removed["workflow_runs"].as_i64()), (Some(2), Some(1), Some(1), Some(1)));
        assert!(s.engine_get_run("case-a").unwrap().is_none());
        assert!(s.engine_get_run("case-b").unwrap().is_some(), "the other employee's history is untouched");
        assert!(s.get_workflow_run("wf-b").unwrap().is_some());
        assert_eq!(s.export_agent_business_data("a").unwrap()["runs"].as_array().unwrap().len(), 0);
    }
}
