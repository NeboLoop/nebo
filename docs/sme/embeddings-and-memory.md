# Embeddings & Memory System -- Deep Dive

Source code locations:
- Embeddings: `/Users/almatuck/workspaces/nebo/nebo/internal/agent/embeddings/`
- Memory: `/Users/almatuck/workspaces/nebo/nebo/internal/agent/memory/`

---

## Part 1: Embeddings System

Five files comprise the embeddings subsystem:
- `service.go` -- Core service, caching, cosine similarity, provider interface
- `providers.go` -- OpenAI and Ollama provider implementations
- `chunker.go` -- Text splitting with sentence-boundary chunking
- `hybrid.go` -- Hybrid search combining FTS5 + vector similarity
- `chunker_test.go` -- Tests for the chunker

---

### 1.1 Provider Interface (`service.go`)

```go
type Provider interface {
    Embed(ctx context.Context, texts []string) ([][]float32, error)
    Dimensions() int
    Model() string
}
```

This is the abstraction over embedding backends. Two implementations exist: OpenAI and Ollama.

---

### 1.2 Embedding Providers (`providers.go`)

#### OpenAI Provider

```go
type OpenAIProvider struct {
    apiKey     string
    model      string
    dimensions int
    baseURL    string
    client     *http.Client
}

type OpenAIConfig struct {
    APIKey     string
    Model      string // default: "text-embedding-3-small"
    Dimensions int    // default: 1536
    BaseURL    string // default: "https://api.openai.com/v1"
}
```

- **Default model:** `text-embedding-3-small`
- **Default dimensions:** 1536
- **Default base URL:** `https://api.openai.com/v1`
- **HTTP timeout:** 60 seconds
- **API endpoint:** `POST {baseURL}/embeddings`
- **Request body:** `{"input": texts, "model": model, "dimensions": dimensions}`
- **Auth header:** `Authorization: Bearer {apiKey}`
- **Batch support:** Native -- sends all texts in a single request. The response contains `data[].embedding` and `data[].index`. Results are reordered by index to match input order.

#### Ollama Provider

```go
type OllamaProvider struct {
    baseURL    string
    model      string
    dimensions int
    client     *http.Client
}

type OllamaConfig struct {
    BaseURL    string // default: "http://localhost:11434"
    Model      string // default: "qwen3-embedding"
    Dimensions int    // default: 256 (qwen3-embedding supports 32-1024)
}
```

- **Default model:** `qwen3-embedding`
- **Default dimensions:** 256
- **Default base URL:** `http://localhost:11434`
- **HTTP timeout:** 120 seconds (2x OpenAI, since Ollama is local and may need to load models)
- **API endpoint:** `POST {baseURL}/api/embed` (the newer batch endpoint, not the legacy `/api/embeddings`)
- **Request body:** `{"model": model, "input": texts, "dimensions": dimensions}` -- dimensions only included if > 0
- **Batch support:** Native -- sends all texts in a single request. Response contains `embeddings[]` array.
- **Validation:** Checks that `len(result.Embeddings) == len(texts)`, returns error on mismatch.

---

### 1.3 Embedding Service (`service.go`)

```go
type Service struct {
    queries  *db.Queries
    sqlDB    *sql.DB
    provider Provider
    mu       sync.RWMutex
}

type Config struct {
    DB       *sql.DB
    Provider Provider
}
```

#### Construction: `NewService(cfg Config) (*Service, error)`

- Requires a non-nil `*sql.DB` (returns error otherwise)
- Provider can be nil at construction time (set later via `SetProvider`)
- **On startup:** evicts stale cache entries older than 30 days via `CleanOldEmbeddingCache` (sqlc-generated query). Uses `sql.NullTime` with cutoff = `time.Now().AddDate(0, 0, -30)`.

#### Thread Safety

The provider is protected by `sync.RWMutex`:
- `SetProvider(p Provider)` -- write lock
- `HasProvider() bool` -- read lock
- `Embed(...)` -- read lock to copy provider reference, then releases lock before calling provider
- `Dimensions() int` -- read lock
- `Model() string` -- read lock

#### Embedding with Cache: `Embed(ctx context.Context, texts []string) ([][]float32, error)`

Algorithm:
1. Read-lock to get provider reference, return error if nil
2. Return nil for empty input
3. For each text, compute SHA256 hash and check cache (`getCached`)
4. Collect uncached texts and their indices
5. If there are uncached texts, call provider with retry logic (up to 3 attempts)
6. Store newly generated embeddings in cache
7. Return all results (cached + newly generated) in original order

**Retry logic:**
- Up to 3 attempts
- Exponential backoff: 500ms, 2s, 8s (formula: `1<<(attempt*2) * 500ms`)
- Does NOT retry on client errors: checks error string for "401", "403", "400", "Unauthorized", "invalid_api_key", "Bad Request"
- Respects context cancellation during backoff

#### Cache Operations

```go
func (s *Service) getCached(ctx context.Context, contentHash, model string) ([]float32, bool)
func (s *Service) setCached(ctx context.Context, contentHash, model string, dimensions int, embedding []float32)
```

- Cache key: `(content_hash, model)` pair -- same text with different models gets separate cache entries
- Embedding storage format: JSON-encoded `[]float32` as byte blob (via `floatsToBlob` / `blobToFloats`)
- Cache table: `embedding_cache` (sqlc-managed via `GetEmbeddingCacheParams` and `UpsertEmbeddingCacheParams`)
- Upsert semantics: if same hash+model exists, it gets overwritten

