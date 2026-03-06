use rusqlite::params;

use crate::models::Advisor;
use crate::Store;
use types::NeboError;

impl Store {
    pub fn list_advisors(&self) -> Result<Vec<Advisor>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, role, description, priority, enabled, memory_access,
                        persona, timeout_seconds, created_at, updated_at
                 FROM advisors ORDER BY priority DESC",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(Advisor {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    role: row.get(2)?,
                    description: row.get(3)?,
                    priority: row.get(4)?,
                    enabled: row.get(5)?,
                    memory_access: row.get(6)?,
                    persona: row.get(7)?,
                    timeout_seconds: row.get(8)?,
                    created_at: row.get(9)?,
                    updated_at: row.get(10)?,
                })
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;

        let mut advisors = Vec::new();
        for row in rows {
            advisors.push(row.map_err(|e| NeboError::Database(e.to_string()))?);
        }
        Ok(advisors)
    }

    pub fn get_advisor_by_name(&self, name: &str) -> Result<Option<Advisor>, NeboError> {
        let conn = self.conn()?;
        match conn.query_row(
            "SELECT id, name, role, description, priority, enabled, memory_access,
                    persona, timeout_seconds, created_at, updated_at
             FROM advisors WHERE name = ?1",
            params![name],
            |row| {
                Ok(Advisor {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    role: row.get(2)?,
                    description: row.get(3)?,
                    priority: row.get(4)?,
                    enabled: row.get(5)?,
                    memory_access: row.get(6)?,
                    persona: row.get(7)?,
                    timeout_seconds: row.get(8)?,
                    created_at: row.get(9)?,
                    updated_at: row.get(10)?,
                })
            },
        ) {
            Ok(a) => Ok(Some(a)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(NeboError::Database(e.to_string())),
        }
    }

    pub fn create_advisor(
        &self,
        name: &str,
        role: &str,
        description: &str,
        priority: i64,
        persona: &str,
        timeout_seconds: i64,
    ) -> Result<Advisor, NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO advisors (name, role, description, priority, persona, timeout_seconds)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![name, role, description, priority, persona, timeout_seconds],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;

        self.get_advisor_by_name(name)?
            .ok_or_else(|| NeboError::Database("failed to retrieve created advisor".into()))
    }

    pub fn update_advisor(
        &self,
        id: i64,
        role: Option<&str>,
        description: Option<&str>,
        priority: Option<i64>,
        persona: Option<&str>,
        enabled: Option<bool>,
        memory_access: Option<bool>,
        timeout_seconds: Option<i64>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;

        if let Some(v) = role {
            conn.execute("UPDATE advisors SET role = ?1, updated_at = unixepoch() WHERE id = ?2", params![v, id])
                .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        if let Some(v) = description {
            conn.execute("UPDATE advisors SET description = ?1, updated_at = unixepoch() WHERE id = ?2", params![v, id])
                .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        if let Some(v) = priority {
            conn.execute("UPDATE advisors SET priority = ?1, updated_at = unixepoch() WHERE id = ?2", params![v, id])
                .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        if let Some(v) = persona {
            conn.execute("UPDATE advisors SET persona = ?1, updated_at = unixepoch() WHERE id = ?2", params![v, id])
                .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        if let Some(v) = enabled {
            let val: i64 = if v { 1 } else { 0 };
            conn.execute("UPDATE advisors SET enabled = ?1, updated_at = unixepoch() WHERE id = ?2", params![val, id])
                .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        if let Some(v) = memory_access {
            let val: i64 = if v { 1 } else { 0 };
            conn.execute("UPDATE advisors SET memory_access = ?1, updated_at = unixepoch() WHERE id = ?2", params![val, id])
                .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        if let Some(v) = timeout_seconds {
            conn.execute("UPDATE advisors SET timeout_seconds = ?1, updated_at = unixepoch() WHERE id = ?2", params![v, id])
                .map_err(|e| NeboError::Database(e.to_string()))?;
        }

        Ok(())
    }

    pub fn delete_advisor(&self, id: i64) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM advisors WHERE id = ?1", params![id])
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }
}
