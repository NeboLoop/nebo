# Embedding System SME

Subject matter expert reference for Nebo's embedding and vector search subsystem.
Covers embedding generation, vector storage, hybrid search, caching, chunking,
and cross-system integration with the memory pipeline and transcript indexer.

---

## Architecture Overview

```
+------------------------------------------------------------------+
|                        ENTRY POINTS                               |
|                                                                   |
|   memory::store_facts()        transcript::index_compacted_messages()
|   (after fact extraction)       (after sliding-window compaction)  |
+----------+-------------------------+------------------------------+
           |                         |
           v                         v
+------------------------------------------------------------------+
|                     CHUNKING LAYER                                |
|                                                                   |
|   chunking::chunk_text_default()                                  |
|   - Sentence-boundary splitting                                   |
|   - 1600 char chunks, 320 char overlap                           |
|   - Returns Vec<TextChunk> with char offsets                      |
+----------+-------------------------+------------------------------+
           |                         |
           v                         v
+------------------------------------------------------------------+
|                   EMBEDDING PROVIDER                              |
|                                                                   |
|   CachedEmbeddingProvider                                         |
|     +-> SHA256 content hash                                       |
|     +-> embedding_cache table (dedup layer)                       |
|     +-> inner: OpenAIEmbeddingProvider | OllamaEmbeddingProvider  |
|                                                                   |
|   OpenAI:  text-embedding-3-small  (1536 dims)                   |
|   Ollama:  nomic-embed-text        (768 dims)                    |
+----------+-------------------------+------------------------------+
           |                         |
           v                         v
+------------------------------------------------------------------+
|                   STORAGE LAYER (SQLite)                          |
|                                                                   |
|   memory_chunks      memory_embeddings     embedding_cache        |
|   +- id              +- id                 +- content_hash (PK)   |
|   +- memory_id?      +- chunk_id (FK)      +- embedding (BLOB)    |
|   +- chunk_index     +- model              +- model               |
|   +- text            +- dimensions         +- dimensions           |
|   +- source          +- embedding (BLOB)   +- created_at          |
|   +- path            +- created_at                                |
|   +- start_char                                                   |
|   +- end_char        memory_chunks_fts     memories_fts           |
|   +- model           (FTS5 virtual table)  (FTS5 virtual table)   |
|   +- user_id         +- text               +- key                 |
|   +- created_at      +- source             +- value               |
|                      +- path               +- tags                |
+------------------------------------------------------------------+
           |
           v
+------------------------------------------------------------------+
|                     RETRIEVAL LAYER                                |
|                                                                   |
|   search::hybrid_search()                                         |
|     1. FTS5 on memories_fts      (text weight)                   |
|     2. FTS5 on memory_chunks_fts (text weight * dampening)       |
|     3. Vector cosine similarity   (vector weight)                 |
|     4. Adaptive weighting by query classification                 |
|     5. Score merging + dedup + sort + truncate                    |
|                                                                   |
|   search_adapter::HybridSearchAdapter                             |
|     Bridges agent::search -> tools::HybridSearcher trait          |
|     Injected into bot_tool for "search" action                    |
+------------------------------------------------------------------+
```

---

## Embedding Providers

### Trait Definition

```
File: crates/ai/src/embedding.rs
```

```rust
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    fn id(&self) -> &str;
    fn dimensions(&self) -> usize;
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, ProviderError>;
}
```

The trait is object-safe (`dyn EmbeddingProvider`), enabling runtime provider
selection. The `embed` method accepts a batch of texts and returns one vector per
text, supporting efficient batch embedding in a single API call.

### OpenAI Provider

```rust
pub struct OpenAIEmbeddingProvider {
    api_key: String,
    model: String,       // default: "text-embedding-3-small"
    base_url: String,    // default: "https://api.openai.com/v1"
    dims: usize,         // default: 1536
    http_client: reqwest::Client,
}
```

