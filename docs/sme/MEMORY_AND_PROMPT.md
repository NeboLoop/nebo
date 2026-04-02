# Memory & Prompt System -- SME Deep-Dive

> **Last updated:** 2026-04-02
>
> **Purpose:** Definitive technical reference for Nebo's entire memory system and system prompt pipeline -- storage, extraction, personality synthesis, hybrid search, embeddings, session transcript indexing, prompt assembly, steering, and context management. Dead code (functions ported but never called from the runner) is explicitly flagged.

---

## Key Files

| File | Purpose | Status |
|------|---------|--------|
| `crates/agent/src/runner.rs` | Agentic loop with provider fallback, objective detection | Active |
| `crates/agent/src/prompt.rs` | `build_static()`, `build_dynamic_suffix()`, STRAP docs, cache boundary | Active |
| `crates/agent/src/db_context.rs` | `DBContext` struct, `load_db_context()`, `format_for_system_prompt()` (9-section assembly) | Active |
| `crates/agent/src/memory.rs` | Extraction, storage, confidence, decay scoring, `load_memory_context()`, `embed_memories_async()` | Active (embed not called) |
| `crates/agent/src/memory_debounce.rs` | Debounced extraction timer (5s per session) | Active |
| `crates/agent/src/memory_flush.rs` | Pre-compaction memory flush (`should_run_memory_flush`, `run_memory_flush`) | Dead code |
| `crates/agent/src/personality.rs` | `synthesize_directive()` with decay, LLM generation, style loading | Dead code |
| `crates/agent/src/steering.rs` | 19 steering generators, injection, pipeline | Active |
| `crates/agent/src/pruning.rs` | Sliding window, micro-compact, LLM summary, token estimation | Active |
| `crates/agent/src/compaction.rs` | Tool failure collection, enhanced summary | Dead code |
| `crates/agent/src/session.rs` | SessionManager: CRUD, summary, active task, work tasks | Active |
| `crates/agent/src/search.rs` | Hybrid search: FTS5 + vector + adaptive weights + cosine similarity | Active |
| `crates/agent/src/search_adapter.rs` | `HybridSearchAdapter`: bridges `hybrid_search()` to bot tool's `HybridSearcher` trait | Active |
| `crates/agent/src/chunking.rs` | Sentence-boundary text chunking with overlap | Active |
| `crates/agent/src/transcript.rs` | Session transcript indexing (post-compaction embedding) | Dead code |
| `crates/agent/src/sanitize.rs` | Prompt injection detection, key/value sanitization | Active |
| `crates/ai/src/types.rs` | StreamEvent (stop_reason), ChatRequest, Provider trait | Active |
| `crates/ai/src/embedding.rs` | EmbeddingProvider trait, OpenAI + Ollama providers, cached wrapper | Active |
| `crates/tools/src/bot_tool.rs` | `handle_memory()`: store/recall/search/list/delete/clear + hybrid search | Active |
| `crates/db/src/queries/embeddings.rs` | Embedding cache, memory chunks, memory embeddings DB queries | Active |

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [Three-Tier Storage Model](#2-three-tier-storage-model)
3. [Memory Extraction (Automatic)](#3-memory-extraction-automatic)
4. [Personality Synthesis](#4-personality-synthesis)
5. [Memory Tool (Agent Actions)](#5-memory-tool-agent-actions)
6. [Hybrid Search (FTS5 + Vector)](#6-hybrid-search-fts5--vector)
7. [Embeddings Service](#7-embeddings-service)
8. [Text Chunking](#8-text-chunking)
9. [Confidence System](#9-confidence-system)
10. [Security & Sanitization](#10-security--sanitization)
11. [DB Context Assembly (Memory -> Prompt)](#11-db-context-assembly-memory---prompt)
12. [Static Prompt Assembly](#12-static-prompt-assembly)
13. [Dynamic Suffix (Per-Iteration)](#13-dynamic-suffix-per-iteration)
14. [Steering Messages (Ephemeral)](#14-steering-messages-ephemeral)
15. [Context Management Pipeline](#15-context-management-pipeline)
16. [Session Management](#16-session-management)
17. [Session Transcript Indexing](#17-session-transcript-indexing)
18. [Special Prompt Paths](#18-special-prompt-paths)
18.5. [Continuation & Recovery Mechanisms](#185-continuation--recovery-mechanisms)
19. [The Complete Flow: User Message -> LLM Call](#19-the-complete-flow-user-message---llm-call)
20. [The Timing Dance](#20-the-timing-dance)
21. [Memory's Journey Through the Prompt Layers](#21-memorys-journey-through-the-prompt-layers)
22. [Maintenance Operations](#22-maintenance-operations)
23. [Performance Characteristics](#23-performance-characteristics)
24. [Database Schema](#24-database-schema)
25. [Configuration Reference](#25-configuration-reference)
26. [Key Design Decisions](#26-key-design-decisions)
27. [Gotchas & Edge Cases](#27-gotchas--edge-cases)

---

## 1. Architecture Overview

The memory and prompt systems form a **circular pipeline**. Memory is the data layer (stores, extracts, searches knowledge). The system prompt is the delivery layer (assembles that knowledge into what the LLM sees). Together they create a feedback loop where conversations generate knowledge that shapes future conversations.

```
+-----------------------------------------------------------------------------+
|                         THE CIRCULAR PIPELINE                                |
|                                                                              |
|  Conversation                                                                |
|       |                                                                      |
|       v                                                                      |
|  Memory Extraction (per-turn, debounced 5s)                                  |
|       | LLM extracts 5 fact categories from last 6 messages                  |
|       v                                                                      |
|  SQLite Storage (memories, memory_chunks, memory_embeddings)                 |
|       |                                                                      |
|       +---> System Prompt Assembly (per-Run)                                 |
|       |      Loads tacit memories -> "What You Know" section                 |
|       |      Loads personality directive -> "Personality (Learned)" section   |
|       |                                                                      |
|       +---> Agent Tool Recall (on-demand)                                    |
|       |      Hybrid search (FTS5 + vector) -> ToolResult in messages         |
|       |                                                                      |
|       +---> Session Transcript Index (post-eviction)                         |
|              Evicted messages -> embedded chunks -> searchable               |
|                                                                              |
|  System Prompt + Messages -> LLM -> Response -> Conversation                |
|       ^                                                                      |
|       |                                                                      |
|  Steering Messages (ephemeral, per-iteration)                                |
|       memoryNudge, identity guard, loop detector, etc.                       |
+-----------------------------------------------------------------------------+
```

The system prompt is a **two-tier, cache-optimized structure**:

```
+----------------------------------------------------------+
|  STATIC PROMPT (Tier 1)                                   |
|  Built once per Run(), reused across iterations            |
|  Anthropic caches this prefix                              |
|                                                            |
|  1. DB Context (identity, persona, user, memories)         |
|  2. Static sections (identity, capabilities, behavior)     |
|  3. STRAP tool documentation (context-activated)           |
|  4. Plugin inventory                                       |
|  5. CACHE BOUNDARY marker                                  |
|  6. Skill catalog + active skills                          |
|  7. Model aliases                                          |
+------------------------------------------------------------+
|  DYNAMIC SUFFIX (Tier 2)                                   |
|  Rebuilt every iteration, appended after static             |
|                                                            |
|  1. Current date/time/timezone                             |
|  2. System context (model, hostname, OS)                   |
|  3. Conversation summary                                   |
|  4. Background objective (soft pin)                        |
+------------------------------------------------------------+
|  STEERING MESSAGES (ephemeral, never persisted)            |
|  Injected into the message array, not the system prompt    |
|                                                            |
|  16 generators, wrapped in <steering> tags                 |
|  Appear as user-role messages to the LLM                   |
+------------------------------------------------------------+
```

The final prompt sent to the LLM: `enrichedPrompt = staticSystem + dynamicSuffix`

This is placed in `ChatRequest.system`. Each provider maps it to their API format:
- **Anthropic:** `params.System = []TextBlockParam{{Text: req.System}}`
- **OpenAI:** `openai.SystemMessage(req.System)` prepended to messages
- **Gemini:** `SystemInstruction` with text part
- **Ollama:** system role message prepended

---

## 2. Three-Tier Storage Model

### Layers

| Layer | Namespace Pattern | Lifespan | Use Case | Example Keys |
|-------|-------------------|----------|----------|--------------|
| `tacit` | `tacit/preferences`, `tacit/personality`, `tacit/artifacts` | Permanent (with decay for personality) | Long-term preferences, style observations, produced content | `code-indentation`, `style/humor-dry`, `artifact/landing-page-hero-copy` |
| `daily` | `daily/<YYYY-MM-DD>` | Time-scoped by date | Day-specific facts, decisions | `architecture-decision`, `meeting-notes` |
| `entity` | `entity/default` | Permanent | People, places, projects, things | `person/sarah`, `project/nebo` |

### Namespace Resolution

**Effective namespace** = `layer + "/" + namespace` (if namespace is provided and is not the layer itself).

```
layer="tacit", namespace="preferences" -> "tacit/preferences"
layer="tacit", namespace=""           -> "tacit"
layer="",     namespace=""           -> "default" (for store), "" (for search -- searches all)
```

### Key Normalization

`normalize_key()` in `memory.rs`:
- Lowercase
- Underscores → hyphens
- Spaces → hyphens
- Collapse repeated hyphens/slashes
- Trim leading/trailing hyphens/slashes

```
"Code_Style"             -> "code-style"
"Preference/Code-Style"  -> "preference/code-style"
"  My--Key//path "       -> "my-key/path"
```

An additional `task_context` category (not in the original 5) maps to `daily/<date>`.

**File:** `crates/agent/src/memory.rs` lines 147-236

---

## 3. Memory Extraction (Automatic)

### Trigger: Debounced Idle Extraction

**When:** After every agentic loop completion (no more tool calls), debounced by 5 seconds.

**Scope:** Last 6 messages only. (Older messages were already processed in their respective turns.)

**Flow:**
```
runLoop completes (text-only response, no tool calls)
  -> MemoryDebouncer.schedule(session_id, callback)
    -> Cancels existing timer via CancellationToken (debounce reset)
    -> tokio::spawn with 5s sleep
      -> extract_facts(provider, messages) with temperature=0.0, max_tokens=4096
        -> build_conversation_text(messages) -- truncate, skip tools, tail-biased
        -> LLM call with extraction prompt
        -> extract_json_object() -- brace-matching with markdown fence stripping
        -> Parse into ExtractedFacts
      -> store_facts(store, facts, user_id)
        -> For each fact:
            detect_prompt_injection(key, value) -- skip if detected
            If category is "styles" -> store_style_observation() (reinforcement)
            Else -> format_for_storage() -> upsert_memory()
```

### Extraction Prompt

The LLM is prompted to return JSON with 5 arrays:

```
Analyze the following conversation and extract durable facts that should be
remembered long-term.

Return a JSON object with five arrays:
1. "preferences" - User preferences and learned behaviors
2. "entities" - Information about people, places, projects (key: "type/name")
3. "decisions" - Important decisions made during this conversation
4. "styles" - Communication/personality style observations (key: "style/trait-name")
5. "artifacts" - Important content produced (key: "artifact/description")

Each fact should have:
- "key": A unique, descriptive key for retrieval (path-like: "category/name")
- "value": The actual information to remember
- "category": One of "preference", "entity", "decision", "style", "artifact"
- "tags": Relevant tags for searching
- "explicit": boolean -- true if user directly stated, false if inferred
```

### Input Limits

| Limit | Value | Purpose |
|-------|-------|---------|
| `MAX_CONTENT_PER_MESSAGE` | 500 chars | Truncate individual messages |
| `MAX_CONVERSATION_CHARS` | 15,000 chars | Cap total prompt (~4k tokens) |
| Tool messages | Skipped entirely | Tool results don't contain extractable user facts |
| Tail-biased | `.iter().rev()` | Recent messages are more relevant for extraction |

### Response Parsing

1. Strip markdown code fences (`` ```json ... ``` ``)
2. Strip inline backticks
3. Find first `{...}` JSON object (brace-matching with escape handling)
4. `serde_json` deserialize into `ExtractedFacts`
5. Empty response or no JSON → return None (no error -- common for trivial conversations)

### FormatForStorage Mapping

`format_for_storage()` in `memory.rs`:

| Category | Layer | Namespace | IsStyle | Example Key |
|----------|-------|-----------|---------|-------------|
| `preferences` | `tacit` | `preferences` | false | `code-indentation` |
| `entities` | `entity` | `default` | false | `person/sarah` |
| `decisions` | `daily` | `<YYYY-MM-DD>` | false | `architecture-choice` |
| `styles` | `tacit` | `personality` | **true** | `style/humor-dry` |
| `artifacts` | `tacit` | `artifacts` | false | `artifact/hero-copy` |
| `task_context` | `daily` | `<YYYY-MM-DD>` | false | `task-context-note` |

### Confidence Resolution

`resolve_confidence()` maps the `explicit` field:

| Source | Confidence | Meaning |
|--------|-----------|---------|
| `explicit: true` | 0.9 | User directly stated the fact |
| `explicit: false` | 0.6 | Inferred from context/behavior |
| No explicit field | Raw value clamped 0-1 | Fallback |

### Dead Code: Pre-Compaction Memory Flush

`crates/agent/src/memory_flush.rs` (~86 lines) contains infrastructure for a second extraction trigger that would fire before compaction on ALL messages (not just last 6). Functions `should_run_memory_flush()` and `run_memory_flush()` exist with dedup guards (compares `compaction_count` vs `memory_flush_compaction_count`). **Not called from runner.**

### Known Gaps

- No `IsDuplicate()` check before storing (relies on upsert for collision handling)
- No concurrent extraction guard per session (debouncer cancels previous timer but no execution-level lock)
- No timeout on extraction (provider stream may hang indefinitely)
- Uses `prov_lock.first()` for model selection, not cost-optimized

**Files:** `crates/agent/src/memory.rs`, `crates/agent/src/memory_debounce.rs`

---

## 4. Personality Synthesis

> **Status: Dead code.** Fully implemented in `crates/agent/src/personality.rs` (~180 lines) but never called.

### How It Works (When Wired)

```
store_facts() stores style observations via store_style_observation()
  -> If synthesize_directive() were called after styles extracted:
    1. Load all tacit/personality/style/* observations for user
    2. If < 3 observations -> skip silently
    3. Apply decay: drop observations where age > reinforced_count × 14 days (if confidence < 0.7)
    4. Sort by reinforced_count DESC, cap at 15 observations
    5. Build synthesis prompt: "- key: value (observed N times)"
    6. LLM call -> one-paragraph personality directive (3-5 sentences, 2nd person)
    7. Store as tacit/personality/directive with metadata:
         {"synthesized_at": "RFC3339", "observation_count": N}
```

### Decay Algorithm

| Reinforced Count | Lifespan | Meaning |
|-----------------|----------|---------|
| 1 | 14 days | One-off observation, auto-pruned if not seen again |
| 2 | 28 days | Observed twice, moderate confidence |
| 5 | 70 days | Strong signal, persists ~2.3 months |
| 10 | 140 days | Very strong, persists ~4.7 months |

High-confidence observations (≥ 0.7) survive expiry.

### Directive in System Prompt

The directive appears as `## Personality (Learned)` in the system prompt's 9-section assembly from `db_context.rs`. If a directive were written (e.g., manually via the memory tool), it WOULD appear. The missing piece is the automated synthesis trigger.

### What IS Wired

- Style reinforcement (`store_style_observation()` in `memory.rs`) — correctly increments `reinforced_count`, boosts confidence, preserves original text
- Directive loading (`db_context.rs:25-29`) — loads `tacit/personality/directive` for prompt
- Personality preset loading — consumed by `db_context.rs`

---

## 5. Memory Tool (Agent Actions)

File: `crates/tools/src/bot_tool.rs` (lines 75-328)

### 6 Agent Actions

#### `store`
1. Sanitize key/value (injection detection + control char stripping)
2. Build effective namespace from layer + namespace
3. `store.upsert_memory()` with user_id
4. Verify write on separate connection

#### `recall`
1. Try exact key match with user_id
2. If not found → fall back to key-only lookup across all namespaces
3. Increment `access_count` on hit
4. Return key, value, tags, metadata, created_at, access count

#### `search`
1. Use `HybridSearcher.search()` if available (FTS5 + vector)
2. Fall back to LIKE-based SQL search
3. Results truncated to 200 chars per value, max 10 results
4. Format: `"Found N memories (hybrid search):\n- key: value (score: 0.85)"`

#### `list`
- `list_memories_by_namespace()` with namespace prefix match
- Max 50 results

#### `delete`
- Three-pass cascade: exact key+namespace+user → namespace scope → key-only across namespaces

#### `clear`
- `delete_memories_by_namespace_and_user()` with namespace prefix match
- Returns count of deleted memories

### Style Reinforcement

`store_style_observation()` in `memory.rs:241-312`:

When storing a style observation that already exists:
1. Load existing metadata
2. Increment `reinforced_count`
3. Update `last_reinforced` timestamp
4. Boost confidence asymptotically: `new = old + (1.0 - old) * 0.2`
5. **Do NOT overwrite value** -- keep original observation text
6. Update metadata only

For new style observations: initial metadata includes confidence, `reinforced_count: 1`, `first_observed`, `last_reinforced`.

### Known Gaps

- No `syncToUserProfile()` bridging (memory ↔ `user_profiles` table)
- No `IsDuplicate()` deduplication (upsert handles at DB level)
- Recall does NOT fall back to hybrid search on miss (returns "not found")
- `embed` and `index` are not explicit tool actions (embedding is automatic via extraction pipeline)

---

## 6. Hybrid Search (FTS5 + Vector)

File: `crates/agent/src/search.rs`

### Search Algorithm

```
hybrid_search(store, embedding_provider, query, user_id, config)
  |
  +-- 1. Adaptive Weighting (based on query characteristics)
  |
  +-- 2. Over-fetch: fts_limit = config.limit × 3
  |
  +-- 3. Text Search (user-scoped)
  |     +-- FTS5 MATCH on memories_fts -> BM25 scoring via normalize_bm25()
  |     +-- FTS on memory_chunks_fts (session transcript chunks, dampened 0.6×)
  |
  +-- 4. Vector Search (if embedding provider available, user-scoped)
  |     +-- Embed query text
  |     +-- Load ALL embeddings for user via LEFT JOIN:
  |     |     memory_chunks LEFT JOIN memories
  |     |     (includes session chunks where memory_id IS NULL)
  |     +-- Cosine similarity against each
  |     +-- Dedup by memory_id (keep best-scoring chunk per memory)
  |
  +-- 5. Merge results
  |     +-- Merge by memory/chunk ID composite key
  |     +-- Combined score = vectorWeight × vecScore + textWeight × textScore
  |
  +-- 6. Filter (score >= minScore) -> Sort DESC -> Limit
```

### Adaptive Weights

| Query Type | Example | Vector Weight | Text Weight |
|-----------|---------|---------------|-------------|
| Short + proper nouns | `"Sarah"` | 0.35 | 0.65 |
| Short generic | `"code style"` | 0.45 | 0.55 |
| Medium (4-5 words) | `"preferred indentation for Go"` | 0.70 | 0.30 |
| Long (6+ words) | `"what did we decide about the API architecture"` | 0.80 | 0.20 |

### FTS Query Building

Tokens extracted from `crates/db/src/queries/embeddings.rs`, cleaned (alphanumeric + underscore only), quoted, joined with **OR**:
```
"golang tutorials" -> "golang" OR "tutorials"
```

### BM25 Score Normalization

BM25 ranks are negative (lower/more negative = better). Converted to 0-1:
```rust
fn normalize_bm25(rank: f64) -> f64 {
    1.0 / (1.0 + rank.abs())
}
```

### Session Chunk FTS Dampening

Session transcript chunks get a **0.6× dampening factor** -- they are less precise than dedicated memory records.

### `HybridSearchAdapter`

`crates/agent/src/search_adapter.rs` (~58 lines) bridges `hybrid_search()` to the `HybridSearcher` trait used by `bot_tool.rs`, avoiding circular crate dependencies.

### Known Gaps

- No LIKE fallback if FTS5 fails (returns empty)
- Over-fetch is 3× (not 8× as in older designs) -- narrower candidate pool

---

## 7. Embeddings Service

File: `crates/ai/src/embedding.rs`

### Provider Interface

```rust
pub trait EmbeddingProvider: Send + Sync {
    fn id(&self) -> &str;
    fn dimensions(&self) -> usize;
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
}
```

### Providers

| Provider | Model Default | Dimensions | Endpoint |
|----------|--------------|------------|----------|
| `OpenAIEmbeddingProvider` | `text-embedding-3-small` | 1536 | `POST {baseURL}/embeddings` |
| `OllamaEmbeddingProvider` | Configurable | Configurable (32-1024) | `POST {baseURL}/api/embed` |

### `CachedEmbeddingProvider`

Wraps any provider with SHA256 content hashing to `embedding_cache` table:

```
embed(texts)
  +-- 1. Check cache for each text (SHA256 hash + model)
  +-- 2. Collect uncached texts
  +-- 3. Batch embed uncached (3-attempt retry)
  |     +-- Exponential backoff: 500ms -> 2s -> 8s
  |     +-- Auth errors (401, 403): fail immediately, no retry
  +-- 4. Store results in cache (embedding_cache table)
  +-- 5. Return all embeddings in original order
```

### Storage Format

Embeddings stored as **little-endian f32 byte blobs** (not JSON-serialized).

### Cosine Similarity

Implemented in `search.rs`:
```rust
fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    dot_product / (sqrt(norm_a) * sqrt(norm_b))
}
```

---

## 8. Text Chunking

File: `crates/agent/src/chunking.rs`

### Constants

```rust
const DEFAULT_CHUNK_SIZE: usize = 1600;   // ~400 tokens per chunk
const DEFAULT_OVERLAP: usize = 320;       // ~80 tokens overlap
const SHORT_CIRCUIT_SIZE: usize = 1920;   // Single chunk threshold
```

### Algorithm

```
chunk_text(text, chunk_size, overlap)
  |
  +-- Short text (< SHORT_CIRCUIT_SIZE)?
  |   -> Return as single chunk
  |
  +-- find_sentence_boundaries(text)
  |   Boundaries:
  |   - Double newline (\n\n)
  |   - Sentence-ending punctuation (. ! ?) followed by space/newline/tab
  |
  +-- Accumulate sentences into chunks:
      For each position:
        - Add sentences until chunk_size (1600) reached
        - Create TextChunk with start_char, end_char offsets
        - Rewind for overlap: walk backwards ~320 chars worth of sentences
        - Continue from overlap position
```

### TextChunk Type

```rust
pub struct TextChunk {
    pub text: String,
    pub start_char: usize,
    pub end_char: usize,
}
```

Used by `embed_memories_async()` in `memory.rs` and `index_compacted_messages()` in `transcript.rs`.

---

## 9. Confidence System

### Extraction Confidence

`resolve_confidence()` in `memory.rs:77-83`:

| Source | Confidence | Meaning |
|--------|-----------|---------|
| `explicit: true` | 0.9 | User directly stated the fact |
| `explicit: false` | 0.6 | Inferred from context/behavior |
| No explicit field | Raw value clamped 0-1 | Backwards compatibility |

### System Prompt Filter

`get_tacit_memories_with_min_confidence()` uses:
```sql
AND (metadata IS NULL
    OR json_extract(metadata, '$.confidence') IS NULL
    OR json_extract(metadata, '$.confidence') >= 0.65)
```

Memories with confidence < 0.65 are **excluded from the system prompt** but remain **searchable via hybrid search**.

### Reinforcement Confidence Boost

`store_style_observation()` in `memory.rs:267`:
```rust
let new_confidence = (old_confidence + (1.0 - old_confidence) * 0.2).min(1.0);
```

| Reinforcements | Confidence |
|---------------|-----------|
| 1 (initial) | 0.60 (inferred) or 0.90 (explicit) |
| 2 | 0.68 / 0.92 |
| 3 | 0.74 / 0.94 |
| 5 | 0.83 / 0.95 |
| 10 | 0.93 / 0.98 |

Converges asymptotically toward 1.0 -- never quite reaches it.

### Decay Scoring

`decay_score()` in `memory.rs:87-97`:
```rust
pub fn decay_score(access_count: i64, accessed_at: Option<i64>) -> f64 {
    let count = (access_count.max(1)) as f64;
    let days = /* seconds since accessed_at / 86400 */;
    count * 0.7_f64.powf(days / 30.0)
}
```

`score_memory()` combines: `confidence × decay` for ranking.

| Access Count | Days Since Access | Decay Score |
|-------------|-------------------|-------------|
| 10 | 0 | 10.0 |
| 10 | 30 | 7.0 |
| 10 | 60 | 4.9 |
| 5 | 0 | 5.0 |
| 5 | 60 | 2.5 |

---

## 10. Security & Sanitization

File: `crates/agent/src/sanitize.rs`

### Prompt Injection Detection

`detect_prompt_injection()` checks 14 regex patterns case-insensitively against memory keys and values:

```
ignore (all)? previous instructions
ignore (all)? above instructions
disregard (all)? previous
forget (all)? previous
you are now (a|an)
new instructions?:
system:\s*you are
assistant:\s*I will
prompt injection
override (system|safety|instructions)
jailbreak
DAN mode
do anything now
act as (if)? (you are|were|a)
```

### Content Limits

| Field | Max Length | Additional Validation |
|-------|-----------|----------------------|
| Key | 128 chars | Control chars stripped (preserves \n) |
| Value | 2048 chars | Control chars stripped (preserves \n), injection patterns blocked |

### User Isolation

All queries are user-scoped via `user_id` column. The unique constraint `(namespace, key, user_id)` prevents cross-user memory leakage.

Usage: `store_facts()` calls `detect_prompt_injection()` on both key and value before storage. Detected entries are skipped with debug logging.

---

## 11. DB Context Assembly (Memory -> Prompt)

File: `crates/agent/src/db_context.rs` (~538 lines)

### DBContext Struct

```rust
pub struct DBContext {
    pub agent: Option<AgentProfile>,          // Agent identity, personality, creature, vibe, rules
    pub user: Option<UserProfile>,            // Name, location, timezone, occupation, interests, goals
    pub preferences: Option<UserPreference>,  // User preferences (language, etc.)
    pub personality_directive: Option<String>, // Synthesized directive from tacit/personality/directive
    pub tacit_memories: Vec<ScoredMemory>,    // Decay-scored, confidence-filtered memories
}
```

### load_db_context Flow

```
load_db_context(store, user_id) -> DBContext
  |
  +-- Load agent profile (agent_profile WHERE id=1)
  |   Defaults: name="Nebo", voice="neutral", length="adaptive"
  |
  +-- Load user profile (user_profiles WHERE user_id=?)
  |
  +-- Load user preferences
  |
  +-- Load personality directive (tacit/personality/directive memory)
  |
  +-- Load scored tacit memories (limit 40)
```

### Two-Pass Loading with Overfetch

From `load_memory_context()` in `memory.rs:519-614`:

- **Pass 1:** `tacit/personality` — overfetch 30, confidence ≥ 0.65, re-rank by `score_memory()`, cap at 10
- **Pass 2:** Other `tacit/*` — overfetch 120, confidence ≥ 0.65, fill remaining up to 40 total
- **Pass 3:** Today's daily memories (limit 20, cap 15)
- **Pass 4:** Entity memories (limit 30, cap 15)

### format_for_system_prompt (9-Section Assembly)

`format_for_system_prompt()` in `db_context.rs:45-224`:

```
1. Identity -- PersonalityPrompt (from preset or custom_personality)
   {name} placeholder replaced with actual agent name

2. Character -- creature, role, vibe, emoji ("business card")
   "You are a [creature]. Your relationship: [role]. Your vibe: [vibe]."

3. Personality (Learned) -- synthesized directive paragraph

4. Communication Style -- voice, formality, emoji, response length, user language

5. User Information -- name, location, timezone, occupation, interests, goals, context

6. Rules -- formatStructuredContent() (JSON sections -> markdown, or raw markdown fallback)

7. Tool Notes -- formatStructuredContent() (same format)

8. What You Know -- scored tacit memories as bullet list
   "These are facts you've learned and stored. Reference them naturally:"
   "- preferences/code-style: Prefers 4-space indentation"

9. Memory Instructions -- recall, search, store usage guide
```

Parts joined with `\n\n---\n\n` separators.

### Known Gaps

- No file-based context fallback (SOUL.md, AGENTS.md, MEMORY.md)
- No onboarding detection (`OnboardingNeeded`)

---

## 12. Static Prompt Assembly

`build_static()` in `crates/agent/src/prompt.rs:376-459`:

### Step 1: DB Context (FIRST -- highest priority position)

Source: `db_context::format_for_system_prompt()`. Full 9-section assembly: identity, character, personality learned, comm style, user info, rules, tool notes, what you know, memory instructions.

### Step 2: Separator

`---` between context and capabilities.

### Step 3: Static Sections (11 constants)

| Section | Variable | Content |
|---------|----------|---------|
| Identity & Prime | `SECTION_IDENTITY` | "You are {agent_name}..." + PRIME DIRECTIVE + BANNED PHRASES |
| Capabilities | `SECTION_CAPABILITIES` | Platform-aware capabilities list |
| Tools Declaration | `SECTION_TOOLS_DECLARATION` | Declares available tools, denies training-data tools |
| Comm Style | `SECTION_COMM_STYLE` | When to narrate vs when to just do |
| Media | `SECTION_MEDIA` | Image and video embed formats |
| Memory Docs | `SECTION_MEMORY_DOCS` | "You have PERSISTENT MEMORY" -- reading/writing/layers |
| Tool Guide | `SECTION_TOOL_GUIDE` | Decision tree for common request patterns |
| Behavior | `SECTION_BEHAVIOR` | Behavioral guidelines -- DO THE WORK, act don't narrate |
| Autonomy | `SECTION_AUTONOMY` | Bias toward action, never ask permission, unlimited context framing |
| System Etiquette | `SECTION_SYSTEM_ETIQUETTE` | Shared computer norms -- clean up, don't steal focus, restore focus |

### Step 4: STRAP Tool Documentation

`build_strap_section()` -- context-activated, loads platform-specific docs from `crates/agent/src/strap/*.txt` (22 files). Core tool docs are in tool schema descriptions. MCP server tools section added if connected.

### Step 5: Plugin Inventory

Lists installed plugin binaries.

### Step 6: Cache Boundary

`CACHE_BOUNDARY` marker injected after static sections. `cache_boundary_offset()` allows programmatic splitting for provider cache optimization.

### Step 7: Skill Catalog + Active Skills

Populated from runner: skill catalog (trigger hints) and active skill content (full SKILL.md templates).

### Step 8: Deferred Tools Listing

`build_deferred_listing()` -- lists available-but-not-loaded tools for on-demand activation.

### Step 9: Tool List

`build_tools_list()` -- single injection of registered tool names. "These are your ONLY tools for this turn."

### Step 10: Model Aliases

From `selector.get_aliases_text()`.

### Step 11: `{agent_name}` Replacement

All occurrences replaced with resolved agent name (default: "Nebo").

### Known Gaps

- No platform capabilities section (`buildPlatformSection()`)
- No app catalog injection
- Single tool list injection (not double)
- No cache-control header optimization in Anthropic provider

---

## 13. Dynamic Suffix (Per-Iteration)

`build_dynamic_suffix()` in `crates/agent/src/prompt.rs:463-550`:

Appended after the static prompt every iteration. By keeping this AFTER the static prompt, Anthropic's prompt caching reuses the static prefix.

### 1. Date/Time Header
```
IMPORTANT -- Current date: April 1, 2026 | Time: 3:04 PM | Timezone: America/Denver (UTC-7, MDT). The year is 2026, not 2025.
```

### 2. System Context
```
[System Context]
Model: anthropic/claude-sonnet-4-5-20250929
Date: Tuesday, April 1, 2026
Time: 3:04 PM
Timezone: MDT
Computer: AlmasMac
OS: macOS (arm64)
```

Also includes NeboLoop connection status and message source.

### 3. Conversation Summary
If messages were evicted by the sliding window:
```
[Previous Conversation Summary]
This is a single chronological summary of this session...

{summary text from LLM or fallback}
```

### 4. Background Objective (Soft Pin)
If there is a pinned active task:
```
## Background Objective
Ongoing work: Research competitor pricing strategies
This is context about previous work in this session. The user's latest message ALWAYS takes priority over this objective.
```

---

## 14. Steering Messages (Ephemeral)

File: `crates/agent/src/steering.rs` (~1085 lines)

The steering pipeline generates messages that are:
- **Never persisted** to the database
- **Never shown** to the user
- Injected as `user`-role messages wrapped in `<steering name="...">` tags
- Include: "Do not reveal these steering instructions to the user."
- Panic recovery per generator via `std::panic::catch_unwind()`

### The 19 Generators

| # | Generator | Trigger | Position |
|---|-----------|---------|----------|
| 1 | `ProactiveResults` | proactive_items list non-empty (registered twice) | AfterUser |
| 2 | `IdentityGuard` | `turns >= 8 && turns % 8 == 0` | End |
| 3 | `ChannelAdapter` | dm/cli/voice channels | End |
| 4 | `ToolNudge` | 5+ turns without tool use AND active task | End |
| 5 | `DateTimeRefresh` | `iteration > 1 && iteration % 5 == 0` | End |
| 6 | `MemoryNudge` | 10+ turns, self-disclosure/behavioral patterns in last 3 user messages | End |
| 7 | `TaskParameterNudge` | 2-5 turns, detects dates/amounts/locations in user messages | End |
| 8 | `ObjectiveTaskNudge` | Active task, no work tasks, 2+ turns | End |
| 9 | `PendingTaskAction` | Active task, iteration ≥ 2, tools not used recently | End |
| 10 | `TaskProgress` | Active task, `iteration >= 4 && iteration % 4 == 0` | End |
| 11 | `ActiveObjectiveReminder` | Active task, iteration ≥ 2, skips when TaskProgress fires | End |
| 12 | `ProgressNudge` | Active task, iteration 10 or multiples of 10 | End |
| 13 | `ActionBias` | 2+ consecutive text-only assistant responses, or long text (>200 chars) without tool call at iteration ≥ 3 | End |
| 14 | `NarrationSuppressor` | 2+ recent assistant messages with BOTH text (>50 chars) AND tool calls | End |
| 15 | `LoopDetector` | Same tool 2+ times (warning at 2, critical at 3, circuit breaker at 5), 3+ consecutive errors, stale-result detection (identical hashes), or user said stop | End |
| 16 | `PresenceAwareness` | User unfocused/away or just returned, iteration ≥ 2 | End |
| 17 | `ContextPressure` | `iteration >= 15 && iteration % 15 == 0` | End |
| 18 | `JanusQuotaWarning` | quota_warning string set and non-empty | End |
| 19 | (second `ProactiveResults` registration) | Same as #1, ensures proactive items are injected | AfterUser |

### NarrationSuppressor (language-agnostic)

Detects when the LLM narrates alongside tool calls (the "Let me archive your emails..." pattern). Counts recent assistant messages that have BOTH text (>50 chars) AND tool calls. If 2+ such turns are found, fires a steering message demanding zero text on tool call turns.

### ActionBias (language-agnostic)

Structural detection — no hardcoded phrases, works in any language:
- **2+ consecutive text-only responses:** Fires when the assistant has responded with text (no tool calls) 2+ times in a row while an active task exists.
- **Long text without tools:** Fires when last assistant response is >200 chars with no tool call, at iteration ≥ 3.

### LoopDetector (enhanced)

Three detection arms:
- **A. Repetitive tool calls:** Warns at 2 consecutive same-tool calls, escalates at 3, circuit-breaks at 5 (lowered from 4/6/8).
- **B. Stale-result detection:** Compares FNV-1a hashes of recent tool results (`recent_tool_result_hashes: Vec<(u64, u64)>` — name hash + content hash). If the last two entries match (same tool, same result), fires a critical steering message.
- **C. Consecutive errors:** 3+ consecutive tool errors triggers steering.
- **D. User stop signal:** Detects "stop", "enough", "quit" in last user message.

### Self-Disclosure & Behavioral Patterns (for MemoryNudge)

**Self-disclosure (15 patterns):**
```
"i am", "i'm", "my name", "i work", "i live", "i prefer", "i like",
"i don't like", "i always", "i never", "i usually", "my wife",
"my husband", "my email", "call me"
```

**Behavioral (8 patterns):**
```
"can you always", "from now on", "don't ever", "stop using",
"start using", "going forward", "please remember", "keep in mind"
```

Fires if **either** list matches in the last 3 user messages.

### Injection

`inject()` handles both `Position::End` (most generators) and `Position::AfterUser` (`ProactiveResults`). All messages wrapped in `<steering>` tags with the `{agent_name}` placeholder replaced.

### Known Gaps

- No `compactionRecovery` generator
- No 30-minute elapsed time check on `DateTimeRefresh`
- `ActionBias` and `LoopDetector` use English for steering messages (acceptable — these are system messages to the LLM, not shown to users)

---

## 15. Context Management Pipeline

File: `crates/agent/src/pruning.rs` (~492 lines)

### Stage 1: Sliding Window (every iteration)

`apply_sliding_window()`:
- `WINDOW_MAX_MESSAGES = 20`, `WINDOW_MAX_TOKENS = 40,000`
- Walk backwards from most recent, accumulate tokens
- Never evict current-run messages (created_at ≥ run_start_time)
- Fix tool-pair boundaries (don't split tool_use from tool_result)

### Stage 2: LLM Summary (on eviction)

When messages are evicted by the sliding window:
1. `build_llm_summary()` generates a summary via sidecar `ChatRequest` to cheapest model
2. Falls back to `build_quick_fallback_summary()` (plaintext from user requests + tool names, no LLM call)
3. Summary stored via `sessions.update_summary()`, appears in dynamic suffix

### Stage 3: Micro-Compact (every iteration, above threshold)

`micro_compact()`:
- Trims old tool results to `[trimmed: {tool} result]`
- Protects 5 most recent tool results (`MICRO_COMPACT_KEEP_RECENT`)
- Min savings threshold: 3,000 tokens (`MICRO_COMPACT_MIN_SAVINGS`)
- Priority ordering: web → file → shell/system → other

### Key Constants

```
CHARS_PER_TOKEN = 4
IMAGE_CHAR_ESTIMATE = 8000
MICRO_COMPACT_MIN_SAVINGS = 3000
MICRO_COMPACT_KEEP_RECENT = 5
WINDOW_MAX_MESSAGES = 20
WINDOW_MAX_TOKENS = 40,000
COMPACTION_MAX_TOKENS = 2000
COMPACTION_CONTENT_CAP = 80,000
```

### Known Gaps

- No two-stage pruning (soft trim + hard clear with head+tail trimming)
- No progressive compaction (keep 10 → 3 → 1)
- No tiered cumulative summaries (Earlier/Recent/Current)
- No file re-injection after eviction
- No base64 image stripping
- Pre-compaction memory flush infrastructure exists (`memory_flush.rs`) but not wired
- Tool failure collection exists (`compaction.rs`) but not integrated into summary

---

## 16. Session Management

File: `crates/agent/src/session.rs` (~248 lines)

### SessionManager

```rust
pub struct SessionManager {
    store: Arc<Store>,
    session_cache: Arc<RwLock<HashMap<String, String>>>,  // session_key -> session_id
}
```

### Operations

| Method | Purpose |
|--------|---------|
| `get_or_create()` | Creates session + companion chat, caches session_key |
| `append_message()` | Validates non-empty, estimates tokens (chars/4), creates via store |
| `get_messages()` | Loads from `chat_messages`, calls `sanitize_messages()` |
| `get_summary()` / `update_summary()` | Rolling summary CRUD (populated by `pruning::build_llm_summary()`) |
| `get_active_task()` / `set_active_task()` / `clear_active_task()` | Objective pin that survives eviction |
| `get_work_tasks()` / `set_work_tasks()` | Work task JSON storage |
| `reset()` / `delete_session()` | Clear and remove |
| `sanitize_messages()` | Removes orphaned tool results without matching `tool_call_id` |

### Known Gaps

- No `Compact()` with `is_compacted` marking
- No `compaction_count` tracking
- No progressive compaction
- Loads all messages, applies sliding window in-memory (no DB-level windowing)

---

## 17. Session Transcript Indexing

> **Status: Dead code.** Fully implemented in `crates/agent/src/transcript.rs` (~156 lines) but never called.

### How It Works (When Wired)

```
Post-eviction hook (not yet implemented)
  -> index_compacted_messages(store, embedding_provider, session_id)
    1. Read last_embedded_message_id (high-water mark)
    2. Fetch messages after that ID (user + assistant roles, non-empty)
    3. Group into blocks of 5 messages (BLOCK_SIZE)
    4. For each block:
       - Concatenate as "role: content" (truncated to 500 chars per message)
       - Chunk via chunk_text_default()
       - Batch embed all chunks
       - Store to memory_chunks (source="session", memory_id=NULL, path=sessionID)
       - Store to memory_embeddings
    5. Update last_embedded_message_id
```

Session chunks participate in hybrid search via LEFT JOIN (alongside memory chunks) and in FTS via `memory_chunks_fts` (with 0.6× dampening).

---

## 18. Special Prompt Paths

### Sub-Agent Prompt

`crates/agent/src/orchestrator.rs` -- minimal focused prompt for sub-agents:
```
You are a focused sub-agent working on a specific task.
Your task: {task}
Guidelines: Focus ONLY on assigned task, work efficiently, use tools...
```

### Workflow Activity Prompt

`crates/workflow/src/engine.rs` -- `build_activity_prompt()`:
- Lean prompt: no steering pipeline, no memory, no personality
- Includes: Execution Rules → Skills → Tools → Task → Steps → Inputs → Prior Results → Browser Guide
- Execution Rules section enforces action-bias (same as chat agent):
  - Call tools immediately, don't narrate
  - Don't re-fetch data you already have
  - Use batch operations when available
  - Track progress, complete the entire task
- Same-tool loop detection: steering injection at 3 consecutive calls of the same tool
- Token budget continuation: `min_iterations` on Activity struct forces continuation on text-only responses

### Unlimited Context Framing

`SECTION_AUTONOMY` in `crates/agent/src/prompt.rs` tells the LLM:
> **Context is unlimited.** Old messages are automatically compacted as needed — you will never run out of space. There is no need to rush, summarize prematurely, or stop early because the conversation is long.

This prevents premature stopping on long sessions.

---

## 18.5 Continuation & Recovery Mechanisms

Three layers prevent the agent from stopping prematurely:

### 1. Max Output Tokens Recovery

**Problem:** When the LLM hits the output token ceiling (typically 4096), the provider sends `finish_reason="length"` or `"max_tokens"`. Without recovery, the agent stops mid-sentence.

**Solution:** `StreamEvent.stop_reason: Option<String>` propagates the finish reason from providers through the event stream. The runner detects truncation and auto-retries:

```
StreamEvent.stop_reason populated by:
  - OpenAI: choice.finish_reason serialized (stop/length/tool_calls)
  - Anthropic: message_delta.stop_reason (end_turn/max_tokens/tool_use)
  - Gemini: candidate.finish_reason mapped (STOP→end_turn, MAX_TOKENS→max_tokens)
  - Ollama: not available (done: bool only)
```

Recovery: up to `MAX_OUTPUT_RECOVERY_ATTEMPTS` (3) retries with steering:
> "Your previous response was cut off by the output token limit. Resume directly from where you stopped."

Takes priority over auto-continuation (checked first in the post-stream decision tree).

### 2. Token Budget Continuation (min_iterations)

**Problem:** The LLM stops after 1-2 iterations even when the task requires more work.

**Solution:** `RunRequest.min_iterations` (runner) / `Activity.min_iterations` (workflow engine) forces the agent to keep iterating. On text-only responses before the budget is met, a steering message is injected:
> "You stopped early but your task is not complete. Keep working."

Default is 0 (disabled). Callers can set it for automations/workflows that need guaranteed multi-step execution.

### 3. Auto-Continuation (existing)

Detects ~20 English patterns indicating the agent paused mid-task (`looks_like_continuation_pause()`: "should i continue", "shall i proceed", etc.) or presented a choice question (`looks_like_choice_question()`). Continues up to `MAX_AUTO_CONTINUATIONS` (5).

Also checks `user_demanded_action()` (~17 imperative patterns in last 2 user messages < 120 chars).

### Decision Priority (post-stream, no tool calls)

```
1. Max output recovery (stop_reason=length/max_tokens) → retry up to 3x
2. Token budget (min_iterations > 0, iteration < min) → force-continue
3. Auto-continuation (pause/choice detected + task context) → continue up to 5x
4. Exit loop (text-only response, agent is done)
```

---

## 19. The Complete Flow: User Message -> LLM Call

```
User sends message (web UI / CLI / channel)
  |
  v
Runner.run(ctx, req)
  | Inject origin into context
  | Get or create session
  | Append user message to session
  | Background: detect_and_set_objective() (fire-and-forget tokio::spawn)
  |
  v
  +-- Load DB context (db_context::load_db_context)
  |    format_for_system_prompt() -> 9-section assembly
  |    load_memory_context() -> decay-scored memories
  |
  +-- Resolve agent name (default: "Nebo")
  +-- Collect tool names, skill catalog, model aliases, plugin inventory
  +-- build_static(PromptContext)
  |
  v
  MAIN LOOP (iteration 1..100)
    |
    +-- Load session messages
    +-- Apply sliding window (20 msgs / 40k tokens)
    |    LLM summary on eviction (or quick fallback)
    |
    +-- Select provider + model
    +-- build_dynamic_suffix(DynamicContext)
    |    Date/time, model context, summary, background objective
    |
    +-- enrichedPrompt = staticSystem + dynamicSuffix
    +-- micro_compact (trim old tool results, 3k min savings)
    |
    +-- Steering pipeline generates messages (19 generators)
    |    inject() into message array
    |
    +-- Build ChatRequest:
    |    System: enrichedPrompt
    |    Messages: windowedMessages
    |    Tools: chatTools
    |    Model: modelName
    |
    +-- provider.stream(ctx, chatReq)
    +-- Process stream events (text, tool calls, errors)
    +-- Capture stop_reason from Done event (length/max_tokens/end_turn)
    +-- Execute tool calls if needed
    +-- If tool calls: hash results for stale detection, continue loop
    +-- If no tool calls:
    |     +-- Max output recovery: if stop_reason=length/max_tokens, retry up to 3x
    |     +-- Token budget: if min_iterations set and not reached, force-continue
    |     +-- Auto-continuation: if looks_like_pause and has_task, continue (up to 5x)
    |     +-- Otherwise: exit loop (text-only response)
  |
  v
  After loop exits:
    memory_debouncer.schedule(session_id)
      -> 5s debounce
      -> extract_facts() -> store_facts()
         With confidence mapping and style reinforcement
```

---

## 20. The Timing Dance

```
Runner.run(ctx, req)
  |
  +-- 1. load_db_context() + load_memory_context()
  |     (reflects extractions from PREVIOUS turns -- one-turn lag)
  |
  +-- 2. build_static(PromptContext)
  |
  v
  MAIN LOOP
    +-- 3. Load session messages
    +-- 4. Sliding window (evicts old messages)
    |     4a. LLM summary on eviction
    +-- 5. build_dynamic_suffix() -- includes summary + objective
    +-- 6. enrichedPrompt = static + dynamic
    +-- 7. micro_compact
    +-- 8. Steering pipeline (19 generators)
    +-- 9. Send to LLM -> stream response, capture stop_reason
    +-- 10. Execute tool calls (hash results for stale detection)
    +-- 10a. Max output recovery (stop_reason=length → retry up to 3x)
    +-- 10b. Token budget continuation (min_iterations → force-continue)
    +-- 10c. Auto-continuation (pause detection → continue up to 5x)
  |
  v
  After loop exits:
    11. memory_debouncer.schedule(session_id)
        -> 5s debounce
        -> extract_facts() -> store_facts() -> style reinforcement

Next Runner.run():
    Step 1 now sees memories from step 11  <- one-turn lag
```

### Visibility Timeline

| Event | Visible in Prompt | Searchable via Agent |
|-------|-------------------|---------------------|
| Idle extraction (step 11) | Next `run()` (step 1) | Immediately after embedding |
| Agent explicit store | Next `run()` | Immediately after embedding |
| Sliding window summary (step 4a) | Same run (step 5) | N/A (in prompt, not searched) |

---

## 21. Memory's Journey Through the Prompt Layers

A single piece of knowledge can appear in up to 4 different places:

```
"User prefers 4-space indentation"
  |
  +-- 1. Static Prompt -> "What You Know" section
  |     (if it's a tacit memory in top 40 by confidence × decay score)
  |
  +-- 2. Dynamic Suffix -> Conversation Summary
  |     (if discussed and captured in LLM summary on eviction)
  |
  +-- 3. Message History -> ToolResult
  |     (if agent called bot(resource: memory, action: search))
  |
  +-- 4. Message History -> Conversation
  |     (if user just said it in the current session)
```

### Connection Point Summary

| Memory Subsystem | Feeds Into Prompt Via | Layer | When |
|---|---|---|---|
| Tacit memories (40 max) | Static prompt → "What You Know" | Tier 1 (cached) | Per-run() |
| Personality directive | Static prompt → "Personality (Learned)" | Tier 1 (cached) | Per-run() |
| Conversation summary | Dynamic suffix → `[Previous Conversation Summary]` | Tier 2 (per-iteration) | After eviction |
| Background objective | Dynamic suffix → `## Background Objective` | Tier 2 (per-iteration) | After objective detection |
| memoryNudge steering | Ephemeral user message | Steering (ephemeral) | Per-iteration (conditional) |
| Hybrid search results | ToolResult in message history | Message history | On-demand |
| Session transcript chunks | Via hybrid search → ToolResult | Message history | On-demand (when indexing wired) |

---

## 22. Maintenance Operations

### Not Yet Implemented

- **Embedding migration** -- detect stale embeddings from previous model, clear and regenerate
- **Embedding backfill** -- generate embeddings for memories without chunks
- **Provisional memory cleanup** -- delete low-confidence memories never reinforced after 30 days
- **Embedding cache eviction** -- no runtime eviction (no startup cleanup of entries older than 30 days)

---

## 23. Performance Characteristics

| Operation | Typical Time |
|-----------|-------------|
| `load_db_context()` + `load_memory_context()` | < 200ms total |
| `format_for_system_prompt()` (9-section assembly) | < 1ms |
| LLM extraction | 5-15s (depends on provider) |
| Memory storage | < 100ms per entry |
| Embedding | 10-30s per fact (async, non-blocking) |
| FTS5 search | 10-50ms |
| Vector search | 50-200ms |
| Hybrid merge | < 1ms |
| Total search | 100-300ms typical |

Memory debouncer: Tokio `JoinHandle` HashMap with CancellationToken, one per session.

---

## 24. Database Schema

### memories

```sql
CREATE TABLE memories (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    namespace TEXT NOT NULL DEFAULT 'default',
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    tags TEXT,           -- JSON array ["tag1", "tag2"]
    metadata TEXT,       -- JSON: {confidence, reinforced_count, first_observed, last_reinforced}
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    accessed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    access_count INTEGER DEFAULT 0,
    user_id TEXT NOT NULL DEFAULT ''
);
-- UNIQUE: idx_memories_namespace_key_user ON (namespace, key, user_id)
```

### memories_fts (FTS5 virtual table)

```sql
CREATE VIRTUAL TABLE memories_fts USING fts5(
    key, value, tags,
    content='memories',
    content_rowid='id'
);
-- Sync triggers: memories_ai (after insert), memories_ad (after delete), memories_au (after update)
```

### memory_chunks

```sql
CREATE TABLE memory_chunks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    memory_id INTEGER REFERENCES memories(id) ON DELETE CASCADE,  -- NULL for session chunks
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

### memory_chunks_fts

```sql
CREATE VIRTUAL TABLE memory_chunks_fts USING fts5(text, content='memory_chunks', content_rowid='id');
```

### memory_embeddings

```sql
CREATE TABLE memory_embeddings (
    id INTEGER PRIMARY KEY,
    chunk_id INTEGER REFERENCES memory_chunks(id) ON DELETE CASCADE,
    model TEXT NOT NULL,
    dimensions INTEGER NOT NULL,
    embedding BLOB NOT NULL,    -- Little-endian f32 byte blob
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

### sessions (memory-relevant columns)

```sql
summary TEXT,
compaction_count INTEGER DEFAULT 0,
memory_flush_at INTEGER,
memory_flush_compaction_count INTEGER,
last_embedded_message_id INTEGER DEFAULT 0,
active_task TEXT
```

### chat_messages

```sql
CREATE TABLE chat_messages (
    id TEXT PRIMARY KEY,
    chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    content TEXT NOT NULL DEFAULT '',
    metadata TEXT,
    tool_calls TEXT,
    tool_results TEXT,
    token_estimate INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    day_marker TEXT
);
```

Messages are immutable and append-only.

---

## 25. Configuration Reference

### Hardcoded Constants (runner)

```rust
DEFAULT_MAX_ITERATIONS = 100
EXTENDED_MAX_ITERATIONS = 200
MAX_AUTO_CONTINUATIONS = 5
MAX_OUTPUT_RECOVERY_ATTEMPTS = 3
TOOL_EXECUTION_TIMEOUT = 300s
WINDOW_MAX_MESSAGES = 20
WINDOW_MAX_TOKENS = 40_000
```

### Hardcoded Constants (workflow engine)

```rust
MAX_ITERATIONS = 50  // per-activity
```

### RunRequest Fields (runner)

| Field | Type | Default | Purpose |
|-------|------|---------|---------|
| `max_iterations` | `usize` | 0 (→ 100) | Max agentic loop iterations |
| `min_iterations` | `usize` | 0 (disabled) | Force continuation until this iteration count |

### Activity Fields (workflow engine)

| Field | Type | Default | Purpose |
|-------|------|---------|---------|
| `min_iterations` | `u32` | 0 (disabled) | Force continuation for workflow activities |

### From DB Tables

| Table | Relevant Columns |
|-------|-----------------|
| `agent_profile` | name, personality_preset, custom_personality, voice_style, response_length, emoji_usage, formality, proactivity, emoji, creature, vibe, role, agent_rules, tool_notes |
| `user_profiles` | display_name, location, timezone, occupation, interests, goals, context, communication_style |
| `personality_presets` | 5 presets (balanced, professional, creative, minimal, supportive) |
| `memories` | Tacit memories injected into prompt (up to 40) |

Both `agent_profile` and `user_profiles` are queried by `db_context::load_db_context()`.

---

## 26. Key Design Decisions

1. **Two-tier split for caching** -- Date/time was the #1 cache buster. Moving it to the dynamic suffix lets Anthropic cache the entire static prefix.

2. **DB context goes FIRST** -- Identity/persona is the most important signal, placed at the highest-priority position for LLM attention.

3. **Steering is ephemeral** -- Never persisted, never shown to user. Prevents context pollution while allowing mid-conversation guidance.

4. **Sliding window replaces progressive compaction** -- Simpler model: keep N most recent messages, summarize evicted ones. LLM summary on eviction preserves context.

5. **Memory budget caps** -- Max 10 personality observations out of 40 total tacit memories. Prevents style notes from crowding out actionable memories.

6. **Automatic extraction handles the common case** -- Idle extraction (5s debounce, last 6 messages) captures most knowledge without explicit agent action.

7. **Confidence as quality gate** -- Inferred facts (< 0.65) stay searchable but don't pollute the system prompt until reinforced.

8. **Reinforcement, not overwrite** -- Style observations increment `reinforced_count` and preserve original text. The first observation is canonical.

9. **Decay creates natural selection** -- Both personality observations (14-day half-life per reinforcement) and tacit memories (access-weighted decay scoring) naturally prune stale knowledge.

10. **Single tool injection** -- Tool names listed once with explicit "ONLY tools" language. Deferred tools listing gives LLM awareness of available-but-not-loaded tools.

---

## 27. Gotchas & Edge Cases

1. **One-turn lag for auto-extracted memories in prompt.** Memories extracted in Turn N appear in the system prompt at Turn N+1. The agent CAN search/recall them in the same turn via hybrid search -- they are immediately searchable.

2. **Personality slot competition.** The 10-slot reservation for `tacit/personality` is shared between style observations AND the directive. With many observations, some will be excluded even though they contributed to the directive. Note: `synthesize_directive()` is dead code so no directives are auto-generated yet.

3. **Session chunks in FTS are dampened.** Session transcript chunks participate in FTS with a 0.6× dampening factor, and in vector search via LEFT JOIN. They are less precise than dedicated memory records.

4. **Summary is flat, not tiered.** Each eviction replaces the previous summary. No tier promotion (Earlier/Recent/Current). Long conversations may lose early context.

5. **memoryNudge vs auto-extraction tension.** The prompt tells the agent "you do NOT need to call bot(action: store)" because auto-extraction handles it. But memoryNudge fires after 10 turns as a fallback when the user is sharing storable information.

6. **Background objective survives eviction but yields to user.** The objective persists in `sessions.active_task` and re-injects every dynamic suffix as a soft pin. User's latest message always takes priority.

7. **Embedding model migration invalidates search.** Changing the embedding model leaves old vectors unsearchable. No migration or backfill mechanism exists yet.

8. **Recall does NOT fall back to search.** Unlike the original design, `recall(key="...")` returns "not found" on miss instead of falling back to hybrid search.

9. **Steering messages are invisible to extraction.** Extraction only sees real messages. Steering messages are ephemeral and never persisted.

10. **Style values are never overwritten.** `store_style_observation()` updates metadata but preserves the original observation text.

11. **Embedding cache has no eviction.** No runtime eviction. Long-running instances accumulate stale entries.

12. **FTS uses OR join.** Queries like "golang tutorials" match either word, not both. This casts a wider net but may return less precise results.

13. **Dead code inventory.** The following modules are fully implemented but never called from the runner: `personality.rs` (synthesis), `memory_flush.rs` (pre-compaction flush), `transcript.rs` (session indexing), `compaction.rs` (tool failure collection), `embed_memories_async()` (memory embedding). These represent ready-to-wire infrastructure.
