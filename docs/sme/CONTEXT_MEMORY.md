# Context & Memory System — SME Deep-Dive

> **Purpose:** Complete technical reference for Nebo's context assembly, memory persistence, and knowledge retrieval systems. Read this file to become a context/memory SME.

---

## Table of Contents

1. [Architecture Overview](#architecture-overview)
2. [Context Assembly Pipeline](#context-assembly-pipeline)
3. [Memory Storage (3-Tier Model)](#memory-storage-3-tier-model)
4. [Memory Extraction (Automatic)](#memory-extraction-automatic)
5. [Personality Synthesis](#personality-synthesis)
6. [Hybrid Search (FTS5 + Vector)](#hybrid-search-fts5--vector)
7. [Embeddings Service](#embeddings-service)
8. [Session Management & Compaction](#session-management--compaction)
9. [Session Transcript Indexing](#session-transcript-indexing)
10. [Steering Generators (Memory-Related)](#steering-generators-memory-related)
11. [File-Based Context (Legacy)](#file-based-context-legacy)
12. [Database Schema](#database-schema)
13. [Key Files](#key-files)
14. [Data Flow Diagrams](#data-flow-diagrams)
15. [Gotchas & Edge Cases](#gotchas--edge-cases)

---

## Architecture Overview

Nebo's memory system has **four interconnected subsystems** that work together to give the agent persistent, searchable knowledge across sessions:

```
┌─────────────────────────────────────────────────────────────────────────┐
│                     CONTEXT ASSEMBLY (per-iteration)                     │
│                                                                         │
│  Static Prompt (cached 5min by Anthropic):                              │
│    DB Context → Identity → Personality → User Profile → Tacit Memories  │
│    → Rules/ToolNotes → Static Sections → STRAP Docs → Platform Caps    │
│    → Skills → Apps → AFV Fences                                         │
│                                                                         │
│  Dynamic Suffix (per-iteration):                                        │
│    Date/Time → Model Info → Active Task → Compaction Summary            │
│                                                                         │
│  Steering Messages (ephemeral, per-iteration):                          │
│    memoryNudge → compactionRecovery → etc.                              │
└─────────────────────────────────────────────────────────────────────────┘
         ↑ reads from                              ↓ writes to
┌─────────────────────────────────────────────────────────────────────────┐
│                     MEMORY STORAGE (SQLite)                              │
│                                                                         │
│  memories table:     namespace/key/value/tags/metadata/user_id          │
│  memory_chunks:      chunk_index/text/source/start_char/end_char        │
│  memory_embeddings:  chunk_id/model/embedding (BLOB)                    │
│  memories_fts:       FTS5 virtual table (key, value, tags)              │
│  embedding_cache:    SHA256 content hash → embedding (dedup)            │
│                                                                         │
│  session_messages:   role/content/tool_calls/tool_results/is_compacted  │
│  sessions:           summary/token_count/compaction_count               │
└─────────────────────────────────────────────────────────────────────────┘
         ↑ stored by                               ↑ searched by
┌──────────────────────────┐    ┌──────────────────────────────────────────┐
│   MEMORY EXTRACTION      │    │   HYBRID SEARCH                          │
│   (automatic, per-turn)  │    │   (FTS5 + vector cosine similarity)      │
│                          │    │                                          │
│   Debounced 5s idle →    │    │   FTS: BM25 scoring on memories_fts      │
│   LLM extracts 5 fact    │    │   Vector: cosine sim on memory_embeddings│
│   categories →           │    │   Merge: 70% vector + 30% text weight    │
│   Dedup → Store → Embed  │    │   MinScore: 0.3, 8x over-fetch          │
│                          │    │   Dedup: best chunk per memory_id        │
│   Pre-compaction flush → │    │                                          │
│   Full message extract   │    │   Fallback chain:                        │
│                          │    │   FTS5 → LIKE → vector-only              │
└──────────────────────────┘    └──────────────────────────────────────────┘
```

---

## Context Assembly Pipeline

### When It Runs

Once per `Runner.Run()` call (line 372 of `runner.go`). The static prompt is built once and reused across all agentic loop iterations. Only the dynamic suffix changes per iteration.

### Assembly Order (BuildStaticPrompt)

File: `internal/agent/runner/prompt.go:~515`

```
1.  ContextSection (from DB or file fallback)
      ├── Personality prompt (preset or custom)
      ├── Character (creature, role, vibe, emoji)
      ├── Personality directive (learned, synthesized)
      ├── Communication style (voice, formality, emoji, length)
      ├── User information (name, location, timezone, occupation, interests, goals)
      ├── Agent rules (structured JSON sections → markdown)
      ├── Tool notes (structured JSON sections → markdown)
      ├── "What You Know" (tacit memories, max 50)
      └── Memory tool instructions
2.  --- separator
3.  Static sections (9 constants):
      - sectionIdentityAndPrime
      - sectionCapabilities (platform-aware)
      - sectionToolsDeclaration
      - sectionCommStyle
      - sectionMedia
      - sectionMemoryDocs
      - sectionToolGuide
      - sectionBehavior
4.  STRAP tool documentation (all tools)
5.  Platform capabilities (from registry)
6.  Registered tool list (reinforcement)
7.  Skill hints (trigger-matched)
8.  Active skill content
9.  App catalog
10. Model aliases
11. Tool awareness reminder (recency bias, near end)
12. AFV security fences
```

### Dynamic Suffix (BuildDynamicSuffix)

File: `internal/agent/runner/prompt.go:~595`

Built per-iteration, appended after the static prompt:

```
1. Date/time header (with timezone, UTC offset, year reinforcement)
2. System context (provider/model, hostname, OS, arch)
3. Active task pin (survives compaction)
4. Compaction summary (cumulative, from previous compactions)
```

### DB Context Loading

File: `internal/agent/memory/dbcontext.go:~69`

`LoadContext(db, userID)` loads from SQLite:

| Source | Table | What |
|--------|-------|------|
| Agent profile | `agent_profile` (id=1) | name, personality, voice style, emoji usage, formality, proactivity, creature, vibe, role, rules, tool notes |
| User profile | `user_profiles` | display name, location, timezone, occupation, interests (JSON array), goals, context, comm style, onboarding status |
| Tacit memories | `memories` | Two-pass: up to 10 from `tacit/personality`, then fill remaining slots up to 50 total from all other `tacit/*` namespaces |
| Personality directive | `memories` | Stored at namespace=`tacit/personality`, key=`directive` |

**Memory budget:** `maxTacitMemories=50`, `maxStyleMemories=10`. Ordered by `access_count DESC`.

**Fallback chain:** If DB loading fails → load file-based context (AGENTS.md, MEMORY.md, SOUL.md) → if all empty → hardcoded identity prompt.

### FormatForSystemPrompt Output

File: `internal/agent/memory/dbcontext.go:~406`

Renders as markdown with `---` separators:

```markdown
# Identity (or personality prompt with {name} replaced)

## Character
You are a [creature]. Your relationship: [role]. Your vibe: [vibe]. Your emoji: [emoji].

## Personality (Learned)
[Synthesized directive paragraph]

Communication style: [voice] voice, [formality] formality, [emoji] emoji usage, [length] response length

# User Information
Name: [display_name]
Location: [location]
...

# Rules
## [Section Name]
- [enabled item]
...

# Tool Notes
## [Section Name]
- [enabled item]
...

## What You Know
These are facts you've learned and stored. Reference them naturally:
- preferences/code-style: Prefers 4-space indentation
- person/sarah: User's wife, works at Google
...

# Memory
You have a persistent memory system. Use it actively:
- **Recall**: agent(resource: memory, action: recall, key: "...")
- **Search**: agent(resource: memory, action: search, query: "...")
- **Store**: agent(resource: memory, action: store, key: "...", value: "...", layer: "tacit")
```

### Structured Content Rendering

File: `internal/agent/memory/dbcontext.go:~527`

Agent rules and tool notes support structured JSON format:

```json
{
  "version": 1,
  "sections": [
    {
      "name": "Code Style",
      "items": [
        {"text": "Always use gofmt", "enabled": true},
        {"text": "Tab indentation", "enabled": false}
      ]
    }
  ]
}
```

Falls back to raw markdown if not valid structured JSON (backwards compat).

---

## Memory Storage (3-Tier Model)

### Layers

| Layer | Namespace Pattern | Lifespan | Use Case |
|-------|-------------------|----------|----------|
| `tacit` | `tacit`, `tacit/preferences`, `tacit/personality`, `tacit/artifacts` | Permanent (with decay for personality) | Long-term preferences, style observations, produced content |
| `daily` | `daily/<YYYY-MM-DD>` | Time-scoped by date | Day-specific facts, decisions |
| `entity` | `entity/default` | Permanent | People, places, projects, things |

### Storage Schema

**Effective namespace** = `layer + "/" + namespace` (if namespace is provided and isn't the layer itself).

Example: `layer="tacit"`, `namespace="preferences"` → effective namespace = `tacit/preferences`.

### Memory Key Normalization

File: `internal/agent/memory/extraction.go:~243`

All keys are normalized via `NormalizeMemoryKey()`:
- Lowercase
- Underscores → hyphens
- Spaces → hyphens
- Collapse repeated hyphens/slashes
- Trim leading/trailing hyphens/slashes

Example: `"Code_Style"` → `"code-style"`, `"Preference/Code-Style"` → `"preference/code-style"`

### Sanitization

File: `internal/agent/tools/memory.go:~17-85`

Two layers of protection:

1. **Prompt injection detection** — regex blocks patterns like:
   - "ignore all previous instructions"
   - "you are now"
   - `<system>` tags
   - "IMPORTANT: you must"
   - "pretend you are"

2. **Content limits:**
   - Key: max 128 chars, control chars stripped
   - Value: max 2048 chars, control chars stripped

### Deduplication

File: `internal/agent/tools/memory.go:~1103-1141`

Two-check dedup via `IsDuplicate()`:
1. **Exact key match** — same namespace + key + user_id → compare values
2. **Same content under any key** — scan namespace for identical value

### Style Reinforcement Tracking

File: `internal/agent/tools/memory.go:~1022-1101`

Style observations (category=`style`, namespace=`tacit/personality`) use **reinforcement** instead of overwrite:

```sql
-- On conflict: increment reinforced_count, update last_reinforced
ON CONFLICT(namespace, key, user_id) DO UPDATE SET
    metadata = json_set(
        COALESCE(memories.metadata, '{}'),
        '$.reinforced_count', COALESCE(json_extract(memories.metadata, '$.reinforced_count'), 0) + 1,
        '$.last_reinforced', ?
    ),
    updated_at = CURRENT_TIMESTAMP
```

Metadata example:
```json
{
  "reinforced_count": 5,
  "first_observed": "2026-02-01T10:00:00Z",
  "last_reinforced": "2026-02-25T14:30:00Z"
}
```

### Vector Embedding (Async)

File: `internal/agent/tools/memory.go:~360-456`

After storing a memory, `embedMemory()` runs async in a goroutine:
1. Delete existing chunks for this memory
2. Build embeddable text: `"key: value"` (or `"[namespace] key: value"` if non-default namespace)
3. Split via `embeddings.SplitText()` (1600 char chunks, 320 char overlap)
4. Batch embed all chunks
5. Store chunks to `memory_chunks` + embeddings to `memory_embeddings`

---

## Memory Extraction (Automatic)

### Two Triggers

File: `internal/agent/runner/runner.go`

#### Trigger 1: Debounced Idle Extraction

**When:** After every agentic loop completion (no more tool calls), debounced by 5 seconds.

**Scope:** Last 6 messages only (last turn — extraction runs per-turn, so older messages were already processed).

**Flow:**
```
runLoop completes (no tool calls)
  → scheduleMemoryExtraction(sessionID, userID)
    → time.AfterFunc(5s, ...) — debounced, resets on new calls
      → extractAndStoreMemories(sessionID, userID)
        → sync.Map guard (prevents overlapping extractions)
        → 90s timeout, 30s watchdog
        → Load last 6 messages
        → Try cheapest model first, then fallback providers
        → memory.NewExtractor(provider).Extract(ctx, messages)
        → FormatForStorage() → MemoryEntry[]
        → For each entry:
            If IsStyle → StoreStyleEntryForUser() (reinforcement)
            Else → IsDuplicate() check → StoreEntryForUser()
        → If styles extracted → SynthesizeDirective()
```

#### Trigger 2: Pre-Compaction Memory Flush

**When:** Before compaction, when tokens exceed 75% of compaction limit.

**Scope:** ALL messages in the session (full conversation).

**Guard:** `ShouldRunMemoryFlush(sessionID)` — compares `compaction_count` vs `memory_flush_compaction_count` to prevent double-flush per compaction cycle.

**Flow:**
```
runLoop → token count > 75% of autoCompact threshold
  → maybeRunMemoryFlush(ctx, sessionID, userID, messages)
    → Check token threshold
    → ShouldRunMemoryFlush() — dedup across compaction cycles
    → RecordMemoryFlush() — mark intent
    → Resolve cheapest provider
    → go runMemoryFlush(ctx, provider, messages, userID) — background goroutine
      → 90s timeout
      → memory.NewExtractor(provider).Extract(ctx, messages)
      → FormatForStorage() → store with dedup
```

### Extraction Prompt

File: `internal/agent/memory/extraction.go:~74`

The LLM is prompted to return JSON with 5 arrays:

| Category | Storage Layer | Namespace | Examples |
|----------|--------------|-----------|----------|
| `preferences` | tacit | preferences | Code style, tool preferences |
| `entities` | entity | default | People (`person/sarah`), projects (`project/nebo`) |
| `decisions` | daily | `<YYYY-MM-DD>` | Architecture decisions, choices made |
| `styles` | tacit | personality | Humor preference, verbosity, engagement patterns |
| `artifacts` | tacit | artifacts | Copy written, strategies outlined, code explained |

**Input limits:**
- 500 chars per message (truncated)
- 15,000 chars total conversation (tail-biased — recent messages more relevant)
- Tool-role messages skipped entirely

**Output parsing:**
- Strip markdown code fences
- Extract first JSON object (brace matching)
- Handle non-string `value` fields via custom `UnmarshalJSON`

---

## Personality Synthesis

File: `internal/agent/memory/personality.go`

### How It Works

After style observations are extracted, `SynthesizeDirective()` is called:

1. **Load** all `tacit/personality/style/*` memories with their reinforcement metadata
2. **Minimum threshold:** Need at least 3 style observations (`MinStyleObservations`)
3. **Decay filter:** Remove weak observations:
   - `reinforced_count=1` → expires after 14 days (`DecayThresholdDays`)
   - Higher counts get proportionally longer lifespans: `maxAge = count * 14 days`
4. **Sort** by reinforcement count (strongest signals first)
5. **Cap** at top 15 observations
6. **LLM synthesis:** Prompt generates a one-paragraph personality directive (3-5 sentences, second person)
7. **Store** as `tacit/personality/directive` memory (upsert)

### Directive in System Prompt

The directive appears as `## Personality (Learned)` in the system prompt, between the Character section and the Communication Style section.

---

## Hybrid Search (FTS5 + Vector)

File: `internal/agent/embeddings/hybrid.go`

### Search Flow

```
HybridSearcher.Search(ctx, query, opts)
  ├── searchFTS(query, namespace, userID, limit*8)
  │     └── FTS5 MATCH on memories_fts → BM25 scoring
  │         (fallback: searchLike → LIKE pattern matching, score=0.5)
  │
  ├── searchVector(ctx, query, namespace, userID, limit*8) — if embedder available
  │     ├── Embed query text
  │     ├── Load all embeddings for user (memory + session chunks via LEFT JOIN)
  │     ├── Cosine similarity against each
  │     └── Dedup by memory_id (keep best-scoring chunk)
  │
  └── mergeResults(fts, vector, vectorWeight=0.7, textWeight=0.3)
        ├── Merge by namespace:key
        ├── Combined score = 0.7 * vectorScore + 0.3 * textScore
        ├── Filter: score >= minScore (0.3)
        └── Sort by combined score descending
```

### Search Result Fields

```go
type SearchResult struct {
    ID          int64
    Key         string
    Value       string
    Namespace   string
    Score       float64  // Combined weighted score
    VectorScore float64  // Cosine similarity (0-1)
    TextScore   float64  // BM25 normalized score (0-1)
    Source      string   // "fts", "like", or "vector"
    ChunkText   string   // Specific matching chunk text
    StartChar   int      // Position in original memory value
    EndChar     int
    CreatedAt   string
}
```

### FTS Query Building

Tokens are extracted, cleaned (alphanumeric + underscore only), quoted, and joined with AND:

```
"golang tutorials" → "golang" AND "tutorials"
```

### BM25 Score Normalization

BM25 ranks are negative (lower = better). Converted to 0-1 scale:
- If rank >= 0: `1 / (1 + rank)`
- If rank < 0: `1 / (1 - rank)` (flips negative)

---

## Embeddings Service

File: `internal/agent/embeddings/service.go`

### Providers

| Provider | Model Default | Dimensions | Notes |
|----------|--------------|------------|-------|
| OpenAI | `text-embedding-3-small` | 1536 | Standard embedding API |
| Ollama | `qwen3-embedding` | 256 | Local, `/api/embed` endpoint |

### Caching

- Content is SHA256-hashed
- Cached in `embedding_cache` table (content_hash → embedding blob)
- Stale cache eviction: >30 days, on service startup
- Embeddings stored as JSON-serialized `[]float32` blobs

### Retry Logic

3 attempts with exponential backoff (500ms → 2s → 8s). No retry on 4xx errors (auth/client).

### Text Chunking

File: `internal/agent/embeddings/chunker.go`

- Chunk size: ~400 tokens / 1600 chars
- Overlap: ~80 tokens / 320 chars
- Short texts (<1920 chars): single chunk, no splitting
- Sentence boundary splitting (`.!?` + space/newline, or double newline)
- Overlap achieved by rewinding sentence index

---

## Session Management & Compaction

File: `internal/db/session_manager.go`

### Session Lifecycle

```
GetOrCreate(sessionKey, userID) → session with unique(name, scope, scope_id)
  → AppendMessage(sessionID, msg) — inserts to session_messages
  → GetMessages(sessionID, limit) — returns non-compacted messages (is_compacted=0)
  → Compact(sessionID, summary, keepCount) — marks old messages as compacted
```

### Compaction Strategy

File: `internal/agent/runner/runner.go` (graduated threshold compaction at ~line 541, overflow retry at ~line 814)

**Progressive compaction** — when tokens exceed autoCompact threshold:

1. Try `keep=10` (keep last 10 messages)
2. If still over threshold → try `keep=3`
3. If still over threshold → try `keep=1`

Each compaction:
- Marks all but last N messages as `is_compacted=1`
- Stores LLM-generated summary in `sessions.summary`
- Increments `compaction_count`
- **Cumulative summaries:** Previous summary is compressed and prepended to new summary

### After Compaction

1. **File re-injection:** Recently accessed files are re-injected as a user message to recover working context
2. **Session transcript indexing:** Compacted messages are chunked and embedded for semantic search (async)

### Memory Flush Guard

```
ShouldRunMemoryFlush(sessionID)
  → compaction_count > memory_flush_compaction_count
  → Only flush once per compaction cycle

RecordMemoryFlush(sessionID)
  → memory_flush_compaction_count = compaction_count
```

### Active Task Pin

The active task survives compaction — stored in `sessions.active_task` column, injected into the dynamic suffix on every iteration.

---

## Session Transcript Indexing

File: `internal/agent/tools/memory.go:~1143-1271`

After compaction, `IndexSessionTranscript()` converts conversation history into searchable embeddings:

1. Load all messages after `last_embedded_message_id`
2. Group into blocks of 5 messages
3. For each block:
   - Concatenate as `[role]: content\n\n`
   - Create chunk with `source="session"`, `memory_id=NULL`, `path=sessionID`
   - Embed and store in `memory_chunks` + `memory_embeddings`
4. Update `last_embedded_message_id`

These session chunks participate in vector search alongside memory chunks (via the LEFT JOIN in `searchVector`).

---

## Steering Generators (Memory-Related)

File: `internal/agent/steering/generators.go`

### memoryNudge (Generator 6)

**Fires when:**
- At least 10 assistant turns in conversation
- `agent` tool not used in last 10 turns
- Recent user messages (last 10) contain self-disclosure patterns

**Two pattern lists (29 total):**

Self-disclosure patterns (17):
```
"i am", "i'm", "my name", "i work", "i live",
"i prefer", "i like", "i don't like", "i hate",
"i always", "i never", "i usually",
"my job", "my company", "my team",
"my wife", "my husband", "my partner",
"my email", "my phone", "my address",
"call me", "i go by"
```

Behavioral patterns (12):
```
"can you always", "from now on", "don't ever",
"stop using", "start using", "going forward",
"every time", "when i ask", "please remember",
"keep in mind", "for future", "note that i"
```

Fires if **either** list matches in recent user messages.

**Message injected (ephemeral, never persisted):**
> If the user has shared personal facts, preferences, or important information recently, consider storing them using agent(resource: memory, action: store). Only store if genuinely useful.

### compactionRecovery (Generator 4)

Fires after compaction to help the agent recover context from the summary.

---

## File-Based Context (Legacy)

File: `internal/agent/memory/files.go`

### Files Loaded

| File | Purpose | In System Prompt? |
|------|---------|-------------------|
| `AGENTS.md` | Agent behavior instructions | Yes |
| `MEMORY.md` | Long-term facts and preferences | Yes |
| `SOUL.md` | Personality and identity | Yes |
| `HEARTBEAT.md` | Proactive tasks to check | No (used by heartbeat daemon) |

### Resolution Order

1. Workspace directory (if provided)
2. Nebo data directory (`~/Library/Application Support/Nebo/` on macOS)

First match wins. Fallback to DB context in normal operation — file-based context is the legacy/error path.

---

## Database Schema

### memories

```sql
CREATE TABLE memories (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    namespace TEXT NOT NULL DEFAULT 'default',
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    tags TEXT,           -- JSON array
    metadata TEXT,       -- JSON object (reinforced_count, timestamps)
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    accessed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    access_count INTEGER DEFAULT 0,
    user_id TEXT NOT NULL DEFAULT ''
);
-- Unique: (namespace, key, user_id) via idx_memories_namespace_key_user
```

### memories_fts (FTS5)

```sql
CREATE VIRTUAL TABLE memories_fts USING fts5(
    key, value, tags,
    content='memories',
    content_rowid='id'
);
-- Sync triggers: memories_ai, memories_ad, memories_au
```

### memory_chunks

```sql
CREATE TABLE memory_chunks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    memory_id INTEGER REFERENCES memories(id) ON DELETE CASCADE,  -- nullable for session chunks
    chunk_index INTEGER NOT NULL,
    text TEXT NOT NULL,
    source TEXT DEFAULT 'memory',    -- 'memory' or 'session'
    path TEXT DEFAULT '',            -- sessionID for session chunks
    start_char INTEGER DEFAULT 0,
    end_char INTEGER DEFAULT 0,
    model TEXT DEFAULT '',
    user_id TEXT NOT NULL DEFAULT '',
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
```

### memory_embeddings

```sql
CREATE TABLE memory_embeddings (
    id INTEGER PRIMARY KEY,
    chunk_id INTEGER REFERENCES memory_chunks(id) ON DELETE CASCADE,
    model TEXT NOT NULL,
    dimensions INTEGER NOT NULL,
    embedding BLOB NOT NULL,    -- JSON-serialized []float32
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
```

### embedding_cache

```sql
CREATE TABLE embedding_cache (
    content_hash TEXT PRIMARY KEY,    -- SHA256 of input text
    embedding BLOB NOT NULL,
    model TEXT NOT NULL,
    dimensions INTEGER NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
```

### sessions

```sql
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    name TEXT,
    scope TEXT DEFAULT 'global',
    scope_id TEXT,
    summary TEXT,
    token_count INTEGER DEFAULT 0,
    message_count INTEGER DEFAULT 0,
    last_compacted_at INTEGER,
    compaction_count INTEGER DEFAULT 0,
    memory_flush_at INTEGER,
    memory_flush_compaction_count INTEGER,
    last_embedded_message_id INTEGER DEFAULT 0,
    active_task TEXT,
    metadata TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
-- Unique: (name, scope, scope_id)
```

### session_messages

```sql
CREATE TABLE session_messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    content TEXT,
    tool_calls TEXT,      -- JSON
    tool_results TEXT,     -- JSON
    token_estimate INTEGER DEFAULT 0,
    is_compacted INTEGER DEFAULT 0,
    created_at INTEGER NOT NULL
);
```

---

## Key Files

| File | LOC | Purpose |
|------|-----|---------|
| `internal/agent/memory/dbcontext.go` | ~573 | DB context loading, system prompt formatting |
| `internal/agent/memory/extraction.go` | ~343 | LLM-based fact extraction from conversations |
| `internal/agent/memory/personality.go` | ~217 | Style observation synthesis into personality directive |
| `internal/agent/memory/files.go` | ~93 | File-based context loading (AGENTS.md, MEMORY.md, SOUL.md) |
| `internal/agent/tools/memory.go` | ~1387 | MemoryTool: store, recall, search, embed, index |
| `internal/agent/embeddings/service.go` | ~260 | Embedding generation with caching |
| `internal/agent/embeddings/hybrid.go` | ~449 | Hybrid search (FTS5 + vector) |
| `internal/agent/embeddings/providers.go` | ~214 | OpenAI and Ollama embedding providers |
| `internal/agent/embeddings/chunker.go` | ~175 | Sentence-boundary text chunking |
| `internal/agent/runner/prompt.go` | ~689 | System prompt assembly (static + dynamic) |
| `internal/agent/runner/runner.go` | ~2050 | Agentic loop (memory extraction in ~1796-1978 range) |
| `internal/agent/session/session.go` | ~28 | Session type aliases (thin wrapper) |
| `internal/agent/session/keyparser.go` | ~206 | Hierarchical session key parsing |
| `internal/db/session_manager.go` | ~600 | Session CRUD, compaction, message storage |
| `internal/agent/steering/generators.go` | ~270 | All 10 steering generators (memoryNudge at ~120-146) |

### Migration Files

| Migration | Purpose |
|-----------|---------|
| `0013_agent_tools.sql` | Initial memories + FTS5 tables |
| `0016_vector_embeddings.sql` | memory_chunks, memory_embeddings, embedding_cache |
| `0019_memories_user_scope.sql` | Added user_id to memories and memory_chunks |
| `0021_fix_memories_unique.sql` | Rebuilt memories table: unique(namespace, key, user_id) |
| `0038_memory_chunks_schema_update.sql` | Nullable memory_id, start_char/end_char, user_id on chunks |
| `0039_session_last_embedded.sql` | last_embedded_message_id on sessions |
| `0010_agent_sessions.sql` | Initial sessions + session_messages tables |
| `0023_session_compaction_tracking.sql` | compaction_count, memory_flush tracking |

---

## Data Flow Diagrams

### Memory Write Path

```
User says "I prefer 4-space indentation"
  ↓
Runner.Run() completes turn (no more tool calls)
  ↓
scheduleMemoryExtraction() — 5s debounce timer
  ↓ (after 5s idle)
extractAndStoreMemories()
  ↓
memory.Extractor.Extract(ctx, last 6 messages)
  ↓ (LLM call — cheapest model)
ExtractedFacts{Preferences: [{Key: "code-indentation", Value: "Prefers 4-space indentation"}]}
  ↓
FormatForStorage() → MemoryEntry{Layer: "tacit", Namespace: "preferences", Key: "code-indentation", ...}
  ↓
NormalizeMemoryKey() → "code-indentation"
  ↓
IsDuplicate() check — exact key + same content
  ↓ (not duplicate)
StoreEntryForUser() → INSERT/UPSERT into memories table
  ↓ (async goroutine)
embedMemory() → SplitText → Embed → Store chunks + embeddings
```

### Memory Read Path (Agent-Initiated)

```
Agent calls: agent(resource: memory, action: search, query: "indentation preference")
  ↓
MemoryTool.Execute() → searchWithContext()
  ↓
HybridSearcher.Search(ctx, "indentation preference", opts)
  ├── searchFTS → memories_fts MATCH → BM25 scoring
  └── searchVector → embed query → cosine sim against memory_embeddings
  ↓
mergeResults(fts, vector, 0.7, 0.3) → filter(minScore=0.3) → top N
  ↓
ToolResult{Content: "Found 3 memories:\n1. [tacit/preferences] code-indentation: Prefers 4-space indentation (score: 0.85)\n..."}
```

### Memory Read Path (System Prompt)

```
Runner.Run() starts
  ↓
memory.LoadContext(db, userID)
  ↓
loadTacitMemories():
  Pass 1: SELECT * FROM memories WHERE namespace='tacit/personality' ORDER BY access_count DESC LIMIT 10
  Pass 2: SELECT * FROM memories WHERE namespace LIKE 'tacit/%' AND namespace != 'tacit/personality' ORDER BY access_count DESC LIMIT 40
  ↓
DBContext.FormatForSystemPrompt()
  ↓
"## What You Know\n- preferences/code-indentation: Prefers 4-space indentation\n..."
  ↓ (injected into static system prompt)
BuildStaticPrompt(pctx) → full system prompt
```

---

## Gotchas & Edge Cases

1. **Tacit memory budget:** Only 50 memories max in system prompt. 10 reserved for personality styles. If a user accumulates many memories, only the most-accessed ones (by `access_count`) are included.

2. **Style decay:** Styles with `reinforced_count=1` expire after 14 days. This means one-off observations are automatically pruned. Repeatedly observed patterns get proportionally longer lifespans.

3. **Extraction runs per-turn:** The idle extraction only looks at the last 6 messages. This is intentional — older messages were already processed in their respective turns.

4. **Pre-compaction flush operates on ALL messages:** Unlike idle extraction (6 messages), the pre-compaction flush sends the full conversation to the LLM. This is a safety net before messages get marked as compacted.

5. **Session transcript chunks have `memory_id=NULL`:** They participate in vector search via LEFT JOIN but aren't associated with any memory record. They're identified by `source='session'` and `path=sessionID`.

6. **Embedding model migration:** `MigrateEmbeddings()` detects when the embedding model changes and clears stale chunks/embeddings. `BackfillEmbeddings()` regenerates embeddings for memories without chunks.

7. **Concurrent extraction guard:** `sync.Map` prevents overlapping extractions for the same session. If extraction is already running, new requests are silently skipped.

8. **Memory flush double-execution prevention:** `ShouldRunMemoryFlush()` checks `compaction_count` vs `memory_flush_compaction_count`. Only one flush per compaction cycle.

9. **User ID scoping:** All memory operations are user-scoped. The `user_id` column on memories, memory_chunks, and the unique constraint ensure isolation between users.

10. **Personality directive is synthetic:** It's not a raw observation — it's an LLM-generated paragraph distilled from weighted style observations. Stored as a memory but treated specially in the system prompt (separate section).

11. **FTS5 fallback chain:** FTS5 → LIKE search → vector-only. If FTS5 fails (e.g., corrupt index), LIKE search provides a degraded but functional alternative.

12. **Embedding cache eviction:** Entries older than 30 days are cleaned on service startup. No runtime eviction.

13. **Tool results are skipped during extraction:** Messages with `role="tool"` are filtered out before sending to the extraction LLM. They don't contain extractable user facts.

14. **The `sectionMemoryDocs` in prompt.go explicitly tells the agent NOT to store explicitly:** "Facts are automatically extracted from your conversation after each turn. You do NOT need to call agent(action: store) during normal conversation." This is because the automatic extraction handles the common case, and explicit stores create duplicates.
