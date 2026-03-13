use rusqlite::params;

use crate::models::{Role, RoleWorkflow};
use crate::OptionalExt;
use crate::Store;
use types::NeboError;

impl Store {
    pub fn list_roles(&self, limit: i64, offset: i64) -> Result<Vec<Role>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, code, name, description, role_md, frontmatter,
                        pricing_model, pricing_cost, is_enabled, installed_at, updated_at,
                        napp_path
                 FROM roles ORDER BY installed_at DESC LIMIT ?1 OFFSET ?2",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![limit, offset], row_to_role)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn count_roles(&self) -> Result<i64, NeboError> {
        let conn = self.conn()?;
        conn.query_row("SELECT COUNT(*) FROM roles", [], |row| row.get(0))
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_role(&self, id: &str) -> Result<Option<Role>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, code, name, description, role_md, frontmatter,
                    pricing_model, pricing_cost, is_enabled, installed_at, updated_at,
                    napp_path
             FROM roles WHERE id = ?1",
            params![id],
            row_to_role,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn create_role(
        &self,
        id: &str,
        code: Option<&str>,
        name: &str,
        description: &str,
        role_md: &str,
        frontmatter: &str,
        pricing_model: Option<&str>,
        pricing_cost: Option<f64>,
    ) -> Result<Role, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "INSERT INTO roles (id, code, name, description, role_md, frontmatter,
                    pricing_model, pricing_cost)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             RETURNING id, code, name, description, role_md, frontmatter,
                       pricing_model, pricing_cost, is_enabled, installed_at, updated_at,
                       napp_path",
            params![id, code, name, description, role_md, frontmatter, pricing_model, pricing_cost],
            row_to_role,
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn update_role(
        &self,
        id: &str,
        name: &str,
        description: &str,
        role_md: &str,
        frontmatter: &str,
        pricing_model: Option<&str>,
        pricing_cost: Option<f64>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE roles SET name = ?1, description = ?2, role_md = ?3,
                    frontmatter = ?4, pricing_model = ?5, pricing_cost = ?6,
                    updated_at = unixepoch()
             WHERE id = ?7",
            params![name, description, role_md, frontmatter, pricing_model, pricing_cost, id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn delete_role(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM roles WHERE id = ?1", params![id])
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn set_role_napp_path(&self, id: &str, napp_path: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE roles SET napp_path = ?1, updated_at = unixepoch() WHERE id = ?2",
            params![napp_path, id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn toggle_role(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE roles SET is_enabled = NOT is_enabled, updated_at = unixepoch() WHERE id = ?1",
            params![id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    // ── Role Workflow Bindings ──

    pub fn upsert_role_workflow(
        &self,
        role_id: &str,
        binding_name: &str,
        workflow_ref: &str,
        workflow_id: Option<&str>,
        trigger_type: &str,
        trigger_config: &str,
        description: Option<&str>,
        inputs: Option<&str>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO role_workflows (role_id, binding_name, workflow_ref, workflow_id,
                    trigger_type, trigger_config, description, inputs)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(role_id, binding_name) DO UPDATE SET
                workflow_ref = excluded.workflow_ref,
                workflow_id = excluded.workflow_id,
                trigger_type = excluded.trigger_type,
                trigger_config = excluded.trigger_config,
                description = excluded.description,
                inputs = excluded.inputs",
            params![role_id, binding_name, workflow_ref, workflow_id,
                    trigger_type, trigger_config, description, inputs],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn list_role_workflows(&self, role_id: &str) -> Result<Vec<RoleWorkflow>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, role_id, binding_name, workflow_ref, workflow_id,
                        trigger_type, trigger_config, description, inputs, is_active
                 FROM role_workflows WHERE role_id = ?1",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![role_id], row_to_role_workflow)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn delete_role_workflows(&self, role_id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM role_workflows WHERE role_id = ?1",
            params![role_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn list_active_event_triggers(&self) -> Result<Vec<RoleWorkflow>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT rw.id, rw.role_id, rw.binding_name, rw.workflow_ref, rw.workflow_id,
                        rw.trigger_type, rw.trigger_config, rw.description, rw.inputs, rw.is_active
                 FROM role_workflows rw
                 JOIN roles r ON rw.role_id = r.id
                 WHERE rw.trigger_type = 'event' AND rw.is_active = 1 AND r.is_enabled = 1",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], row_to_role_workflow)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn update_role_workflow_last_fired(
        &self,
        role_id: &str,
        binding_name: &str,
        fired_at: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE role_workflows SET last_fired = ?1
             WHERE role_id = ?2 AND binding_name = ?3",
            params![fired_at, role_id, binding_name],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
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
}

fn row_to_role_workflow(row: &rusqlite::Row) -> rusqlite::Result<RoleWorkflow> {
    Ok(RoleWorkflow {
        id: row.get(0)?,
        role_id: row.get(1)?,
        binding_name: row.get(2)?,
        workflow_ref: row.get(3)?,
        workflow_id: row.get(4)?,
        trigger_type: row.get(5)?,
        trigger_config: row.get(6)?,
        description: row.get(7)?,
        inputs: row.get(8)?,
        is_active: row.get(9)?,
    })
}

fn row_to_role(row: &rusqlite::Row) -> rusqlite::Result<Role> {
    Ok(Role {
        id: row.get(0)?,
        code: row.get(1)?,
        name: row.get(2)?,
        description: row.get(3)?,
        role_md: row.get(4)?,
        frontmatter: row.get(5)?,
        pricing_model: row.get(6)?,
        pricing_cost: row.get(7)?,
        is_enabled: row.get(8)?,
        installed_at: row.get(9)?,
        updated_at: row.get(10)?,
        napp_path: row.get(11)?,
    })
}