- **Model:** `text-embedding-3-small` (1536 dimensions)
- **API endpoint:** `POST {base_url}/embeddings`
- **Request body:** `{ input: Vec<String>, model: String }`
- **Response parsing:** `data[].embedding` (Vec<f32>)
- **Retry policy:** Exponential backoff at [500ms, 2000ms, 8000ms] (3 attempts)
- **Auth errors:** HTTP 401/403 are surfaced immediately as `ProviderError::Auth`
- **Customization:** `with_base_url()` for OpenAI-compatible endpoints,
  `with_model()` for alternative models and dimension counts

### Ollama Provider

```rust
pub struct OllamaEmbeddingProvider {
    base_url: String,    // e.g. "http://localhost:11434"
    model: String,       // default: "nomic-embed-text"
    dims: usize,         // default: 768
    http_client: reqwest::Client,
}
```

- **Model:** `nomic-embed-text` (768 dimensions)
- **API endpoint:** `POST {base_url}/api/embed`
- **Request body:** `{ model: String, input: Vec<String> }`
- **Response parsing:** `embeddings` (Vec<Vec<f32>>)
- **Retry policy:** Same exponential backoff [500ms, 2000ms, 8000ms]
- **No auth required:** Ollama runs locally

### Cached Provider (Decorator)

```rust
pub struct CachedEmbeddingProvider {
    inner: Box<dyn EmbeddingProvider>,
    store: Arc<db::Store>,
}
```

Wraps any `EmbeddingProvider` with SHA256 content-hash deduplication. This is
the provider that is always instantiated in practice -- the raw providers are
never used directly.

**Cache lookup flow:**

```
For each text in batch:
  1. hash = SHA256(text)  ->  64-char hex string
  2. SELECT embedding FROM embedding_cache
     WHERE content_hash = hash AND model = provider.id()
  3. Cache hit  -> deserialize BLOB to Vec<f32>, skip embedding
  4. Cache miss -> add to uncached batch

Embed uncached texts in single API call

For each newly embedded text:
  5. INSERT OR REPLACE INTO embedding_cache
     (content_hash, embedding, model, dimensions)
  6. Populate results array at correct index
```

This means identical text is never re-embedded regardless of which memory or
chunk it belongs to. The cache is keyed on (content_hash, model), so switching
embedding providers invalidates the cache naturally.

### Provider Selection at Startup

```
File: crates/server/src/lib.rs  (build_embedding_provider)
```

```rust
fn build_embedding_provider(
    store: &Arc<db::Store>,
) -> Option<Arc<dyn ai::EmbeddingProvider>>
```

Iterates active `auth_profiles` in database order:
1. **First active `openai` profile found** -> `OpenAIEmbeddingProvider` wrapped
   in `CachedEmbeddingProvider`
2. **First active `ollama` profile found** -> `OllamaEmbeddingProvider` (nomic-embed-text)
   wrapped in `CachedEmbeddingProvider`
3. **No matching profile** -> `None` (embedding disabled, text-only search)

The provider is injected into the `Runner` via `set_embedding_provider()` and
propagated to the `HybridSearchAdapter`. The system degrades gracefully: when
no embedding provider is available, hybrid search falls back to FTS5-only.

---

## Vector Storage Schema

### Database Tables (SQLite)

All tables defined in migration `0016_vector_embeddings.sql`, updated by
`0019_memories_user_scope.sql` and `0038_memory_chunks_schema_update.sql`.

#### memory_chunks

Stores the raw text chunks. Each chunk belongs to either a parent `Memory`
record (for extracted facts) or stands alone (for session transcript chunks).

```sql
CREATE TABLE memory_chunks (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    memory_id   INTEGER REFERENCES memories(id) ON DELETE CASCADE,  -- nullable
    chunk_index INTEGER NOT NULL,
    text        TEXT NOT NULL,
    source      TEXT DEFAULT 'memory',   -- 'memory' | 'session'
    path        TEXT DEFAULT '',          -- session_id for transcripts
    start_char  INTEGER DEFAULT 0,
    end_char    INTEGER DEFAULT 0,
    model       TEXT DEFAULT '',          -- embedding model used
    user_id     TEXT NOT NULL DEFAULT '',
    created_at  DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Indexes
idx_memory_chunks_memory_id  ON (memory_id)
idx_memory_chunks_model      ON (model)
idx_memory_chunks_source     ON (source)
idx_memory_chunks_user_id    ON (user_id)
```

