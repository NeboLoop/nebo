use rusqlite::params;

use crate::models::{EmitSource, Agent, AgentWorkflow};
use crate::OptionalExt;
use crate::Store;
use types::NeboError;

impl Store {
    pub fn list_agents(&self, limit: i64, offset: i64) -> Result<Vec<Agent>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, kind, name, description, agent_md, frontmatter,
                        pricing_model, pricing_cost, is_enabled, installed_at, updated_at,
                        napp_path, input_values
                 FROM agents ORDER BY installed_at DESC LIMIT ?1 OFFSET ?2",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![limit, offset], row_to_agent)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn count_agents(&self) -> Result<i64, NeboError> {
        let conn = self.conn()?;
        conn.query_row("SELECT COUNT(*) FROM agents", [], |row| row.get(0))
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_agent(&self, id: &str) -> Result<Option<Agent>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, kind, name, description, agent_md, frontmatter,
                    pricing_model, pricing_cost, is_enabled, installed_at, updated_at,
                    napp_path, input_values
             FROM agents WHERE id = ?1",
            params![id],
            row_to_agent,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn create_agent(
        &self,
        id: &str,
        kind: Option<&str>,
        name: &str,
        description: &str,
        agent_md: &str,
        frontmatter: &str,
        pricing_model: Option<&str>,
        pricing_cost: Option<f64>,
    ) -> Result<Agent, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "INSERT INTO agents (id, kind, name, description, agent_md, frontmatter,
                    pricing_model, pricing_cost)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             RETURNING id, kind, name, description, agent_md, frontmatter,
                       pricing_model, pricing_cost, is_enabled, installed_at, updated_at,
                       napp_path, input_values",
            params![id, kind, name, description, agent_md, frontmatter, pricing_model, pricing_cost],
            row_to_agent,
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn update_agent(
        &self,
        id: &str,
        name: &str,
        description: &str,
        agent_md: &str,
        frontmatter: &str,
        pricing_model: Option<&str>,
        pricing_cost: Option<f64>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE agents SET name = ?1, description = ?2, agent_md = ?3,
                    frontmatter = ?4, pricing_model = ?5, pricing_cost = ?6,
                    updated_at = unixepoch()
             WHERE id = ?7",
            params![name, description, agent_md, frontmatter, pricing_model, pricing_cost, id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Get an agent by name (case-insensitive).
    pub fn get_agent_by_name(&self, name: &str) -> Result<Option<Agent>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, kind, name, description, agent_md, frontmatter,
                    pricing_model, pricing_cost, is_enabled, installed_at, updated_at,
                    napp_path, input_values
             FROM agents WHERE LOWER(name) = LOWER(?1)",
            params![name],
            row_to_agent,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Check if an agent is installed by matching its name (case-insensitive).
    pub fn agent_installed_by_name(&self, name: &str) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agents WHERE LOWER(name) = LOWER(?1)",
                params![name],
                |row| row.get(0),
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(count > 0)
    }

    pub fn delete_agent(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM agents WHERE id = ?1", params![id])
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn set_agent_napp_path(&self, id: &str, napp_path: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE agents SET napp_path = ?1, updated_at = unixepoch() WHERE id = ?2",
            params![napp_path, id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn update_agent_input_values(&self, id: &str, input_values: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE agents SET input_values = ?1, updated_at = unixepoch() WHERE id = ?2",
            params![input_values, id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn toggle_agent(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE agents SET is_enabled = NOT is_enabled, updated_at = unixepoch() WHERE id = ?1",
            params![id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn set_agent_enabled(&self, id: &str, enabled: bool) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE agents SET is_enabled = ?1, updated_at = unixepoch() WHERE id = ?2",
            params![enabled as i32, id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    // ── Agent Workflow Bindings ──

    pub fn upsert_agent_workflow(
        &self,
        agent_id: &str,
        binding_name: &str,
        trigger_type: &str,
        trigger_config: &str,
        description: Option<&str>,
        inputs: Option<&str>,
        emit: Option<&str>,
        activities: Option<&str>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO agent_workflows (agent_id, binding_name,
                    trigger_type, trigger_config, description, inputs, emit, activities)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(agent_id, binding_name) DO UPDATE SET
                trigger_type = excluded.trigger_type,
                trigger_config = excluded.trigger_config,
                description = excluded.description,
                inputs = excluded.inputs,
                emit = excluded.emit,
                activities = excluded.activities",
            params![agent_id, binding_name,
                    trigger_type, trigger_config, description, inputs, emit, activities],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn list_agent_workflows(&self, agent_id: &str) -> Result<Vec<AgentWorkflow>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, agent_id, binding_name,
                        trigger_type, trigger_config, description, inputs, is_active, emit, activities, last_fired
                 FROM agent_workflows WHERE agent_id = ?1",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![agent_id], row_to_agent_workflow)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn delete_single_agent_workflow(
        &self,
        agent_id: &str,
        binding_name: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM agent_workflows WHERE agent_id = ?1 AND binding_name = ?2",
            params![agent_id, binding_name],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn toggle_agent_workflow(
        &self,
        agent_id: &str,
        binding_name: &str,
    ) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE agent_workflows SET is_active = NOT is_active WHERE agent_id = ?1 AND binding_name = ?2",
            params![agent_id, binding_name],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        // Return new is_active state
        let is_active: i64 = conn
            .query_row(
                "SELECT is_active FROM agent_workflows WHERE agent_id = ?1 AND binding_name = ?2",
                params![agent_id, binding_name],
                |row| row.get(0),
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(is_active != 0)
    }

    pub fn delete_agent_workflows(&self, agent_id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM agent_workflows WHERE agent_id = ?1",
            params![agent_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Check if an agent workflow is active AND its parent agent is enabled.
    pub fn is_agent_workflow_active(&self, agent_id: &str, binding_name: &str) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_workflows aw
                 JOIN agents a ON aw.agent_id = a.id
                 WHERE aw.agent_id = ?1 AND aw.binding_name = ?2
                   AND aw.is_active = 1 AND a.is_enabled = 1",
                params![agent_id, binding_name],
                |row| row.get(0),
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(count > 0)
    }

    pub fn list_active_event_triggers(&self) -> Result<Vec<AgentWorkflow>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT aw.id, aw.agent_id, aw.binding_name,
                        aw.trigger_type, aw.trigger_config, aw.description, aw.inputs, aw.is_active, aw.emit, aw.activities, aw.last_fired
                 FROM agent_workflows aw
                 JOIN agents a ON aw.agent_id = a.id
                 WHERE aw.trigger_type = 'event' AND aw.is_active = 1 AND a.is_enabled = 1",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], row_to_agent_workflow)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn update_agent_workflow_last_fired(
        &self,
        agent_id: &str,
        binding_name: &str,
        fired_at: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE agent_workflows SET last_fired = ?1
             WHERE agent_id = ?2 AND binding_name = ?3",
            params![fired_at, agent_id, binding_name],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn list_emit_sources(&self) -> Result<Vec<EmitSource>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT aw.emit, a.name AS agent_name, aw.binding_name, aw.description
                 FROM agent_workflows aw
                 JOIN agents a ON aw.agent_id = a.id
                 WHERE aw.emit IS NOT NULL AND aw.emit != '' AND aw.is_active = 1",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(EmitSource {
                    emit: row.get(0)?,
                    agent_name: row.get(1)?,
                    binding_name: row.get(2)?,
                    description: row.get(3)?,
                })
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn delete_cron_jobs_by_prefix(&self, prefix: &str) -> Result<i64, NeboError> {
        let conn = self.conn()?;
        let pattern = format!("{}%", prefix);
        let count = conn
            .execute(
                "DELETE FROM cron_jobs WHERE name LIKE ?1",
                params![pattern],
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(count as i64)
    }

    // ── Agent data cleanup (on delete) ──

    /// Delete all chats belonging to agent sessions.
    /// Must be called BEFORE delete_agent_sessions (uses session_name pattern).
    /// Chat messages cascade-delete via FK.
    pub fn delete_agent_chats(&self, agent_id: &str) -> Result<usize, NeboError> {
        let conn = self.conn()?;
        let pattern = format!("agent:{}:%", agent_id);
        conn.execute(
            "DELETE FROM chats WHERE session_name LIKE ?1",
            params![pattern],
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Delete all sessions scoped to this agent.
    pub fn delete_agent_sessions(&self, agent_id: &str) -> Result<usize, NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM sessions WHERE scope = 'agent' AND scope_id = ?1",
            params![agent_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Delete all memories extracted during conversations with this agent.
    /// Memory user_id format: "{user_id}:agent:{agent_id}".
    /// Memory chunks cascade-delete via FK.
    pub fn delete_agent_memories(&self, agent_id: &str) -> Result<usize, NeboError> {
        let conn = self.conn()?;
        let pattern = format!("%:agent:{}", agent_id);
        conn.execute(
            "DELETE FROM memories WHERE user_id LIKE ?1",
            params![pattern],
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Delete all workflow run history for this agent.
    /// Agent runs use workflow_id = "agent:{agent_id}".
    /// Activity results cascade-delete via FK.
    pub fn delete_agent_workflow_runs(&self, agent_id: &str) -> Result<usize, NeboError> {
        let conn = self.conn()?;
        let wf_id = format!("agent:{}", agent_id);
        conn.execute(
            "DELETE FROM workflow_runs WHERE workflow_id = ?1",
            params![wf_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }
}

fn row_to_agent_workflow(row: &rusqlite::Row) -> rusqlite::Result<AgentWorkflow> {
    let activities_str: Option<String> = row.get(9)?;
    let activities = activities_str.and_then(|s| serde_json::from_str(&s).ok());
    Ok(AgentWorkflow {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        binding_name: row.get(2)?,
        trigger_type: row.get(3)?,
        trigger_config: row.get(4)?,
        description: row.get(5)?,
        inputs: row.get(6)?,
        is_active: row.get(7)?,
        emit: row.get(8)?,
        activities,
        last_fired: row.get(10)?,
    })
}

fn row_to_agent(row: &rusqlite::Row) -> rusqlite::Result<Agent> {
    Ok(Agent {
        id: row.get(0)?,
        kind: row.get(1)?,
        name: row.get(2)?,
        description: row.get(3)?,
        agent_md: row.get(4)?,
        frontmatter: row.get(5)?,
        pricing_model: row.get(6)?,
        pricing_cost: row.get(7)?,
        is_enabled: row.get(8)?,
        installed_at: row.get(9)?,
        updated_at: row.get(10)?,
        napp_path: row.get(11)?,
        input_values: row.get::<_, Option<String>>(12)?.unwrap_or_else(|| "{}".to_string()),
    })
}