#### Convenience Methods

```go
func (s *Service) EmbedOne(ctx context.Context, text string) ([]float32, error)
```
Wraps `Embed` for a single text. Returns error if result slice is empty.

```go
func (s *Service) GetDB() *sql.DB
```
Exposes the raw DB connection for use by FTS queries in `HybridSearcher`.

#### Hashing

```go
func hashText(text string) string
```
SHA256 of raw text bytes, hex-encoded. This is the cache key.

#### Vector Serialization

```go
func floatsToBlob(floats []float32) []byte   // json.Marshal
func blobToFloats(blob []byte) ([]float32, error) // json.Unmarshal
```

Vectors are stored as JSON arrays in SQLite blobs. This is human-readable and portable but not the most space-efficient. A 1536-dim vector stored this way is roughly 10-15KB per embedding.

#### Cosine Similarity

```go
func CosineSimilarity(a, b []float32) float64
```

Standard cosine similarity:
```
similarity = dot(a, b) / (||a|| * ||b||)
```

- Returns 0 if vectors have different lengths or either has zero length
- Returns 0 if either vector has zero norm
- Computed in float64 precision (each float32 is promoted)
- Range: [-1, 1] for normalized vectors (in practice, embedding vectors are typically non-negative, so range is [0, 1])

#### Utility

```go
func containsAny(s string, subs ...string) bool
```
Used by retry logic to detect non-retryable errors.

---

### 1.4 Text Chunking (`chunker.go`)

#### Chunk Struct

```go
type Chunk struct {
    Text      string
    StartChar int  // byte offset in original text
    EndChar   int  // byte offset in original text
    Index     int  // sequential chunk index
}
```

#### Constants

```go
const (
    defaultMaxChars     = 1600  // ~400 tokens (4 chars/token heuristic)
    defaultOverlapChars = 320   // ~80 tokens overlap
)
```

#### Configuration via Options Pattern

```go
type ChunkOption func(*chunkConfig)

type chunkConfig struct {
    maxChars     int
    overlapChars int
}

func WithMaxChars(n int) ChunkOption
func WithOverlapChars(n int) ChunkOption
```

#### Main Function: `SplitText(text string, opts ...ChunkOption) []Chunk`

Algorithm:
1. Apply options over defaults (maxChars=1600, overlapChars=320)
2. Trim whitespace; return nil for empty text
3. **Short text check:** if `utf8.RuneCountInString(text) < maxChars + overlapChars` (i.e., < 1920 runes by default), return single chunk. This avoids splitting text that would barely produce two chunks.
4. Split text into sentences via `splitSentences()`
5. If only 0 or 1 sentences, return single chunk
6. Iterate through sentences, accumulating into chunks:
   - Add sentences to current chunk until adding the next sentence would exceed `maxChars` AND the buffer is non-empty
   - When a chunk is full, emit it
   - **Overlap rewind:** Walk backwards from current sentence position, accumulating character lengths, until `overlapChars` worth of text is covered. Set the next chunk's start position to this rewind point.
   - The overlap rewind skips the very first sentence of the current chunk (starts from `startSent+1`) to guarantee forward progress.

#### Sentence Splitting: `splitSentences(text string) []sentence`

```go
type sentence struct {
    text  string
    start int  // byte offset in original text
}
```

Two boundary types:
1. **Double newline:** `\n\n` -- always a sentence boundary. The double newline is included with the preceding sentence.
2. **Sentence-ending punctuation:** `.`, `!`, `?` followed by a space, newline, or tab. The punctuation AND the following whitespace are included with the preceding sentence.

Any remaining text after the last boundary becomes the final sentence.

Key behaviors:
- Delimiters are preserved with the sentence they terminate (not the next sentence)
- Single newlines are NOT boundaries
- Abbreviations like "Dr. Smith" would incorrectly split (no special handling)
- The algorithm operates on bytes, not runes (fine for ASCII punctuation)

#### Test Coverage (`chunker_test.go`)

- `TestSplitText_ShortText` -- Verifies short text stays as single chunk with correct char offsets
- `TestSplitText_Empty` -- Empty string and whitespace-only return 0 chunks
- `TestSplitText_LongText` -- 50 sentences (~4000 chars) produces multiple chunks; verifies: first chunk starts at 0, last chunk ends at len(text), chunks are in order, overlap exists between consecutive chunks (next start < previous end), no chunk exceeds 2x maxChars
- `TestSplitText_CustomOptions` -- `WithMaxChars(200), WithOverlapChars(50)` forces splitting of 20 sentences
- `TestSplitSentences` -- Verifies period+space splitting and double-newline paragraph splitting

---

### 1.5 Hybrid Search (`hybrid.go`)

#### Structs

```go
type HybridSearcher struct {
    db       *sql.DB
    embedder *Service
}

type HybridSearchConfig struct {
    DB       *sql.DB
    Embedder *Service
}

type SearchResult struct {
    ID          int64   `json:"id"`
    Key         string  `json:"key"`
    Value       string  `json:"value"`
    Namespace   string  `json:"namespace"`
    Score       float64 `json:"score"`
    VectorScore float64 `json:"vector_score,omitempty"`
    TextScore   float64 `json:"text_score,omitempty"`
    Source      string  `json:"source,omitempty"`
    // Citation fields (populated only by vector search matches)
    ChunkText   string  `json:"chunk_text,omitempty"`
    StartChar   int     `json:"start_char,omitempty"`
    EndChar     int     `json:"end_char,omitempty"`
    CreatedAt   string  `json:"created_at,omitempty"`
}

type SearchOptions struct {
    Namespace    string
    Limit        int
    VectorWeight float64 // default: 0.7
    TextWeight   float64 // default: 0.3
    MinScore     float64 // default: 0.3
    UserID       string
}
```