**Two source types:**
- `source = "memory"`, `memory_id = Some(id)` -- chunk of a stored memory fact
- `source = "session"`, `memory_id = None`, `path = session_id` -- transcript chunk

#### memory_chunks_fts (FTS5)

Full-text search virtual table for chunks, kept in sync by triggers.

```sql
CREATE VIRTUAL TABLE memory_chunks_fts USING fts5(
    text, source, path,
    content='memory_chunks',
    content_rowid='id'
);
```

Three triggers maintain sync: `memory_chunks_ai` (after insert),
`memory_chunks_ad` (after delete), `memory_chunks_au` (after update).

#### memory_embeddings

Stores the actual float vectors as BLOBs. One-to-one with `memory_chunks`.

```sql
CREATE TABLE memory_embeddings (
    id          INTEGER PRIMARY KEY,
    chunk_id    INTEGER REFERENCES memory_chunks(id) ON DELETE CASCADE,
    model       TEXT NOT NULL,
    dimensions  INTEGER NOT NULL,
    embedding   BLOB NOT NULL,           -- little-endian f32 array
    created_at  DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Indexes
idx_memory_embeddings_chunk_id ON (chunk_id)
idx_memory_embeddings_model    ON (model)
```

#### embedding_cache

Content-addressable cache for deduplication. Keyed on SHA256 hash of the input
text combined with model identifier.

```sql
CREATE TABLE embedding_cache (
    content_hash TEXT PRIMARY KEY,       -- SHA256 hex (64 chars)
    embedding    BLOB NOT NULL,          -- little-endian f32 array
    model        TEXT NOT NULL,
    dimensions   INTEGER NOT NULL,
    created_at   DATETIME DEFAULT CURRENT_TIMESTAMP
);
```

#### memories_fts (FTS5)

Full-text search on the top-level `memories` table (defined in `0013_agent_tools.sql`).

```sql
CREATE VIRTUAL TABLE memories_fts USING fts5(
    key, value, tags,
    content='memories',
    content_rowid='id'
);
```

Also trigger-synced (insert, delete, update).

### Binary Format

Embeddings are stored as little-endian `f32` byte arrays:

```rust
// crates/ai/src/embedding.rs

pub fn f32_to_bytes(vec: &[f32]) -> Vec<u8> {
    vec.iter().flat_map(|f| f.to_le_bytes()).collect()
}

pub fn bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes.chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}
```

- **OpenAI (1536 dims):** 1536 * 4 = 6,144 bytes per embedding
- **Ollama (768 dims):** 768 * 4 = 3,072 bytes per embedding

---

## Text Chunking

```
File: crates/agent/src/chunking.rs
```

### Constants

| Constant           | Value | Purpose                                    |
|--------------------|-------|--------------------------------------------|
| DEFAULT_CHUNK_SIZE | 1600  | Max characters per chunk                   |
| DEFAULT_OVERLAP    | 320   | Overlap between consecutive chunks          |
| SHORT_CIRCUIT_SIZE | 1920  | Texts shorter than this become a single chunk |

### Algorithm

```
chunk_text(text, chunk_size, overlap):
  if len(text) <= 1920:
    return [text as single chunk]

  boundaries = find_sentence_boundaries(text)
    - Paragraph breaks (\n\n)
    - Sentence-ending punctuation (. ! ?) followed by space/newline
    - End of text

  start = 0
  while start < len(text):
    target_end = min(start + chunk_size, len(text))
    end = last boundary <= target_end and > start
    emit chunk(text[start..end])
    next_start = first boundary >= (end - overlap) and < end
    start = next_start
```

```rust
pub struct TextChunk {
    pub text: String,
    pub start_char: usize,
    pub end_char: usize,
}
```

The overlap ensures that sentence fragments at chunk boundaries are present in
adjacent chunks, improving retrieval when a query matches content that spans a
chunk boundary.

---

## Embedding Pipeline: Memory Facts

When `memory::store_facts()` stores extracted facts, it optionally triggers
background embedding for vector search.

```
File: crates/agent/src/memory.rs
```

### Flow

