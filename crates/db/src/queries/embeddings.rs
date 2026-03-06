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
                 JOIN memory_chunks mc ON mc.id = me.chunk_id
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

    /// FTS5 search on memories table. Returns (memory_id, rank).
    pub fn search_memories_fts(
        &self,
        query: &str,
        user_id: &str,
        limit: i64,
    ) -> Result<Vec<(i64, f64)>, NeboError> {
        let conn = self.conn()?;
        // FTS5 match query — escape special chars
        let fts_query = sanitize_fts_query(query);
        let mut stmt = conn
            .prepare(
                "SELECT m.id, fts.rank
                 FROM memories_fts fts
                 JOIN memories m ON m.id = fts.rowid
                 WHERE memories_fts MATCH ?1
                   AND m.user_id = ?2
                 ORDER BY fts.rank
                 LIMIT ?3",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![fts_query, user_id, limit], |row| {
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
            .prepare(
                "SELECT mc.id, fts.rank
                 FROM memory_chunks_fts fts
                 JOIN memory_chunks mc ON mc.id = fts.rowid
                 WHERE memory_chunks_fts MATCH ?1
                   AND mc.user_id = ?2
                 ORDER BY fts.rank
                 LIMIT ?3",
            )
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
    pub fn get_memory_chunk(&self, chunk_id: i64) -> Result<Option<(i64, Option<i64>, String, Option<String>)>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, memory_id, text, source FROM memory_chunks WHERE id = ?1",
            params![chunk_id],
            |row| Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            )),
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }
}

/// Sanitize a query string for FTS5 MATCH: escape double quotes, strip operators.
fn sanitize_fts_query(query: &str) -> String {
    // For simple queries, wrap each word in quotes to avoid FTS syntax issues
    query
        .split_whitespace()
        .map(|word| {
            let clean: String = word.chars().filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_').collect();
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