#### Default Search Options

```go
func DefaultSearchOptions() SearchOptions {
    return SearchOptions{
        Namespace:    "default",
        Limit:        10,
        VectorWeight: 0.7,
        TextWeight:   0.3,
        UserID:       "",
    }
}
```

#### Main Search: `Search(ctx, query, opts) ([]SearchResult, error)`

Algorithm:
1. Default limit to 10 if <= 0
2. If both weights are 0, use `adaptiveWeights(query)` to auto-determine
3. Default MinScore to 0.3
4. **Over-fetch:** candidates = `limit * 8` -- fetches 8x the requested limit from each search backend because in-Go cosine similarity is microsecond-scale and more candidates produce better ranking
5. **FTS search** (`searchFTS`): query the `memories_fts` FTS5 table. On failure, fall back to `searchLike` (plain `LIKE` search).
6. **Session chunk FTS** (`searchChunksFTS`): search `memory_chunks_fts` for session transcript chunks. Non-fatal on failure. Results are appended to FTS results.
7. **Vector search** (`searchVector`): only if `embedder != nil && embedder.HasProvider()`. Non-fatal on failure -- degrades gracefully to FTS-only.
8. **Merge** results with weighted scoring
9. **Filter** by MinScore (default 0.3)
10. **Limit** to requested count

#### Adaptive Weights: `adaptiveWeights(query string) (vectorWeight, textWeight float64)`

Heuristic based on query word count and proper noun density:

| Condition | Vector Weight | Text Weight | Rationale |
|-----------|---------------|-------------|-----------|
| 0 words | 0.70 | 0.30 | Default |
| 1-3 words, >30% proper nouns | 0.35 | 0.65 | Short specific queries (names) favor exact match |
| 1-3 words, other | 0.45 | 0.55 | Short queries slightly favor FTS |
| 4-5 words | 0.70 | 0.30 | Medium queries favor semantic |
| 6+ words | 0.80 | 0.20 | Long conceptual queries strongly favor semantic |

Proper noun detection: any word (except the first) starting with an uppercase ASCII letter.

#### FTS Search: `searchFTS(query, namespace, userID string, limit int) ([]SearchResult, error)`

SQL query:
```sql
SELECT m.id, m.key, m.value, m.namespace, bm25(memories_fts) as rank
FROM memories m
JOIN memories_fts f ON m.id = f.rowid
WHERE memories_fts MATCH ? AND m.namespace LIKE ? || '%' AND m.user_id = ?
ORDER BY rank
LIMIT ?
```

- Namespace filtering uses prefix match (`LIKE 'namespace%'`)
- User-scoped via `user_id`
- BM25 rank is converted to a 0-1 score via `bm25RankToScore`
- Source tagged as `"fts"`

#### FTS Query Building: `buildFTSQuery(raw string) string`

Tokenization:
1. Split on whitespace
2. Strip non-alphanumeric characters (keep `[a-zA-Z0-9_]`)
3. Quote each token: `"token"`
4. Join with ` AND `

Example: `"hello world 123"` becomes `"hello" AND "world" AND "123"`

Returns empty string for empty input or all-punctuation input.

#### BM25 Score Normalization: `bm25RankToScore(rank float64) float64`

BM25 ranks from SQLite FTS5 are negative (more negative = more relevant):
- If `rank >= 0`: `1 / (1 + rank)` -- gives 1.0 for rank 0, decaying towards 0
- If `rank < 0`: `1 / (1 - rank)` -- for rank=-5, gives `1/6 = 0.167`; for rank=-0.5, gives `1/1.5 = 0.667`

This produces a (0, 1] range where higher is better.

#### LIKE Fallback: `searchLike(query, namespace, userID string, limit int) ([]SearchResult, error)`

Used when FTS fails:
```sql
SELECT id, key, value, namespace
FROM memories
WHERE namespace LIKE ? || '%' AND user_id = ? AND (key LIKE ? OR value LIKE ?)
LIMIT ?
```
- Pattern: `%{query}%`
- All LIKE matches get a fixed TextScore of 0.5
- Source tagged as `"like"`

#### Session Chunk FTS: `searchChunksFTS(query, namespace, userID string, limit int) ([]SearchResult, error)`

Searches session transcript chunks stored in a separate FTS5 table:
```sql
SELECT c.path, c.text, bm25(memory_chunks_fts) as rank
FROM memory_chunks_fts
JOIN memory_chunks c ON c.rowid = memory_chunks_fts.rowid
WHERE memory_chunks_fts MATCH ?
AND c.source = 'session'
AND c.namespace = ?
AND c.user_id = ?
ORDER BY rank
LIMIT ?
```

- **Session boost factor:** 0.6 -- dampens session chunk scores because they are less precise than extracted memories
- Score formula: `-rank * 0.6` (BM25 rank is negative, so `-rank` makes it positive, then multiplied by the dampening factor)
- Key format: `"session:{path}"`
- Source tagged as `"fts_session"`

#### Vector Search: `searchVector(ctx, query, namespace, userID string, limit int) ([]SearchResult, error)`

Algorithm:
1. Generate query embedding via `embedder.EmbedOne(ctx, query)`
2. Get current model identifier for filtering embeddings by model
3. Query all embeddings for this user + namespace:

```sql
SELECT c.id, COALESCE(m.key, 'session:' || c.path), c.text,
       COALESCE(m.namespace, c.source), e.embedding, COALESCE(c.memory_id, 0),
       COALESCE(c.start_char, 0), COALESCE(c.end_char, 0),
       COALESCE(m.created_at, c.created_at)
FROM memory_embeddings e
JOIN memory_chunks c ON e.chunk_id = c.id
LEFT JOIN memories m ON c.memory_id = m.id
WHERE (m.namespace LIKE ? || '%' OR c.source = 'session')
  AND c.user_id = ? AND e.model = ?
```

4. For each row: deserialize embedding blob, compute cosine similarity with query vector
5. **Deduplication:** Keep only the best-scoring chunk per `memory_id` (prevents the same memory from appearing multiple times via different chunks)
6. Sort by cosine similarity descending
7. Limit to requested count

The LEFT JOIN on `memories` ensures session chunks (where `memory_id IS NULL`) are also included.

Citation metadata (ChunkText, StartChar, EndChar, CreatedAt) is populated from the chunk and memory rows.

#### Result Merging: `mergeResults(ftsResults, vectorResults []SearchResult, vectorWeight, textWeight float64) []SearchResult`

Algorithm:
1. Create a map keyed by `"{namespace}:{key}"`
2. Process FTS results: add to map, setting TextScore
3. Process vector results: if key exists, merge VectorScore and citation metadata; otherwise add new entry
4. Calculate combined score for each: `Score = vectorWeight * VectorScore + textWeight * TextScore`
5. Sort by combined score descending

When a result appears in both FTS and vector results, it gets a boosted combined score. Results appearing in only one source get scored with one weight zeroed out.

Citation preservation: when merging, the vector result's ChunkText, StartChar, EndChar, and CreatedAt overwrite the FTS result's fields.

---

### 1.6 Database Schema (Inferred from Queries)

Based on the SQL queries in the code, the following tables are used:

**`memories`** -- Core memory storage
- `id` INTEGER PRIMARY KEY
- `key` TEXT
- `value` TEXT
- `namespace` TEXT
- `user_id` TEXT
- `tags` TEXT (JSON array)
- `access_count` INTEGER
- `accessed_at` TIMESTAMP
- `metadata` TEXT (JSON object, contains `confidence`, `reinforced_count`, etc.)
- `created_at` TIMESTAMP
- `updated_at` TIMESTAMP
- UNIQUE constraint on `(namespace, key, user_id)`

**`memories_fts`** -- FTS5 virtual table on memories
- `rowid` references `memories.id`
- Indexed columns include key and value (inferred from MATCH usage)

**`memory_chunks`** -- Chunked text segments
- `id` INTEGER PRIMARY KEY
- `memory_id` INTEGER (nullable -- NULL for session chunks)
- `text` TEXT
- `path` TEXT (for session chunks)
- `source` TEXT (`'session'` or other)
- `namespace` TEXT
- `user_id` TEXT
- `start_char` INTEGER
- `end_char` INTEGER
- `created_at` TIMESTAMP

**`memory_chunks_fts`** -- FTS5 virtual table on memory_chunks
- `rowid` references `memory_chunks.id`

**`memory_embeddings`** -- Vector storage
- `id` INTEGER PRIMARY KEY
- `chunk_id` INTEGER references `memory_chunks.id`
- `embedding` BLOB (JSON-encoded float32 array)
- `model` TEXT (e.g., "text-embedding-3-small")

**`embedding_cache`** -- Embedding computation cache
- `content_hash` TEXT (SHA256 hex)
- `model` TEXT
- `embedding` BLOB (JSON-encoded float32 array)
- `dimensions` INTEGER
- Stale entries (>30 days) cleaned on startup

---

## Part 2: Memory System

Five files comprise the memory subsystem:
- `extraction.go` -- Fact extraction from conversations via LLM
- `extraction_test.go` -- Test for key normalization
- `dbcontext.go` -- Database context builder for system prompt
- `files.go` -- File-based memory loading (SOUL.md, MEMORY.md, etc.)
- `personality.go` -- Personality directive synthesis from style observations

---

### 2.1 Fact Extraction (`extraction.go`)

#### Extracted Fact Categories

```go
type ExtractedFacts struct {
    Preferences []Fact `json:"preferences"`   // User preferences and behaviors
    Entities    []Fact `json:"entities"`       // People, places, things mentioned
    Decisions   []Fact `json:"decisions"`      // Decisions made during conversation
    Styles      []Fact `json:"styles"`         // Communication/personality style observations
    Artifacts   []Fact `json:"artifacts"`      // Content the agent produced that user may reference
    TaskContext []Fact `json:"task_context"`   // Active task parameters (dates, quantities, budgets)
}
```

Six categories total. Each maps to a specific memory layer and namespace (see FormatForStorage below).

#### Fact Struct

```go
type Fact struct {
    Key        string   `json:"key"`
    Value      string   `json:"value"`
    Category   string   `json:"category"`
    Tags       []string `json:"tags"`
    Confidence float64  `json:"-"` // Set via UnmarshalJSON, not from LLM
}
```

Confidence is NOT returned by the LLM. It is derived from the `explicit` boolean field in the JSON:

| `explicit` value | Confidence | Meaning |
|------------------|------------|---------|
| `true` | 0.9 | User directly stated this fact |
| `false` | 0.6 | Inferred from context/behavior |
| not provided | 0.75 | Backward compatibility default |

