use rusqlite::params;

use crate::Store;
use types::NeboError;

impl Store {
    /// Get a cached embedding by content hash and model.
    pub fn get_cached_embedding(
        &self,
        content_hash: &str,
        model: &str,
    ) -> Result<Option<Vec<u8>>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT embedding FROM embedding_cache WHERE content_hash = ?1 AND model = ?2",
            params![content_hash, model],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Insert a cached embedding.
    pub fn insert_cached_embedding(
        &self,
        content_hash: &str,
        embedding: &[u8],
        model: &str,
        dimensions: i64,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO embedding_cache (content_hash, embedding, model, dimensions, created_at)
             VALUES (?1, ?2, ?3, ?4, CURRENT_TIMESTAMP)",
            params![content_hash, embedding, model, dimensions],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Insert a memory chunk and return its ID.
    pub fn insert_memory_chunk(
        &self,
        memory_id: Option<i64>,
        chunk_index: i64,
        text: &str,
        source: &str,
        path: &str,
        start_char: i64,
        end_char: i64,
        model: &str,
        user_id: &str,
    ) -> Result<i64, NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO memory_chunks (memory_id, chunk_index, text, source, path, start_char, end_char, model, user_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, CURRENT_TIMESTAMP)",
            params![memory_id, chunk_index, text, source, path, start_char, end_char, model, user_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(conn.last_insert_rowid())
    }

    /// Insert a memory embedding for a chunk.
    pub fn insert_memory_embedding(
        &self,
        chunk_id: i64,
        model: &str,
        dimensions: i64,
        embedding: &[u8],
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO memory_embeddings (chunk_id, model, dimensions, embedding, created_at)
             VALUES (?1, ?2, ?3, ?4, CURRENT_TIMESTAMP)",
            params![chunk_id, model, dimensions, embedding],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Delete every chunk for a memory (embeddings follow via the
    /// `memory_embeddings.chunk_id ON DELETE CASCADE` FK; the FTS index follows
    /// via the `memory_chunks_ad` trigger). Used by the embedding backfill to
    /// clear orphaned unembedded chunks before re-chunking.
    pub fn delete_chunks_for_memory(&self, memory_id: i64) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM memory_chunks WHERE memory_id = ?1",
            params![memory_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Distinct memory-scope user ids with at least one stored embedding under
    /// `model` — the boot index prewarm builds one ANN index per entry.
    pub fn list_embedding_user_ids(&self, model: &str) -> Result<Vec<String>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT mc.user_id
                 FROM memory_embeddings me
                 CROSS JOIN memory_chunks mc ON mc.id = me.chunk_id
                 WHERE me.model = ?1",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![model], |row| row.get(0))
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Get all embeddings for a user and model.
    /// Returns (chunk_id, embedding_blob) pairs.
    pub fn get_all_embeddings_by_user(
        &self,
        user_id: &str,
        model: &str,
    ) -> Result<Vec<(i64, Vec<u8>)>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT me.chunk_id, me.embedding
                 FROM memory_embeddings me
                 CROSS JOIN memory_chunks mc ON mc.id = me.chunk_id
                 WHERE mc.user_id = ?1 AND me.model = ?2",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![user_id, model], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// All memory-level embeddings for ONE exact user scope under `model`:
    /// `(memory_id, memory key, embedding blob)` rows. Exact `user_id` match —
    /// never a scope chain — so the write-time contradiction check can only
    /// ever compare within a single isolation scope (a `:ctx:` scope never
    /// sees a sibling's vectors). Transcript chunks (NULL memory_id) excluded.
    pub fn list_memory_embeddings_by_user(
        &self,
        user_id: &str,
        model: &str,
    ) -> Result<Vec<(i64, String, Vec<u8>)>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT mc.memory_id, m.key, me.embedding
                 FROM memory_embeddings me
                 CROSS JOIN memory_chunks mc ON mc.id = me.chunk_id
                 JOIN memories m ON m.id = mc.memory_id
                 WHERE mc.user_id = ?1 AND me.model = ?2",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![user_id, model], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// FTS5 search on memories table across a READ scope chain (the exact
    /// scope plus its ancestors, from `memory::memory_scope_chain`). Returns
    /// (memory_id, rank). An agent-scoped search also surfaces owner-level
    /// facts; writes remain exact-scope.
    pub fn search_memories_fts(
        &self,
        query: &str,
        user_ids: &[String],
        limit: i64,
    ) -> Result<Vec<(i64, f64)>, NeboError> {
        if user_ids.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn()?;
        // FTS5 match query — escape special chars
        let fts_query = sanitize_fts_query(query);
        let sql = memories_fts_sql(user_ids.len());
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let mut params_vec: Vec<&dyn rusqlite::ToSql> = vec![&fts_query, &limit];
        for uid in user_ids {
            params_vec.push(uid);
        }
        let rows = stmt
            .query_map(params_vec.as_slice(), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// FTS5 search on memory_chunks table. Returns (chunk_id, rank).
    pub fn search_chunks_fts(
        &self,
        query: &str,
        user_id: &str,
        limit: i64,
    ) -> Result<Vec<(i64, f64)>, NeboError> {
        let conn = self.conn()?;
        let fts_query = sanitize_fts_query(query);
        let mut stmt = conn
            .prepare(CHUNKS_FTS_SQL)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![fts_query, user_id, limit], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Get a memory chunk's text and source by chunk ID.
    pub fn get_memory_chunk(
        &self,
        chunk_id: i64,
    ) -> Result<Option<(i64, Option<i64>, String, Option<String>)>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, memory_id, text, source FROM memory_chunks WHERE id = ?1",
            params![chunk_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }
}

// The FTS searches join the index to its rows to keep one scope's hits.
// They are CROSS JOINs so the match drives: given a plain JOIN, SQLite
// walked the scope's rows by its user_id index and ran the whole MATCH once
// per row (`SCAN fts VIRTUAL TABLE INDEX 32:=M3` inside the loop) — 20 s for
// a long prompt over 20k chunks, on the turn's recall (2026-10-09). With the
// match outermost it runs once, in rank order.

/// Ranked memory hits of `?1` in the scopes `?3..` (one per scope), `?2` at most.
fn memories_fts_sql(scopes: usize) -> String {
    let placeholders = (0..scopes)
        .map(|i| format!("?{}", i + 3))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "SELECT m.id, fts.rank
         FROM memories_fts fts
         CROSS JOIN memories m ON m.id = fts.rowid
         WHERE memories_fts MATCH ?1
           AND m.user_id IN ({placeholders})
         ORDER BY fts.rank
         LIMIT ?2"
    )
}

/// Ranked chunk hits of `?1` in the scope `?2`, `?3` at most.
const CHUNKS_FTS_SQL: &str = "SELECT mc.id, fts.rank
     FROM memory_chunks_fts fts
     CROSS JOIN memory_chunks mc ON mc.id = fts.rowid
     WHERE memory_chunks_fts MATCH ?1
       AND mc.user_id = ?2
     ORDER BY fts.rank
     LIMIT ?3";

/// Sanitize a query string for FTS5 MATCH: escape double quotes, strip operators.
fn sanitize_fts_query(query: &str) -> String {
    // For simple queries, wrap each word in quotes to avoid FTS syntax issues
    query
        .split_whitespace()
        .map(|word| {
            let clean: String = word
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            if clean.is_empty() {
                String::new()
            } else {
                format!("\"{}\"", clean)
            }
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" OR ")
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

#[cfg(test)]
mod tests {
    use crate::Store;

    /// The match drives each memory search: the FTS index is the outer loop,
    /// read once in rank order, and each hit seeks its row. The other way
    /// round runs the match once per row of the scope.
    #[test]
    fn memory_searches_run_the_match_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("t.db").to_str().unwrap()).unwrap();
        let conn = store.conn().unwrap();
        for sql in [super::memories_fts_sql(2), super::CHUNKS_FTS_SQL.to_string()] {
            let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
            let unbound = rusqlite::params_from_iter(vec![rusqlite::types::Null; stmt.parameter_count()]);
            let plan: Vec<String> = stmt
                .query_map(unbound, |row| row.get::<_, String>(3))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            assert!(plan[0].starts_with("SCAN fts VIRTUAL TABLE"), "{sql}: {plan:#?}");
            assert!(plan[1].starts_with("SEARCH m"), "{sql}: {plan:#?}");
        }
    }
}