```
store_facts(store, facts, user_id, embedding_provider)
  |
  +-> format_for_storage(facts) -> Vec<MemoryEntry>
  |
  +-> For each entry:
  |     secret_scan::contains_secret() -> skip if credential detected
  |     sanitize::detect_prompt_injection() -> skip if injection detected
  |     store.upsert_memory(namespace, key, value, tags, metadata, user_id)
  |     collect into stored_entries
  |
  +-> If embedding_provider is Some AND stored_entries is non-empty:
        embed_memories_async(store, provider, stored_entries, user_id)
```

### embed_memories_async

```rust
pub fn embed_memories_async(
    store: Arc<Store>,
    embedding_provider: Arc<dyn EmbeddingProvider>,
    entries: Vec<MemoryEntry>,
    user_id: String,
)
```

Spawns a `tokio::spawn` fire-and-forget task:

```
For each MemoryEntry:
  1. Look up the Memory record by (namespace, key, user_id)
  2. Build text = "{key}: {value}"
  3. chunk_text_default(text) -> Vec<TextChunk>
  4. Collect chunk texts into batch
  5. embedding_provider.embed(batch) -> Vec<Vec<f32>>
  6. For each (chunk, embedding):
     a. store.insert_memory_chunk(memory_id, index, text, "memory", ...)
     b. f32_to_bytes(embedding)
     c. store.insert_memory_embedding(chunk_id, model, dims, blob)
```

All errors are logged at `debug` level but do not propagate to the caller.

---

## Embedding Pipeline: Session Transcripts

After sliding-window compaction evicts messages from a session, those messages
are indexed for cross-session semantic search.

```
File: crates/agent/src/transcript.rs
```

### Flow

```
index_compacted_messages(store, embedding_provider, session_id, user_id)
  |
  +-> Read high-water mark: session.last_embedded_message_id
  |
  +-> Load all messages for session, filter to:
  |     - id > last_embedded_message_id
  |     - role = "user" or "assistant"
  |     - content is non-empty
  |
  +-> Group into blocks of BLOCK_SIZE (5) messages
  |
  +-> For each block:
  |     1. Concatenate: "{role}: {content[..500]}\n" per message
  |     2. chunk_text_default(block_text) -> Vec<TextChunk>
  |     3. Batch embed chunks
  |     4. For each (chunk, embedding):
  |          insert_memory_chunk(None, index, text, "session", session_id, ...)
  |          insert_memory_embedding(chunk_id, model, dims, blob)
  |     5. Track highest message ID in block
  |
  +-> Update session.last_embedded_message_id = highest_id
```

### Trigger Point in Runner

```
File: crates/agent/src/runner.rs  (line ~1672)
```

Called during the run loop after sliding-window compaction:

```rust
if let Some(ep) = embedding_provider {
    let store_c = store.clone();
    let ep_c = ep.clone();
    let sid = session_id.to_string();
    let uid = memory_user_id.clone();
    let handle = tokio::spawn(async move {
        transcript::index_compacted_messages(&store_c, ep_c.as_ref(), &sid, &uid).await;
    });
    crate::memory_flush::track_extraction(handle).await;
}
```

The `memory_flush::track_extraction` call ensures the background task is tracked
for graceful shutdown coordination.

---

## Hybrid Search

```
File: crates/agent/src/search.rs
```

### Entry Point

```rust
pub async fn hybrid_search(
    store: &Arc<Store>,
    embedding_provider: Option<&dyn EmbeddingProvider>,
    query: &str,
    user_id: &str,
    config: &SearchConfig,
) -> Vec<SearchResult>
```

### SearchConfig

```rust
pub struct SearchConfig {
    pub limit: usize,    // default: 20
    pub min_score: f64,  // default: 0.3
}
```

### SearchResult

```rust
pub struct SearchResult {
    pub memory_id: Option<i64>,
    pub chunk_id: Option<i64>,
    pub key: String,
    pub value: String,
    pub namespace: String,
    pub score: f64,
    pub source: String,  // "fts_memory" | "fts_chunk" | "vector"
}
```

### Three-Phase Search