Custom `UnmarshalJSON`:
- Uses an alias struct with `json.RawMessage` for the Value field to handle both string and non-string JSON values
- If Value is a JSON string, uses it directly
- If Value is anything else (number, object, array), converts the raw JSON to a trimmed string

#### Extraction Prompt

The `ExtractFactsPrompt` constant is a detailed prompt that instructs the LLM to:
- Analyze conversation and extract durable facts across 6 categories
- Use path-like key format: `"category/name"` (e.g., `"person/sarah"`, `"style/humor-dry"`, `"artifact/landing-page-hero-copy"`)
- Include an `explicit` boolean (true = user stated directly, false = inferred)
- Skip greetings, casual chat, temporary info, technical details in code, easily-lookupable info
- NEVER skip dates, quantities, budgets, deadlines (critical task parameters)
- Store artifacts VERBATIM, store task context with EXACT values
- Return ONLY valid JSON

#### Extractor

```go
type Extractor struct {
    provider ai.Provider
}

func NewExtractor(provider ai.Provider) *Extractor
```

#### Input Limits

```go
const (
    maxContentPerMessage = 500   // chars -- truncate individual messages
    maxConversationChars = 15000 // chars -- cap total prompt size (~4k tokens)
)
```

#### Extract Method: `Extract(ctx context.Context, messages []session.Message) (*ExtractedFacts, error)`

Algorithm:
1. Return empty facts for empty messages
2. Build conversation text:
   - Skip `"tool"` role messages entirely (no extractable facts)
   - Skip messages with empty content
   - Truncate each message to 500 chars, append "..."
   - Format as `[role]: content\n\n`
3. If total conversation text exceeds 15000 chars, keep only the TAIL (recent messages are more relevant for fact extraction)
4. Stream LLM response using the extraction prompt
5. Parse response:
   - Handle empty response (return empty facts)
   - Strip markdown code fences (```json ... ```)
   - Strip inline backticks
   - Find first `{` in response
   - Find matching closing `}` by counting brace depth
   - Extract the balanced JSON object
   - Additional cleanup: remove embedded backticks and code fence markers
   - Unmarshal into `ExtractedFacts`
   - If no JSON found or braces unbalanced, return empty facts (not an error)

#### Key Normalization: `NormalizeMemoryKey(key string) string`

Canonicalizes keys to prevent the LLM from creating duplicates:
1. Trim whitespace
2. Lowercase
3. Replace underscores with hyphens
4. Replace spaces with hyphens
5. Collapse repeated hyphens (`--` to `-`)
6. Collapse repeated slashes (`//` to `/`)
7. Trim leading/trailing hyphens and slashes

Examples from tests:
- `"Code_Style"` -> `"code-style"`
- `"preferences/code_style"` -> `"preferences/code-style"`
- `"PERSON/Sarah"` -> `"person/sarah"`
- `"artifact//landing-page"` -> `"artifact/landing-page"`
- `"-leading-trailing-"` -> `"leading-trailing"`

#### Memory Entry Mapping: `FormatForStorage() []MemoryEntry`

```go
type MemoryEntry struct {
    Layer      string
    Namespace  string
    Key        string
    Value      string
    Tags       []string
    IsStyle    bool    // Style observations use reinforcement tracking instead of overwrite
    Confidence float64 // Extraction confidence (0.0-1.0)
}
```

Category-to-storage mapping:

| Category | Layer | Namespace | Tags added | IsStyle |
|----------|-------|-----------|------------|---------|
| Preferences | `"tacit"` | `"preferences"` | `"preference"` | false |
| Entities | `"entity"` | `"default"` | `"entity"` | false |
| Decisions | `"daily"` | `"{today's date}"` (YYYY-MM-DD) | `"decision"` | false |
| Styles | `"tacit"` | `"personality"` | `"style"` | **true** |
| Artifacts | `"tacit"` | `"artifacts"` | `"artifact"` | false |
| TaskContext | `"daily"` | `"{today's date}"` (YYYY-MM-DD) | `"task_context"` | false |

All keys are normalized via `NormalizeMemoryKey`.

#### Utility Methods

```go
func (f *ExtractedFacts) IsEmpty() bool      // true if all 6 arrays are empty
func (f *ExtractedFacts) TotalCount() int     // sum of all 6 array lengths
```

---

### 2.2 File-Based Memory (`files.go`)

#### Loaded Files Struct

```go
type LoadedFiles struct {
    Agents    string // AGENTS.md -- Agent behavior instructions
    Memory    string // MEMORY.md -- Long-term facts and preferences
    Soul      string // SOUL.md -- Personality and identity
    Heartbeat string // HEARTBEAT.md -- Proactive tasks to check
}
```

#### Loading: `LoadMemoryFiles(workspaceDir string) LoadedFiles`

Search order for each file:
1. `{workspaceDir}/{filename}` -- workspace-specific override
2. `{dataDir}/{filename}` -- Nebo data directory (platform-specific, via `defaults.DataDir()`)

First found wins (workspace takes priority). Files are read via `os.ReadFile`, trimmed with `strings.TrimSpace`.

Files loaded: `AGENTS.md`, `MEMORY.md`, `SOUL.md`, `HEARTBEAT.md`

#### System Prompt Formatting: `FormatForSystemPrompt() string`

Order of injection into system prompt:
1. `# Personality (SOUL.md)` -- Soul/personality comes FIRST (defines who the agent is)
2. `# Agent Instructions (AGENTS.md)` -- Behavioral instructions
3. `# User Memory (MEMORY.md)` -- Long-term facts

