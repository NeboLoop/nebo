use rusqlite::params;

use crate::models::Role;
use crate::OptionalExt;
use crate::Store;
use types::NeboError;

impl Store {
    pub fn list_roles(&self, limit: i64, offset: i64) -> Result<Vec<Role>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, code, name, description, role_md, frontmatter,
                        pricing_model, pricing_cost, is_enabled, installed_at, updated_at
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
                    pricing_model, pricing_cost, is_enabled, installed_at, updated_at
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
                       pricing_model, pricing_cost, is_enabled, installed_at, updated_at",
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

    pub fn toggle_role(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE roles SET is_enabled = NOT is_enabled, updated_at = unixepoch() WHERE id = ?1",
            params![id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }
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
    })
}