```
Phase 1: FTS5 on memories table
  +-> store.search_memories_fts(query, user_id, limit * 3)
  +-> normalize_bm25(rank) * text_weight
  +-> Merge into HashMap<merge_key, SearchResult>
  +-> merge_key = "mem:{memory_id}"

Phase 2: FTS5 on memory_chunks table
  +-> store.search_chunks_fts(query, user_id, limit * 3)
  +-> normalize_bm25(rank) * text_weight * dampening
  +-> dampening = 0.6 for source="session", 1.0 otherwise
  +-> merge_key = "mem:{memory_id}" or "chunk:{chunk_id}"
  +-> Additive score merging with Phase 1

Phase 3: Vector cosine similarity (if embedding_provider available)
  +-> Embed query: provider.embed([query])
  +-> Load all embeddings: store.get_all_embeddings_by_user(user_id, model)
  +-> For each stored embedding:
  |     similarity = cosine_similarity(query_vec, stored_vec)
  |     if similarity < min_score: skip
  |     vector_score = similarity * vector_weight
  +-> Additive score merging with Phases 1 & 2

Final:
  +-> Filter results where score >= min_score
  +-> Sort by score DESC
  +-> Truncate to config.limit
```

### Adaptive Weighting

Query classification determines the balance between vector and text search:

```
classify_query(query) -> QueryClass:
  word_count <= 3 AND has proper nouns  -> ShortProperNoun
  word_count <= 3                       -> ShortGeneric
  word_count <= 8                       -> Medium
  word_count > 8                        -> Long

adaptive_weights(class) -> (vector_weight, text_weight):
  ShortProperNoun  -> (0.35, 0.65)   // Names: FTS excels
  ShortGeneric     -> (0.45, 0.55)   // Short queries: balanced
  Medium           -> (0.70, 0.30)   // Typical queries: favor vectors
  Long             -> (0.80, 0.20)   // Semantic queries: strong vector bias
```

Weights always sum to 1.0. The intuition: short queries with proper nouns
benefit from exact text matching (BM25), while longer semantic queries benefit
from vector similarity.

### BM25 Normalization

```rust
pub fn normalize_bm25(rank: f64) -> f64 {
    1.0 / (1.0 + rank.abs())
}
```

SQLite FTS5 BM25 ranks are negative (more negative = better match). This
function maps them to (0, 1] where higher is better. A perfect match (rank = 0)
yields 1.0; a rank of -5.0 yields approximately 0.167.

### Cosine Similarity

```rust
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    // Returns 0.0 if lengths differ or empty
    // Computes: (a . b) / (||a|| * ||b||)
    // Clamped to [-1.0, 1.0]
    // Uses f64 accumulators for numerical stability
}
```

The computation promotes f32 values to f64 for dot product and norm accumulation,
preventing catastrophic cancellation with high-dimensional vectors. The result
is clamped to the valid cosine similarity range.

### Session Chunk Dampening

Session transcript chunks receive a 0.6x dampening factor in FTS scoring. This
prevents old conversational fragments from outranking explicitly stored memory
facts. The dampening only applies to the text search component; vector scores
are not dampened (similarity is already semantic).

---

## Cross-System Integration

### Runner Integration

```
File: crates/agent/src/runner.rs
```

The `Runner` struct holds `embedding_provider: Option<Arc<dyn EmbeddingProvider>>`.
It is used in three places:

1. **Memory extraction** (line ~3458): After debounced fact extraction, the
   embedding provider is passed to `memory::store_facts()` so newly stored
   memories are embedded in the background.

2. **Transcript indexing** (line ~1672): After sliding-window compaction, evicted
   messages are indexed via `transcript::index_compacted_messages()`.

3. **Forked command runs** (line ~616): The embedding provider is propagated to
   forked sub-agent runs for their own memory/transcript operations.

### HybridSearchAdapter

```
File: crates/agent/src/search_adapter.rs
```

Bridges the `agent::search` module to the `tools::HybridSearcher` trait,
avoiding circular crate dependencies:

```
+----------------+          +---------------------+
|  tools crate   |          |   agent crate       |
|                |          |                     |
| trait          |  <----   | HybridSearchAdapter |
| HybridSearcher |  impl    |   .store            |
|                |          |   .embedding_provider|
+----------------+          |                     |
                            | search::hybrid_search()|
                            +---------------------+
```