HEARTBEAT.md is deliberately NOT included (used by heartbeat daemon separately).

Sections are joined with `\n\n---\n\n` separator.

#### Utility Methods

```go
func (f LoadedFiles) IsEmpty() bool       // true if Agents, Memory, and Soul are all empty (ignores Heartbeat)
func (f LoadedFiles) HasHeartbeat() bool   // true if Heartbeat is non-empty
```

---

### 2.3 Database Context Builder (`dbcontext.go`)

This is the primary system for assembling all memory and context into the agent's system prompt.

#### DBContext Struct

```go
type DBContext struct {
    // Agent identity
    AgentName         string
    PersonalityPrompt string
    VoiceStyle        string
    ResponseLength    string
    EmojiUsage        string
    Formality         string
    Proactivity       string
    AgentEmoji        string
    AgentCreature     string
    AgentVibe         string
    AgentRole         string
    AgentRules        string
    ToolNotes         string

    // User profile
    UserDisplayName  string
    UserLocation     string
    UserTimezone     string
    UserOccupation   string
    UserInterests    []string
    UserGoals        string
    UserContext       string
    UserCommStyle    string
    OnboardingNeeded bool

    // Memories
    TacitMemories        []DBMemoryItem
    PersonalityDirective string // Synthesized from style observations
}
```

#### DBMemoryItem

```go
type DBMemoryItem struct {
    Namespace   string
    Key         string
    Value       string
    Tags        []string
    accessCount int       // for decay scoring (unexported)
    accessedAt  time.Time // for decay scoring (unexported)
    confidence  float64   // for quality-weighted ranking (unexported)
}
```

#### Decay Scoring: `decayScore(accessCount int, accessedAt *time.Time) float64`

Formula:
```
score = access_count * 0.7 ^ (days_since_last_access / 30.0)
```

- Half-life is approximately 30 * ln(2) / ln(1/0.7) = ~58 days
- After 30 days of no access, score is multiplied by 0.7
- After 60 days, multiplied by 0.49
- After 90 days, multiplied by 0.343
- If `accessedAt` is nil or zero, returns raw `access_count` (no decay)

This ensures frequently-accessed-but-stale memories decay below recently-accessed ones.

#### Main Loader: `LoadContext(db *sql.DB, userID string) (*DBContext, error)`

Accepts a shared `*sql.DB` (does NOT close it). Uses a 5-second context timeout.

Loading steps (each is non-fatal -- logs warning and continues):
1. `loadAgentProfile` -- agent identity and personality
2. `loadUserProfile` -- user demographics and preferences
3. `loadTacitMemories` -- persistent learned facts
4. `GetDirective` -- synthesized personality directive

#### Agent Profile: `loadAgentProfile(ctx, db, result)`

Loads from `agent_profile` table (id=1):

```sql
SELECT name, personality_preset, custom_personality, voice_style,
       response_length, emoji_usage, formality, proactivity,
       emoji, creature, vibe, role, agent_rules, tool_notes
FROM agent_profile WHERE id = 1
```

Defaults if not found:
- Name: `"Nebo"`
- VoiceStyle: `"neutral"`
- ResponseLength: `"adaptive"`
- EmojiUsage: `"moderate"`
- Formality: `"adaptive"`
- Proactivity: `"moderate"`

Personality prompt resolution:
1. If `custom_personality` is set and non-empty, use it directly
2. Otherwise, look up `personality_preset` in `personality_presets` table
3. If preset lookup fails, use fallback: `"You are {name}, a helpful and friendly AI assistant."`

#### User Profile: `loadUserProfile(ctx, db, result, userID)`

If `userID` is provided:
```sql
SELECT display_name, location, timezone, occupation, interests,
       goals, context, communication_style, onboarding_completed
FROM user_profiles WHERE user_id = ?
```

If `userID` is empty (CLI backwards compatibility):
```sql
SELECT ... FROM user_profiles LIMIT 1
```

- `ErrNoRows` -> sets `OnboardingNeeded = true`
- Any other error -> sets `OnboardingNeeded = true` and returns error
- `interests` is stored as a JSON array string, parsed via `json.Unmarshal`
- `onboarding_completed` is an int64; 0 or NULL means onboarding needed

#### Tacit Memory Loading

**Budget Constants:**
```go
const (
    maxTacitMemories = 50 // Total memories injected into system prompt
    maxStyleMemories = 10 // Cap for tacit/personality entries
)
```

**Two-Pass Strategy:** `loadTacitMemories(ctx, db, result, userID)`

1. **Pass 1:** Load up to 10 personality/style memories from `tacit/personality` namespace
2. **Pass 2:** Fill remaining slots (50 - pass1_count) from all `tacit/*` namespaces EXCEPT `tacit/personality`

This prevents style observations from crowding out actionable memories (preferences, artifacts, project context).

**Memory Loading Algorithm:** `loadTacitSlice(ctx, db, result, userID, namespace, limit) (int, error)`

1. **Overfetch:** `limit * 3` (minimum 30 rows) -- fetches more than needed for re-ranking
2. **Confidence filter:** Only includes memories where:
   - `metadata` is NULL (legacy memories -- always included), OR
   - `confidence` key is not present in metadata JSON, OR
   - `confidence >= 0.80`

   This means:
   - Explicit facts (confidence 0.9) get into the system prompt immediately
   - Inferred facts (confidence 0.6) need at least 2 reinforcements to cross 0.80
   - They can still be found via hybrid search, just not injected into every prompt

