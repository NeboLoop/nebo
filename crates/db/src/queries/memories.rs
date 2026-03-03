use rusqlite::params;

use crate::models::Memory;
use crate::Store;
use types::NeboError;

impl Store {
    pub fn list_memories(&self, limit: i64, offset: i64) -> Result<Vec<Memory>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, namespace, key, value, tags, metadata, created_at, updated_at,
                        accessed_at, access_count, user_id
                 FROM memories ORDER BY access_count DESC LIMIT ?1 OFFSET ?2",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![limit, offset], row_to_memory)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn list_memories_by_namespace(
        &self,
        namespace_prefix: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Memory>, NeboError> {
        let conn = self.conn()?;
        let pattern = format!("{namespace_prefix}%");
        let mut stmt = conn
            .prepare(
                "SELECT id, namespace, key, value, tags, metadata, created_at, updated_at,
                        accessed_at, access_count, user_id
                 FROM memories WHERE namespace LIKE ?1
                 ORDER BY access_count DESC LIMIT ?2 OFFSET ?3",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![pattern, limit, offset], row_to_memory)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_memory(&self, id: i64) -> Result<Option<Memory>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, namespace, key, value, tags, metadata, created_at, updated_at,
                    accessed_at, access_count, user_id
             FROM memories WHERE id = ?1",
            params![id],
            row_to_memory,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_memory_by_key(
        &self,
        namespace: &str,
        key: &str,
    ) -> Result<Option<Memory>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, namespace, key, value, tags, metadata, created_at, updated_at,
                    accessed_at, access_count, user_id
             FROM memories WHERE namespace = ?1 AND key = ?2",
            params![namespace, key],
            row_to_memory,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_memory_by_key_and_user(
        &self,
        namespace: &str,
        key: &str,
        user_id: &str,
    ) -> Result<Option<Memory>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, namespace, key, value, tags, metadata, created_at, updated_at,
                    accessed_at, access_count, user_id
             FROM memories WHERE namespace = ?1 AND key = ?2 AND user_id = ?3",
            params![namespace, key, user_id],
            row_to_memory,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn upsert_memory(
        &self,
        namespace: &str,
        key: &str,
        value: &str,
        tags: Option<&str>,
        metadata: Option<&str>,
        user_id: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO memories (namespace, key, value, tags, metadata, user_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
             ON CONFLICT(namespace, key, user_id) DO UPDATE SET
                value = excluded.value,
                tags = COALESCE(excluded.tags, tags),
                metadata = COALESCE(excluded.metadata, metadata),
                updated_at = CURRENT_TIMESTAMP",
            params![namespace, key, value, tags, metadata, user_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn update_memory(
        &self,
        id: i64,
        value: Option<&str>,
        tags: Option<&str>,
        metadata: Option<&str>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE memories SET
                value = COALESCE(?2, value),
                tags = COALESCE(?3, tags),
                metadata = COALESCE(?4, metadata),
                updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1",
            params![id, value, tags, metadata],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn delete_memory(&self, id: i64) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM memories WHERE id = ?1", params![id])
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn delete_memory_by_key_and_user(
        &self,
        namespace: &str,
        key: &str,
        user_id: &str,
    ) -> Result<usize, NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM memories WHERE namespace = ?1 AND key = ?2 AND user_id = ?3",
            params![namespace, key, user_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn delete_memories_by_namespace_and_user(
        &self,
        namespace_prefix: &str,
        user_id: &str,
    ) -> Result<usize, NeboError> {
        let conn = self.conn()?;
        let pattern = format!("{namespace_prefix}%");
        conn.execute(
            "DELETE FROM memories WHERE namespace LIKE ?1 AND user_id = ?2",
            params![pattern, user_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn search_memories(
        &self,
        query: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Memory>, NeboError> {
        let conn = self.conn()?;
        let pattern = format!("%{query}%");
        let mut stmt = conn
            .prepare(
                "SELECT id, namespace, key, value, tags, metadata, created_at, updated_at,
                        accessed_at, access_count, user_id
                 FROM memories WHERE key LIKE ?1 OR value LIKE ?1 OR tags LIKE ?1
                 ORDER BY access_count DESC LIMIT ?2 OFFSET ?3",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![pattern, limit, offset], row_to_memory)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn search_memories_by_user(
        &self,
        user_id: &str,
        query: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Memory>, NeboError> {
        let conn = self.conn()?;
        let pattern = format!("%{query}%");
        let mut stmt = conn
            .prepare(
                "SELECT id, namespace, key, value, tags, metadata, created_at, updated_at,
                        accessed_at, access_count, user_id
                 FROM memories WHERE user_id = ?1
                 AND (namespace LIKE ?2 OR key LIKE ?2 OR value LIKE ?2)
                 ORDER BY access_count DESC LIMIT ?3 OFFSET ?4",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![user_id, pattern, limit, offset], |row| {
                row_to_memory(row)
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn list_memories_by_user_and_namespace(
        &self,
        user_id: &str,
        namespace_prefix: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Memory>, NeboError> {
        let conn = self.conn()?;
        let pattern = format!("{namespace_prefix}%");
        let mut stmt = conn
            .prepare(
                "SELECT id, namespace, key, value, tags, metadata, created_at, updated_at,
                        accessed_at, access_count, user_id
                 FROM memories WHERE user_id = ?1 AND namespace LIKE ?2
                 ORDER BY access_count DESC LIMIT ?3 OFFSET ?4",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![user_id, pattern, limit, offset], |row| {
                row_to_memory(row)
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_tacit_memories_by_user(
        &self,
        user_id: &str,
        limit: i64,
    ) -> Result<Vec<Memory>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, namespace, key, value, tags, metadata, created_at, updated_at,
                        accessed_at, access_count, user_id
                 FROM memories WHERE namespace LIKE 'tacit/%' AND user_id = ?1
                 ORDER BY access_count DESC LIMIT ?2",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![user_id, limit], row_to_memory)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn count_memories(&self) -> Result<i64, NeboError> {
        let conn = self.conn()?;
        conn.query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn count_memories_by_namespace(&self, namespace_prefix: &str) -> Result<i64, NeboError> {
        let conn = self.conn()?;
        let pattern = format!("{namespace_prefix}%");
        conn.query_row(
            "SELECT COUNT(*) FROM memories WHERE namespace LIKE ?1",
            params![pattern],
            |row| row.get(0),
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn increment_memory_access(&self, id: i64) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE memories SET access_count = access_count + 1, accessed_at = CURRENT_TIMESTAMP WHERE id = ?1",
            params![id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn increment_memory_access_by_key(
        &self,
        namespace: &str,
        key: &str,
        user_id: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE memories SET access_count = access_count + 1, accessed_at = CURRENT_TIMESTAMP
             WHERE namespace = ?1 AND key = ?2 AND user_id = ?3",
            params![namespace, key, user_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn get_distinct_namespaces(&self) -> Result<Vec<String>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT DISTINCT namespace FROM memories ORDER BY namespace")
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }
}

fn row_to_memory(row: &rusqlite::Row) -> rusqlite::Result<Memory> {
    Ok(Memory {
        id: row.get("id")?,
        namespace: row.get("namespace")?,
        key: row.get("key")?,
        value: row.get("value")?,
        tags: row.get("tags")?,
        metadata: row.get("metadata")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        accessed_at: row.get("accessed_at")?,
        access_count: row.get("access_count")?,
        user_id: row.get("user_id")?,
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
