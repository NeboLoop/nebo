use rusqlite::params;
use rusqlite::types::Value;

use super::fts::{Terms, merge_ranked};

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
    /// (memory_id, rank): any of the words, found anywhere in the memory
    /// (`fts::Terms`). An agent-scoped search also surfaces owner-level
    /// facts; writes remain exact-scope.
    pub fn search_memories_fts(
        &self,
        query: &str,
        user_ids: &[String],
        limit: i64,
    ) -> Result<Vec<(i64, f64)>, NeboError> {
        self.search_index(&MEMORIES, query, user_ids, limit)
    }

    /// FTS5 search on memory_chunks table. Returns (chunk_id, rank): any of
    /// the words, found anywhere in the chunk.
    pub fn search_chunks_fts(
        &self,
        query: &str,
        user_id: &str,
        limit: i64,
    ) -> Result<Vec<(i64, f64)>, NeboError> {
        self.search_index(&CHUNKS, query, &[user_id.to_string()], limit)
    }

    /// The any-word search of one memory index in `scopes`: the indexed
    /// words through its FTS5 table (BM25), the short ones with `LIKE`, one
    /// list best first.
    fn search_index(&self, index: &Index, query: &str, scopes: &[String], limit: i64) -> Result<Vec<(i64, f64)>, NeboError> {
        let terms = Terms::parse(query);
        if scopes.is_empty() || terms.is_empty() {
            return Ok(Vec::new());
        }
        let scope_values = || scopes.iter().map(|s| Value::Text(s.clone()));
        let conn = self.conn()?;
        let indexed = match terms.match_expr(" OR ") {
            Some(expr) => {
                let values = [Value::Text(expr), Value::Integer(limit)].into_iter().chain(scope_values());
                ranked(&conn, &fts_sql(index, scopes.len()), values)?
            }
            None => Vec::new(),
        };
        let patterns = terms.any_word_patterns();
        let short = if patterns.is_empty() {
            Vec::new()
        } else {
            let values = std::iter::once(Value::Integer(limit))
                .chain(patterns.iter().cloned().map(Value::Text))
                .chain(scope_values());
            ranked(&conn, &short_sql(index, patterns.len(), scopes.len()), values)?
        };
        Ok(merge_ranked(indexed, short, limit))
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

/// A memory index the any-word search reads: its FTS5 table over `table`,
/// and the three columns it indexes.
struct Index {
    fts: &'static str,
    table: &'static str,
    columns: [&'static str; 3],
}

const MEMORIES: Index = Index { fts: "memories_fts", table: "memories", columns: ["key", "value", "tags"] };
const CHUNKS: Index = Index { fts: "memory_chunks_fts", table: "memory_chunks", columns: ["text", "source", "path"] };

/// `?first..` for `n` values.
fn placeholders(first: usize, n: usize) -> String {
    (first..first + n).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ")
}

// The FTS searches join the index to its rows to keep one scope's hits.
// They are CROSS JOINs so the match drives: given a plain JOIN, SQLite
// walked the scope's rows by its user_id index and ran the whole MATCH once
// per row (`SCAN fts VIRTUAL TABLE INDEX 32:=M3` inside the loop) — 20 s for
// a long prompt over 20k chunks, on the turn's recall (2026-10-09). With the
// match outermost it runs once, in rank order.

/// Ranked hits of the match `?1` in the scopes `?3..` (one per scope), `?2` at most.
fn fts_sql(index: &Index, scopes: usize) -> String {
    let (fts, table) = (index.fts, index.table);
    format!(
        "SELECT m.id, fts.rank
         FROM {fts} fts
         CROSS JOIN {table} m ON m.id = fts.rowid
         WHERE {fts} MATCH ?1
           AND m.user_id IN ({})
         ORDER BY fts.rank
         LIMIT ?2",
        placeholders(3, scopes)
    )
}

/// Rows holding any of the `LIKE` patterns `?2..` in the scopes after
/// them, `?1` at most, ranked minus how many patterns each holds (so they
/// order with BM25's negative ranks: more is better).
fn short_sql(index: &Index, patterns: usize, scopes: usize) -> String {
    let held = (0..patterns)
        .map(|i| {
            let any = index.columns.map(|c| format!("IFNULL(m.{c}, '') LIKE ?{}", i + 2)).join(" OR ");
            format!("({any})")
        })
        .collect::<Vec<_>>()
        .join(" + ");
    format!(
        "SELECT id, rank FROM (
             SELECT m.id, -({held}) AS rank FROM {table} m WHERE m.user_id IN ({scopes})
         ) WHERE rank < 0
         ORDER BY rank
         LIMIT ?1",
        table = index.table,
        scopes = placeholders(2 + patterns, scopes)
    )
}

/// The (id, rank) rows of `sql` with `values`.
fn ranked(conn: &rusqlite::Connection, sql: &str, values: impl IntoIterator<Item = Value>) -> Result<Vec<(i64, f64)>, NeboError> {
    let mut stmt = conn.prepare(sql).map_err(|e| NeboError::Database(e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(values), |row| Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?)))
        .map_err(|e| NeboError::Database(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| NeboError::Database(e.to_string()))
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

    /// A memory or chunk is found by any word inside it, in Japanese,
    /// Korean, Chinese and Thai as in English, two-character words
    /// included, and only in the scopes asked for; a memory holding more of
    /// the words ranks first.
    #[test]
    fn memory_search_finds_a_word_inside_a_sentence_in_any_language() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("t.db").to_str().unwrap()).unwrap();
        let facts = [
            ("ja", "毎週月曜日に会議があります"),
            ("ko", "회의는 매주 화요일입니다"),
            ("zh", "预算由财务部负责"),
            ("th", "ประชุมทุกวันพุธตอนเช้า"),
            ("en", "Dana owns the quarterly report"),
        ];
        for (key, value) in facts {
            store.upsert_memory("tacit", key, value, None, None, "owner").unwrap();
        }
        store.upsert_memory("tacit", "other", "会議 회의 预算", None, None, "someone-else").unwrap();
        let owner = ["owner".to_string()];
        let keys = |q: &str| {
            store
                .search_memories_fts(q, &owner, 10)
                .unwrap()
                .into_iter()
                .map(|(id, _)| store.get_memory(id).unwrap().unwrap().key)
                .collect::<Vec<_>>()
        };
        for (q, key) in [("会議", "ja"), ("회의", "ko"), ("预算", "zh"), ("财务部", "zh"), ("ประชุม", "th"), ("quarterly", "en")] {
            assert_eq!(keys(q), [key], "{q}");
        }
        // Any word: both memories, the one with more of the words first.
        let mut both = keys("会議 회의");
        both.sort();
        assert_eq!(both, ["ja", "ko"]);
        assert_eq!(keys("会議 毎週 月曜日").first().map(String::as_str), Some("ja"));
        assert!(keys("invoice").is_empty());
        // A prompt's "a", "of", "to" are no reason to recall every memory.
        assert_eq!(keys("is it up to a report of Dana"), ["en"]);

        let chunk = store.insert_memory_chunk(None, 0, "来週の会議は東京で行います", "session", "", 0, 13, "m", "owner").unwrap();
        assert_eq!(store.search_chunks_fts("会議", "owner", 5).unwrap().iter().map(|(id, _)| *id).collect::<Vec<_>>(), [chunk]);
        assert_eq!(store.search_chunks_fts("東京", "owner", 5).unwrap().len(), 1);
        assert!(store.search_chunks_fts("会議", "someone-else", 5).unwrap().is_empty());
    }

    /// The match drives each memory search: the FTS index is the outer loop,
    /// read once in rank order, and each hit seeks its row. The other way
    /// round runs the match once per row of the scope.
    #[test]
    fn memory_searches_run_the_match_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("t.db").to_str().unwrap()).unwrap();
        let conn = store.conn().unwrap();
        for sql in [super::fts_sql(&super::MEMORIES, 2), super::fts_sql(&super::CHUNKS, 1)] {
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