3. **Re-ranking:** Sort by `confidence * decayScore(accessCount, accessedAt)` descending. This means high-confidence, recently-accessed memories outrank frequently-accessed but low-confidence or stale ones.

4. Take top N after re-ranking.

**Non-Personality Loading:** `loadTacitNonPersonality(ctx, db, result, userID, limit) (int, error)`

Same algorithm as `loadTacitSlice` but with namespace filter:
```sql
WHERE (namespace = 'tacit' OR namespace LIKE 'tacit/%') AND namespace != 'tacit/personality'
```

**Row Scanner:** `scanMemoryRow(rows *sql.Rows) (DBMemoryItem, error)`

Scans columns: namespace, key, value, tags, access_count, accessed_at, confidence.
- Default confidence is 1.0 (no metadata = full trust for legacy memories)
- Tags are JSON-parsed from NullString

#### System Prompt Assembly: `FormatForSystemPrompt() string`

Sections in order (joined with `\n\n---\n\n`):

1. **Agent Identity** -- Personality prompt with `{name}` placeholder replaced, OR default identity statement. The identity explicitly states "You are NOT Claude, ChatGPT, or any other AI brand".

2. **Character** (optional) -- Creature type, relationship to user, vibe, signature emoji. Only included if at least one is set.

3. **Personality (Learned)** (optional) -- The synthesized personality directive from style observations.

4. **Communication Style** -- Voice style, formality, emoji usage, response length. Formatted as a single line.

5. **User Information** (optional) -- Name, location, timezone, occupation, interests, goals, context, communication preference. Only included if at least one is set.

6. **Rules** (optional) -- User-defined agent rules. Supports structured JSON format (versioned sections with enabled/disabled items) or plain text fallback.

7. **Tool Notes** (optional) -- Environment-specific tool instructions. Same structured JSON or plain text format.

8. **What You Know** (optional) -- Tacit memories formatted as:
   ```
   ## What You Know

   These are facts you've learned and stored. Reference them naturally --
   don't announce that you're "recalling" them:
   - {namespace_suffix}/{key}: {value}
   ```
   The namespace prefix `tacit/` is stripped from display.

9. **Memory Tool Instructions** -- Always appended. Instructs the agent on how to use the memory tool:
   - `agent(resource: memory, action: recall, key: "...")`
   - `agent(resource: memory, action: search, query: "...")`
   - `agent(resource: memory, action: store, key: "...", value: "...", layer: "tacit")`

#### Structured Content Parsing: `formatStructuredContent(content, heading string) string`

Supports a versioned JSON format:
```json
{
    "version": 1,
    "sections": [
        {
            "name": "Section Name",
            "items": [
                {"text": "Rule text", "enabled": true},
                {"text": "Disabled rule", "enabled": false}
            ]
        }
    ]
}
```

Only sections with at least one enabled item are rendered. Disabled items within included sections are skipped. Falls back to raw text if JSON parsing fails (backwards compatibility).

#### Utility Methods

```go
func (c *DBContext) IsEmpty() bool           // true if no personality prompt and no user name
func (c *DBContext) NeedsOnboarding() bool   // delegates to OnboardingNeeded field
func stringOr(ns sql.NullString, def string) string  // NullString helper
```

---

### 2.4 Personality Tracking (`personality.go`)

This subsystem synthesizes a personality directive from accumulated style observations, creating an emergent personality that evolves over time.

#### Constants

```go
const PersonalityDirectiveKey = "directive"
const PersonalityDirectiveNamespace = "tacit/personality"
const MinStyleObservations = 5   // Minimum observations before synthesis
const DecayThresholdDays = 14    // Days a count==1 style survives without reinforcement
```

#### Style Observation

```go
type styleObservation struct {
    Key             string
    Value           string
    ReinforcedCount float64
    FirstObserved   time.Time
    LastReinforced  time.Time
}
```

#### Synthesis: `SynthesizeDirective(ctx, db, provider, userID) (string, error)`

Algorithm:
1. Validate db and provider are non-nil
2. Load all style observations from `tacit/personality` namespace where key matches `style/*`
3. If fewer than 5 observations, return empty (not enough signal yet)
4. Apply decay filter -- remove weak observations
5. Sort by reinforcement count descending (strongest signals first)
6. Cap at top 15 observations to keep synthesis prompt compact
7. Build synthesis prompt with observation lines formatted as `- {key}: {value} (observed {count} times)`
8. Stream LLM response
9. Store result as memory with namespace=`tacit/personality`, key=`directive`
10. Metadata includes `synthesized_at` timestamp and `observation_count`

The synthesis prompt instructs the LLM to:
- Write a single cohesive paragraph (3-5 sentences)
- Use second person ("You tend to...", "Keep responses...")
- Focus on strongest signals
- Weave traits into natural prose, not a list

Storage uses `INSERT ... ON CONFLICT(namespace, key, user_id) DO UPDATE` for upsert semantics.

#### Loading Style Observations: `loadStyleObservations(ctx, db, userID) ([]styleObservation, error)`

```sql
SELECT key, value, metadata, created_at
FROM memories
WHERE namespace = 'tacit/personality' AND key LIKE 'style/%' AND user_id = ?
```

Metadata JSON is parsed for:
- `reinforced_count` (float64) -- defaults to 1 if not present
- `first_observed` (RFC3339 timestamp) -- defaults to `created_at` if not present
- `last_reinforced` (RFC3339 timestamp) -- defaults to `created_at` if not present

#### Decay Filter: `applyDecay(observations []styleObservation) []styleObservation`

