use rusqlite::params;

use crate::{DbErrExt, OptionalExt};
use crate::Store;
use crate::models::{Agent, AgentWorkflow, EmitSource};
use types::NeboError;

impl Store {
    pub fn list_agents(&self, limit: i64, offset: i64) -> Result<Vec<Agent>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, kind, name, description, agent_md, frontmatter,
                        pricing_model, pricing_cost, is_enabled, installed_at, updated_at,
                        napp_path, input_values, is_app, app_ui_path, app_binary_path, app_window_config, soul, rules, handle, color, loop_exposed, loop_agent_id, department, voice, name_locked
                 FROM agents ORDER BY installed_at DESC LIMIT ?1 OFFSET ?2",
            )
            .db_err("list_agents prepare")?;
        let rows = stmt
            .query_map(params![limit, offset], row_to_agent)
            .db_err("list_agents query")?;
        rows.collect::<Result<Vec<_>, _>>()
            .db_err("list_agents collect")
    }

    pub fn count_agents(&self) -> Result<i64, NeboError> {
        let conn = self.conn()?;
        conn.query_row("SELECT COUNT(*) FROM agents", [], |row| row.get(0))
            .db_err("count_agents")
    }

    pub fn get_agent(&self, id: &str) -> Result<Option<Agent>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, kind, name, description, agent_md, frontmatter,
                    pricing_model, pricing_cost, is_enabled, installed_at, updated_at,
                    napp_path, input_values, is_app, app_ui_path, app_binary_path, app_window_config, soul, rules, handle, color, loop_exposed, loop_agent_id, department, voice, name_locked
             FROM agents WHERE id = ?1",
            params![id],
            row_to_agent,
        )
        .optional()
        .db_err("get_agent")
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
                       napp_path, input_values, is_app, app_ui_path, app_binary_path, app_window_config, soul, rules, handle, color, loop_exposed, loop_agent_id, department, voice, name_locked",
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
        soul: Option<&str>,
        rules: Option<&str>,
        handle: Option<&str>,
        color: Option<&str>,
        loop_exposed: Option<bool>,
        voice: Option<&str>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        // A blank name is never written and never locks: locking one would leave
        // the row nameless with no way back, since sync_agent_identity only
        // restores the manifest name while name_locked = 0. Same guard shape the
        // sync uses, so the two agree on what counts as a name.
        conn.execute(
            "UPDATE agents SET name_locked = CASE WHEN TRIM(?1) != '' AND ?1 != name THEN 1 ELSE name_locked END,
                    name = CASE WHEN TRIM(?1) != '' THEN ?1 ELSE name END,
                    description = ?2, agent_md = ?3,
                    frontmatter = ?4, pricing_model = ?5, pricing_cost = ?6,
                    soul = COALESCE(?7, soul),
                    rules = COALESCE(?8, rules),
                    handle = COALESCE(?9, handle),
                    color = COALESCE(?10, color),
                    loop_exposed = COALESCE(?11, loop_exposed),
                    voice = COALESCE(?12, voice),
                    updated_at = unixepoch()
             WHERE id = ?13",
            params![
                name,
                description,
                agent_md,
                frontmatter,
                pricing_model,
                pricing_cost,
                soul,
                rules,
                handle,
                color,
                loop_exposed.map(|b| b as i32),
                voice,
                id
            ],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Set an agent's "Expose to Loop" flag. Used to seed the primary agent's
    /// default (ON) at row creation; the toggle save path uses `update_agent`.
    pub fn set_loop_exposed(&self, id: &str, exposed: bool) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE agents SET loop_exposed = ?2, updated_at = unixepoch() WHERE id = ?1",
            params![id, exposed as i32],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Sync filesystem-owned columns: content (agent_md, frontmatter) and
    /// manifest identity (name, description). The manifest is the source of
    /// truth for display name — without this, agents get stuck with slug names.
    ///
    /// Owner-set runtime state rides IN the DB frontmatter and the filesystem
    /// knows nothing about it, so a content refresh must carry it forward:
    /// `memory.context_isolated` (the isolation toggle) previously vanished on
    /// every server restart, silently un-isolating employees. The owner's DB
    /// value always wins over the publisher's shipped default.
    pub fn sync_agent_content(
        &self,
        id: &str,
        agent_md: &str,
        frontmatter: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        let existing: Option<String> = conn
            .query_row(
                "SELECT frontmatter FROM agents WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()
            .db_err("sync_agent_content read")?;
        let mut merged = frontmatter.to_string();
        if let Some(iso) = existing
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|fm| fm.pointer("/memory/context_isolated").and_then(|v| v.as_bool()))
        {
            if let Ok(mut incoming) = serde_json::from_str::<serde_json::Value>(frontmatter) {
                if let Some(obj) = incoming.as_object_mut() {
                    let mem = obj
                        .entry("memory")
                        .or_insert_with(|| serde_json::json!({}));
                    if let Some(mem_obj) = mem.as_object_mut() {
                        mem_obj.insert("context_isolated".into(), serde_json::json!(iso));
                        merged = incoming.to_string();
                    }
                }
            }
        }
        conn.execute(
            "UPDATE agents SET agent_md = ?1, frontmatter = ?2, updated_at = unixepoch()
             WHERE id = ?3",
            params![agent_md, merged, id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Flip `memory.context_isolated` in the DB frontmatter and return the
    /// merged frontmatter so the caller can mirror it to agent.json. Used
    /// when a phone line is attached: a receptionist's callers must never
    /// share memory, so the line forces isolation on.
    pub fn set_agent_context_isolated(&self, id: &str, isolated: bool) -> Result<serde_json::Value, NeboError> {
        let conn = self.conn()?;
        let current: String = conn
            .query_row("SELECT frontmatter FROM agents WHERE id = ?1", params![id], |r| r.get(0))
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let mut fm: serde_json::Value = serde_json::from_str(&current).unwrap_or(serde_json::json!({}));
        if !fm.is_object() {
            fm = serde_json::json!({});
        }
        let mem = fm
            .as_object_mut()
            .unwrap()
            .entry("memory")
            .or_insert_with(|| serde_json::json!({}));
        if !mem.is_object() {
            *mem = serde_json::json!({});
        }
        mem.as_object_mut().unwrap().insert("context_isolated".into(), serde_json::json!(isolated));
        conn.execute(
            "UPDATE agents SET frontmatter = ?2, updated_at = unixepoch() WHERE id = ?1",
            params![id, fm.to_string()],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(fm)
    }

    /// Sync display name and description from the manifest.
    /// Only updates if the manifest provides non-empty values, and never
    /// overwrites an owner-renamed (name_locked) name — the boot FS→DB sync
    /// runs on every restart and used to revert christened names to the
    /// bundled manifest's default.
    pub fn sync_agent_identity(
        &self,
        id: &str,
        name: &str,
        description: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE agents SET name = CASE WHEN ?2 != '' AND name_locked = 0 THEN ?2 ELSE name END,
                    description = CASE WHEN ?3 != '' THEN ?3 ELSE description END,
                    updated_at = unixepoch()
             WHERE id = ?1",
            params![id, name, description],
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
                    napp_path, input_values, is_app, app_ui_path, app_binary_path, app_window_config, soul, rules, handle, color, loop_exposed, loop_agent_id, department, voice, name_locked
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
        // Clean up per-agent state with no FK cascade so deleting an agent doesn't
        // leave orphans: its entity_config rows (tracked bug) and its
        // artifact-update-tracking row. Best-effort — a failure here must not block
        // the delete itself.
        let _ = conn.execute("DELETE FROM entity_config WHERE entity_id = ?1", params![id]);
        let _ = conn.execute(
            "DELETE FROM artifact_update_prefs WHERE artifact_id = ?1 AND artifact_type = 'agent'",
            params![id],
        );
        Ok(())
    }

    pub fn set_agent_app_fields(
        &self,
        id: &str,
        is_app: bool,
        app_ui_path: Option<&str>,
        app_binary_path: Option<&str>,
        app_window_config: Option<&str>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE agents SET is_app = ?1, app_ui_path = ?2, app_binary_path = ?3,
                    app_window_config = ?4, updated_at = unixepoch()
             WHERE id = ?5",
            params![
                is_app as i32,
                app_ui_path,
                app_binary_path,
                app_window_config,
                id
            ],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn set_agent_department(&self, id: &str, department: Option<&str>) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE agents SET department = ?1, updated_at = unixepoch() WHERE id = ?2",
            params![department, id],
        )
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

    /// Set (or clear, with None) the NeboAI loop agent UUID for an agent.
    /// Kept separate from `update_agent` so reconcile can capture/backfill/clear
    /// this field without clobbering user-edited identity columns.
    pub fn set_agent_loop_agent_id(
        &self,
        id: &str,
        loop_agent_id: Option<&str>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE agents SET loop_agent_id = ?1 WHERE id = ?2",
            params![loop_agent_id, id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Look up a local agent by its NeboAI loop agent UUID. Used by the channel
    /// branch to resolve an `<@{loop_agent_id}>` mention token to a local agent.
    pub fn get_agent_by_loop_agent_id(
        &self,
        loop_agent_id: &str,
    ) -> Result<Option<Agent>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, kind, name, description, agent_md, frontmatter,
                    pricing_model, pricing_cost, is_enabled, installed_at, updated_at,
                    napp_path, input_values, is_app, app_ui_path, app_binary_path, app_window_config, soul, rules, handle, color, loop_exposed, loop_agent_id, department, voice, name_locked
             FROM agents WHERE loop_agent_id = ?1",
            params![loop_agent_id],
            row_to_agent,
        )
        .optional()
        .db_err("get_agent_by_loop_agent_id")
    }

    /// Persist the NeboAI agent-space conversation id observed for an agent.
    /// Durable side of the in-memory ConvMaps conv→agent association so inbound
    /// DMs still resolve after a restart (before the join repopulates ConvMaps).
    pub fn set_agent_loop_conv_id(&self, id: &str, loop_conv_id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE agents SET loop_conv_id = ?1 WHERE id = ?2",
            params![loop_conv_id, id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Resolve a local agent id from a NeboAI agent-space conversation id.
    /// Fallback used by the inbound DM path when ConvMaps has no entry yet.
    pub fn get_agent_id_by_loop_conv_id(
        &self,
        loop_conv_id: &str,
    ) -> Result<Option<String>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id FROM agents WHERE loop_conv_id = ?1",
            params![loop_conv_id],
            |row| row.get(0),
        )
        .optional()
        .db_err("get_agent_id_by_loop_conv_id")
    }

    // ── Agent Workflow Bindings ──

    /// `owner_modified` says who is writing: `true` for the owner's surfaces
    /// (API PUT/POST, the work tool) — the write always lands and flags the
    /// row; `false` for package sync (boot FS→DB, legacy migration) — the
    /// write lands only on rows the owner has never touched. A cloud bot's
    /// image roll re-extracts the packaged agent.json, and an unconditional
    /// upsert silently reverted live owner edits on every restart.
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
        connections: Option<&str>,
        owner_modified: bool,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO agent_workflows (agent_id, binding_name,
                    trigger_type, trigger_config, description, inputs, emit, activities, connections, owner_modified)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(agent_id, binding_name) DO UPDATE SET
                trigger_type = excluded.trigger_type,
                trigger_config = excluded.trigger_config,
                description = excluded.description,
                inputs = excluded.inputs,
                emit = excluded.emit,
                activities = excluded.activities,
                connections = excluded.connections,
                owner_modified = MAX(agent_workflows.owner_modified, excluded.owner_modified)
             WHERE agent_workflows.owner_modified = 0 OR excluded.owner_modified = 1",
            params![agent_id, binding_name,
                    trigger_type, trigger_config, description, inputs, emit, activities, connections,
                    owner_modified as i64],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn list_agent_workflows(&self, agent_id: &str) -> Result<Vec<AgentWorkflow>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, agent_id, binding_name,
                        trigger_type, trigger_config, description, inputs, is_active, emit, activities, last_fired, connections
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
    pub fn is_agent_workflow_active(
        &self,
        agent_id: &str,
        binding_name: &str,
    ) -> Result<bool, NeboError> {
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
                        aw.trigger_type, aw.trigger_config, aw.description, aw.inputs, aw.is_active, aw.emit, aw.activities, aw.last_fired, aw.connections
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

    /// Watch-trigger configs of active bindings, with the owning agent's
    /// name — used to surface `{plugin}.{event}` auto-emission sources.
    pub fn list_watch_trigger_configs(&self) -> Result<Vec<(String, String, String)>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT a.name, aw.binding_name, aw.trigger_config
                 FROM agent_workflows aw
                 JOIN agents a ON aw.agent_id = a.id
                 WHERE aw.trigger_type = 'watch' AND aw.is_active = 1",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
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
            .execute("DELETE FROM cron_jobs WHERE name LIKE ?1", params![pattern])
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
        // Both the base agent scope AND its isolation contexts — the old
        // base-only pattern left every sealed matter's memories alive after
        // the employee was deleted. Chunks/embeddings cascade via FK.
        let base = format!("%:agent:{}", agent_id);
        let ctx = format!("%:agent:{}:ctx:%", agent_id);
        let n = conn
            .execute(
                "DELETE FROM memories WHERE user_id LIKE ?1 OR user_id LIKE ?2",
                params![base, ctx],
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(n)
    }

    /// The other half of "deleted things leave memory": memories in ANY scope
    /// whose key or value mentions the deleted thing by name get a tombstone
    /// note appended (never destroyed — a memory may hold other facts), and
    /// their chunks are cleared so the boot backfill re-embeds the corrected
    /// text. Deterministic, no LLM. Names shorter than 4 chars are skipped —
    /// a substring sweep on "Al" would maul unrelated memories.
    pub fn tombstone_memories_mentioning(
        &self,
        name: &str,
        note: &str,
    ) -> Result<usize, NeboError> {
        let name = name.trim();
        if name.len() < 4 {
            return Ok(0);
        }
        let conn = self.conn()?;
        let pattern = format!("%{}%", name.to_lowercase());
        let ids: Vec<i64> = {
            let mut stmt = conn
                .prepare(
                    "SELECT id FROM memories
                     WHERE (LOWER(value) LIKE ?1 OR LOWER(key) LIKE ?1)
                       AND value NOT LIKE '%' || ?2 || '%'",
                )
                .map_err(|e| NeboError::Database(e.to_string()))?;
            let rows = stmt
                .query_map(params![pattern, note], |r| r.get(0))
                .map_err(|e| NeboError::Database(e.to_string()))?;
            rows.filter_map(|r| r.ok()).collect()
        };
        for id in &ids {
            conn.execute(
                "UPDATE memories SET value = value || ' [' || ?2 || ']', updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
                params![id, note],
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
            conn.execute(
                "DELETE FROM memory_chunks WHERE memory_id = ?1",
                params![id],
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        Ok(ids.len())
    }

    /// Delete all workflow run history for this agent.
    /// Agent runs use workflow_id = "agent:{agent_id}".
    /// Activity results cascade-delete via FK.
    pub fn delete_agent_workflow_runs(&self, agent_id: &str) -> Result<usize, NeboError> {
        let conn = self.conn()?;
        let wf_id = types::keyparser::agent_workflow_id(agent_id);
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
    let connections_str: Option<String> = row.get(11)?;
    let connections = connections_str.and_then(|s| serde_json::from_str(&s).ok());
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
        connections,
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
        input_values: row
            .get::<_, Option<String>>(12)?
            .unwrap_or_else(|| "{}".to_string()),
        is_app: row.get(13)?,
        app_ui_path: row.get(14)?,
        app_binary_path: row.get(15)?,
        app_window_config: row.get(16)?,
        soul: row.get(17)?,
        rules: row.get(18)?,
        handle: row.get(19)?,
        color: row.get(20)?,
        loop_exposed: row.get(21)?,
        loop_agent_id: row.get(22)?,
        department: row.get(23)?,
        voice: row.get(24)?,
        name_locked: row.get(25)?,
    })
}

#[cfg(test)]
mod owner_modified_tests {
    use crate::Store;

    fn store() -> Store {
        let path = std::env::temp_dir()
            .join(format!("nebo-wfown-test-{}.db", uuid::Uuid::new_v4()));
        let s = Store::new(&path.to_string_lossy()).expect("store");
        s.create_agent("a1", None, "Test", "", "", "{}", None, None)
            .expect("agent row");
        s
    }

    fn activities(s: &Store) -> String {
        s.list_agent_workflows("a1").unwrap()[0]
            .activities
            .as_ref()
            .map(|v| v.to_string())
            .unwrap_or_default()
    }

    /// The Biss incident: a pod restart re-synced the packaged agent.json
    /// over a live owner restructure. Package sync (owner_modified=false)
    /// must never touch a row the owner has written.
    #[test]
    fn package_sync_cannot_clobber_owner_edits() {
        let s = store();
        // Package install seeds the binding.
        s.upsert_agent_workflow("a1", "order-intake", "watch", "{}", None, None, None,
            Some(r#"[{"id":"v1"}]"#), None, false).unwrap();
        assert!(activities(&s).contains("v1"));
        // Owner restructures it.
        s.upsert_agent_workflow("a1", "order-intake", "watch", "{}", None, None, None,
            Some(r#"[{"id":"v2-owner"}]"#), None, true).unwrap();
        // Boot re-sync from the packaged file: must be a no-op.
        s.upsert_agent_workflow("a1", "order-intake", "watch", "{}", None, None, None,
            Some(r#"[{"id":"v1"}]"#), None, false).unwrap();
        assert!(activities(&s).contains("v2-owner"), "package sync reverted an owner edit");
        // A later owner edit still lands.
        s.upsert_agent_workflow("a1", "order-intake", "watch", "{}", None, None, None,
            Some(r#"[{"id":"v3-owner"}]"#), None, true).unwrap();
        assert!(activities(&s).contains("v3-owner"));
    }

    /// Untouched package rows keep following the package.
    #[test]
    fn package_sync_still_updates_pristine_rows() {
        let s = store();
        s.upsert_agent_workflow("a1", "daily-brief", "schedule", "0 0 8 * * * *", None, None, None,
            Some(r#"[{"id":"pkg1"}]"#), None, false).unwrap();
        s.upsert_agent_workflow("a1", "daily-brief", "schedule", "0 0 9 * * * *", None, None, None,
            Some(r#"[{"id":"pkg2"}]"#), None, false).unwrap();
        let rows = s.list_agent_workflows("a1").unwrap();
        assert_eq!(rows[0].trigger_config, "0 0 9 * * * *");
        assert!(activities(&s).contains("pkg2"));
    }
}