```rust
pub struct HybridSearchAdapter {
    store: Arc<Store>,
    embedding_provider: Option<Arc<dyn ai::EmbeddingProvider>>,
}

impl HybridSearcher for HybridSearchAdapter {
    fn search(&self, query, user_id, limit) -> Pin<Box<dyn Future<...>>> {
        // Delegates to search::hybrid_search()
        // Maps SearchResult -> HybridSearchResult (drops internal fields)
    }
}
```

### Bot Tool (AgentTool) Integration

```
File: crates/tools/src/bot_tool.rs
```

The `AgentTool` (domain tool "agent") has three memory actions:

- **store:** Upserts a memory fact directly (no embedding triggered here --
  embedding happens in the debounced extraction path)
- **recall:** Exact key lookup with fallback chain
- **search:** Delegates to `HybridSearcher` (FTS5 + vector) when available

```rust
"search" => {
    if let Some(ref searcher) = self.hybrid_searcher {
        let results = searcher.search(query, &ctx.user_id, limit).await;
        // Format results with scores
    }
}
```

---

## DB Query Functions

```
File: crates/db/src/queries/embeddings.rs
```

| Function | Signature | Purpose |
|----------|-----------|---------|
| `get_cached_embedding` | `(&self, content_hash: &str, model: &str) -> Result<Option<Vec<u8>>>` | Cache lookup by SHA256 hash + model |
| `insert_cached_embedding` | `(&self, content_hash: &str, embedding: &[u8], model: &str, dimensions: i64)` | INSERT OR REPLACE into cache |
| `insert_memory_chunk` | `(&self, memory_id: Option<i64>, chunk_index: i64, text: &str, source: &str, path: &str, start_char: i64, end_char: i64, model: &str, user_id: &str) -> Result<i64>` | Insert chunk, return ID |
| `insert_memory_embedding` | `(&self, chunk_id: i64, model: &str, dimensions: i64, embedding: &[u8])` | Insert vector BLOB for a chunk |
| `get_all_embeddings_by_user` | `(&self, user_id: &str, model: &str) -> Result<Vec<(i64, Vec<u8>)>>` | All (chunk_id, blob) pairs for a user |
| `search_memories_fts` | `(&self, query: &str, user_id: &str, limit: i64) -> Result<Vec<(i64, f64)>>` | BM25 search on memories table |
| `search_chunks_fts` | `(&self, query: &str, user_id: &str, limit: i64) -> Result<Vec<(i64, f64)>>` | BM25 search on memory_chunks table |
| `get_memory_chunk` | `(&self, chunk_id: i64) -> Result<Option<(i64, Option<i64>, String, Option<String>)>>` | (id, memory_id, text, source) by chunk ID |

### FTS Query Sanitization

```rust
fn sanitize_fts_query(query: &str) -> String {
    // Split on whitespace
    // Filter each word to alphanumeric + hyphen + underscore
    // Wrap each word in double quotes
    // Join with " OR "
    // Example: "John Smith" -> "\"John\" OR \"Smith\""
}
```

This prevents FTS5 syntax errors from user input containing operators, brackets,
or special characters.

---

## Error Handling

### Provider Errors

Both embedding providers use a three-attempt retry with exponential backoff:

```
Attempt 0: immediate
Attempt 1: sleep 500ms
Attempt 2: sleep 2000ms
Attempt 3: sleep 8000ms (final, then fail)
```

Error types:
- `ProviderError::Auth` -- HTTP 401/403, immediate failure (no retry)
- `ProviderError::Request` -- network errors, non-2xx status after retries

### Graceful Degradation

The entire embedding subsystem is optional. When `embedding_provider` is `None`:
- `hybrid_search()` skips Phase 3 (vector search), using FTS5 only
- `store_facts()` skips background embedding
- Transcript indexing is skipped entirely
- The system remains fully functional with text-only search

Background embedding failures (in `embed_memories_async` and
`index_compacted_messages`) are logged at `debug`/`warn` level and do not affect
the foreground chat interaction. Each individual chunk failure is handled
independently -- a single embedding failure does not abort the batch.