For each observation:
- Maximum age = `reinforced_count * DecayThresholdDays * 24 hours`
- If `time.Since(lastReinforced) < maxAge`, keep it

This means:
- A style observed once (`count=1`) survives 14 days without reinforcement
- A style observed twice (`count=2`) survives 28 days
- A style observed 5 times survives 70 days
- A style observed 10 times survives 140 days (~4.7 months)

Higher reinforcement earns proportionally longer memory.

#### Directive Retrieval: `GetDirective(ctx, db, userID) string`

Simple query:
```sql
SELECT value FROM memories
WHERE namespace = ? AND key = ? AND user_id = ?
```

Returns empty string on any error (including no rows).

This is called by `LoadContext` to inject the personality directive into the system prompt.

---

## Part 3: How Everything Connects

### Memory Lifecycle

1. **User sends message** -> enters the runner's agentic loop
2. **System prompt built** from:
   - File-based memory (`LoadMemoryFiles` -> `FormatForSystemPrompt`)
   - Database context (`LoadContext` -> `FormatForSystemPrompt`)
   - Both are concatenated and injected as the system message
3. **Agent responds** -> response streamed to user
4. **Fact extraction** (background): `Extractor.Extract()` processes recent messages
   - Extracted facts mapped to `MemoryEntry` structs with layer/namespace/key/value
   - Stored via the agent memory tool (`agent(resource: memory, action: store, ...)`)
   - Style facts (`IsStyle=true`) use reinforcement tracking instead of overwrite
5. **Embedding generation** (background): new memory values are chunked via `SplitText`, embedded via `Service.Embed`, stored in `memory_chunks` + `memory_embeddings`
6. **Personality synthesis** (periodic): `SynthesizeDirective` runs when enough style observations accumulate (>= 5)

### Search Flow

When the agent needs to recall information:
1. `agent(resource: memory, action: search, query: "...")` triggers hybrid search
2. `HybridSearcher.Search()` runs:
   - FTS5 on `memories` table (exact keyword matching via BM25)
   - FTS5 on `memory_chunks` for session transcripts (dampened by 0.6x)
   - Vector similarity on `memory_embeddings` (cosine similarity)
   - Results merged with configurable weights (default 0.7 vector + 0.3 FTS)
   - Filtered by minimum score (default 0.3)
3. Results returned to agent with citation metadata (chunk text, character offsets, timestamps)

### Memory Layers

| Layer | Purpose | Namespace Examples | Persistence |
|-------|---------|-------------------|-------------|
| `tacit` | Long-term learned knowledge | `preferences`, `personality`, `artifacts` | Permanent (subject to decay scoring) |
| `daily` | Day-specific context | `2026-03-04` | Permanent but naturally ages out via decay |
| `entity` | Information about specific entities | `default` | Permanent |

### Confidence System

Confidence flows through the entire pipeline:
1. **Extraction:** LLM provides `explicit` boolean -> mapped to confidence (0.6 inferred, 0.75 unknown, 0.9 explicit)
2. **Storage:** Confidence stored in memory metadata JSON
3. **Prompt injection:** Confidence filter at >= 0.80 prevents low-confidence facts from cluttering the system prompt
4. **Reinforcement:** When the same fact is observed again, confidence is boosted (inferred facts at 0.6 can cross 0.80 threshold after reinforcements)
5. **Search:** No confidence filter -- all memories are searchable regardless of confidence
6. **Ranking:** `confidence * decayScore(accessCount, accessedAt)` determines injection priority

### Tuning Parameters Summary

| Parameter | Value | Location |
|-----------|-------|----------|
| Chunk max chars | 1600 (~400 tokens) | `chunker.go` |
| Chunk overlap chars | 320 (~80 tokens) | `chunker.go` |
| Short text threshold | maxChars + overlapChars = 1920 runes | `chunker.go` |
| OpenAI default dimensions | 1536 | `providers.go` |
| Ollama default dimensions | 256 | `providers.go` |
| OpenAI HTTP timeout | 60s | `providers.go` |
| Ollama HTTP timeout | 120s | `providers.go` |
| Embedding cache TTL | 30 days | `service.go` |
| Embed retry count | 3 | `service.go` |
| Embed retry backoff | 500ms, 2s, 8s | `service.go` |
| Search over-fetch factor | 8x | `hybrid.go` |
| Default vector weight | 0.7 | `hybrid.go` |
| Default text weight | 0.3 | `hybrid.go` |
| Default min score | 0.3 | `hybrid.go` |
| Default search limit | 10 | `hybrid.go` |
| Session boost factor | 0.6 | `hybrid.go` |
| Max content per message (extraction) | 500 chars | `extraction.go` |
| Max conversation chars (extraction) | 15000 chars (~4k tokens) | `extraction.go` |
| Explicit fact confidence | 0.9 | `extraction.go` |
| Inferred fact confidence | 0.6 | `extraction.go` |
| Unknown explicit confidence | 0.75 | `extraction.go` |
| Prompt injection confidence threshold | 0.80 | `dbcontext.go` |
| Max tacit memories in prompt | 50 | `dbcontext.go` |
| Max style memories in prompt | 10 | `dbcontext.go` |
| Tacit memory overfetch factor | 3x (min 30) | `dbcontext.go` |
| Decay half-life basis | 30 days (0.7 decay factor) | `dbcontext.go` |
| Min style observations for synthesis | 5 | `personality.go` |
| Decay threshold days (count=1) | 14 | `personality.go` |
| Max observations for synthesis | 15 | `personality.go` |