### Cache Errors

Cache lookup failures in `CachedEmbeddingProvider` fall through to the inner
provider (the text is treated as uncached). Cache write failures are silently
ignored (the embedding still reaches the caller).

---

## Performance Considerations

### Caching Layer

The `embedding_cache` table provides content-addressable deduplication:
- **Key:** SHA256(text content) + model identifier
- **Effect:** Identical text is never re-embedded, even across different memories
- **Persistence:** Cache survives application restarts (SQLite)
- **Invalidation:** Automatic when switching models (different model = different key)

### Batch Processing

Both providers accept multiple texts in a single API call:
- `OpenAIEmbeddingProvider::embed(&[String])` -- single HTTP request for N texts
- `OllamaEmbeddingProvider::embed(&[String])` -- single HTTP request for N texts

The `CachedEmbeddingProvider` partitions the batch into cached and uncached
subsets, only sending uncached texts to the inner provider. This minimizes
API calls.

### Vector Search: Full Scan

**Current implementation:** `get_all_embeddings_by_user()` loads ALL embeddings
for a user into memory, then computes cosine similarity against each one.

```
For a user with N embedded chunks:
  - Memory: N * (4 * dims + overhead) bytes
  - CPU: N * dims multiply-accumulate operations
  - I/O: One SQLite query loading N BLOBs

Example (1000 chunks, OpenAI 1536 dims):
  - ~6 MB memory for vectors
  - ~1.5M floating-point operations
```

This is a brute-force approach that works well for the expected scale of a
personal companion (hundreds to low thousands of memory chunks per user). For
significantly larger scales, this would need to be replaced with an approximate
nearest-neighbor index (e.g., HNSW, IVF).

### Chunking Overhead

The 20% overlap (320 chars out of 1600) means approximately 20% more chunks and
embeddings are generated than a non-overlapping strategy. This is a deliberate
tradeoff for better retrieval quality at chunk boundaries.

### Background Processing

Both embedding pipelines (memory facts and transcript indexing) run as `tokio::spawn`
background tasks, ensuring that embedding latency does not block the chat
response stream. The `memory_flush::track_extraction` mechanism coordinates
these tasks for graceful shutdown.

### Memory Debouncing

Memory extraction is debounced per-session (via `MemoryDebouncer`) to avoid
redundant extraction when messages arrive in rapid succession. Embedding only
happens after extraction completes, so the debounce indirectly reduces
embedding API calls.

---

## Data Model Relationships

```
+-------------------+        +------------------+
|     memories      |        |     sessions     |
| (extracted facts) |        | (chat sessions)  |
+-------------------+        +------------------+
         |                            |
         | 1:N (memory_id)           | (session_id stored in path)
         v                            v
+-------------------+        +------------------+
|  memory_chunks    |        |  memory_chunks   |
| source="memory"   |        | source="session" |
| memory_id=<id>    |        | memory_id=NULL   |
+-------------------+        +------------------+
         |                            |
         | 1:1 (chunk_id)            | 1:1 (chunk_id)
         v                            v
+--------------------+       +--------------------+
| memory_embeddings  |       | memory_embeddings  |
| (vector BLOBs)     |       | (vector BLOBs)     |
+--------------------+       +--------------------+

+--------------------+
|  embedding_cache   |
| (content-hash      |
|  deduplication)    |
+--------------------+
```

---

## Configuration Reference

| Setting | Source | Default | Notes |
|---------|--------|---------|-------|
| Embedding provider | `auth_profiles` table | OpenAI preferred, Ollama fallback | Selected at server startup |
| OpenAI model | Hardcoded | `text-embedding-3-small` | `with_model()` available but unused |
| OpenAI dimensions | Hardcoded | 1536 | Tied to model |
| Ollama model | Hardcoded | `nomic-embed-text` | Selected when Ollama profile active |
| Ollama dimensions | Hardcoded | 768 | Tied to model |
| Chunk size | `chunking::DEFAULT_CHUNK_SIZE` | 1600 chars | |
| Chunk overlap | `chunking::DEFAULT_OVERLAP` | 320 chars | |
| Short-circuit size | `chunking::SHORT_CIRCUIT_SIZE` | 1920 chars | Texts below this are not chunked |
| Transcript block size | `transcript::BLOCK_SIZE` | 5 messages | Messages grouped before chunking |
| Search result limit | `SearchConfig::limit` | 20 | Max results returned |
| Min search score | `SearchConfig::min_score` | 0.3 | Below this, results are filtered out |
| Retry delays | Hardcoded | [500, 2000, 8000] ms | Exponential backoff |
| Session dampening | Hardcoded | 0.6 | Reduces FTS weight for transcript chunks |
| Message content cap | `transcript.rs` | 500 chars/message | Truncation before indexing |

---

## Key Files

| File | Purpose |
|------|---------|
| `crates/ai/src/embedding.rs` | `EmbeddingProvider` trait, OpenAI/Ollama/Cached providers, f32/bytes conversion |
| `crates/ai/src/lib.rs` | Public re-exports of embedding types |
| `crates/agent/src/chunking.rs` | Sentence-boundary text chunking |
| `crates/agent/src/memory.rs` | Fact extraction, storage, `embed_memories_async()` |
| `crates/agent/src/transcript.rs` | Session transcript indexing with high-water mark |
| `crates/agent/src/search.rs` | Hybrid search (FTS5 + vector), cosine similarity, adaptive weighting |
| `crates/agent/src/search_adapter.rs` | `HybridSearchAdapter` bridging agent/tools crates |
| `crates/agent/src/runner.rs` | Runner holds embedding provider, triggers embedding in run loop |
| `crates/db/src/queries/embeddings.rs` | All embedding/chunk/FTS DB queries |
| `crates/db/src/models.rs` | `Memory`, `MemoryChunk`, `MemoryEmbedding`, `EmbeddingCache` structs |
| `crates/db/migrations/0013_agent_tools.sql` | `memories` table and `memories_fts` FTS5 |
| `crates/db/migrations/0016_vector_embeddings.sql` | `memory_chunks`, `memory_embeddings`, `embedding_cache` |
| `crates/db/migrations/0019_memories_user_scope.sql` | `user_id` column on memories and chunks |
| `crates/db/migrations/0038_memory_chunks_schema_update.sql` | Rename start_line/end_line, make memory_id nullable |
| `crates/tools/src/bot_tool.rs` | `HybridSearcher` trait, `AgentTool` search action |
| `crates/server/src/lib.rs` | `build_embedding_provider()` startup initialization |

---

## Test Coverage

### Unit Tests

| Module | Test | What It Verifies |
|--------|------|------------------|
| `embedding.rs` | `test_f32_roundtrip` | f32 -> bytes -> f32 lossless |
| `embedding.rs` | `test_content_hash` | SHA256 determinism and length |
| `search.rs` | `test_cosine_similarity_identical` | cos(a, a) = 1.0 |
| `search.rs` | `test_cosine_similarity_orthogonal` | cos(perpendicular) = 0.0 |
| `search.rs` | `test_cosine_similarity_opposite` | cos(a, -a) = -1.0 |
| `search.rs` | `test_cosine_similarity_empty` | Empty vectors return 0.0 |
| `search.rs` | `test_cosine_similarity_mismatched_len` | Mismatched dims return 0.0 |
| `search.rs` | `test_normalize_bm25` | BM25 rank -> score mapping |
| `search.rs` | `test_classify_*` | Query classification by length and content |
| `search.rs` | `test_adaptive_weights_sum_to_one` | Weight pairs always sum to 1.0 |
| `chunking.rs` | `test_short_text_single_chunk` | Short text not split |
| `chunking.rs` | `test_long_text_multiple_chunks` | Long text produces multiple chunks |
| `chunking.rs` | `test_chunk_boundaries_are_sentence_aligned` | Chunks end at sentence punctuation |
| `chunking.rs` | `test_chunk_offsets` | start_char <= end_char <= text.len() |
| `transcript.rs` | `test_block_size` | BLOCK_SIZE constant is 5 |
| `transcript.rs` | `test_message_text_truncation` | Content capped at 500 chars |
| `transcript.rs` | `test_block_grouping` | Messages grouped correctly |
