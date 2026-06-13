# Memory & Prompt System -- SME Deep-Dive

> **Last updated:** 2026-05-25
>
> **Purpose:** Definitive technical reference for Nebo's entire memory system and system prompt pipeline -- storage, extraction, personality synthesis, hybrid search, embeddings, session transcript indexing, prompt assembly, steering, and context management. Dead code (functions ported but never called from the runner) is explicitly flagged.

---

## Key Files

| File | Purpose | Status |
|------|---------|--------|
| `crates/agent/src/runner.rs` | Agentic loop with provider fallback, objective detection | Active |
| `crates/agent/src/prompt.rs` | `build_static()`, `build_dynamic_suffix()`, STRAP docs, cache boundary | Active |
| `crates/agent/src/db_context.rs` | `DBContext` struct, `load_db_context()`, `format_for_system_prompt()` (9-section assembly) | Active |
| `crates/agent/src/memory.rs` | Extraction, storage, confidence, decay scoring, `load_memory_context()`, `embed_memories_async()` | Active (embedding called from `store_facts()` when provider available) |
| `crates/agent/src/memory_debounce.rs` | Debounced extraction with turn interval + tool call gates | Active |
| `crates/agent/src/memory_flush.rs` | Pre-compaction memory flush with overlap guard + extraction tracking | Active (runner.rs:1062-1075) |
| `crates/agent/src/secret_scan.rs` | Pre-write secret scanner (15 regex patterns for API keys, tokens, private keys) | Wired for automatic extraction; explicit memory tool store still bypasses it |
| `crates/agent/src/summarizer.rs` | `summarize_tool_batch()` + `generate_session_title()` via cheap provider | Active |
| `crates/agent/src/memory_consolidation.rs` | Background memory consolidation — dedup, merge, prune per user_id scope | Active (spawned from server/lib.rs) |
| `crates/agent/src/personality.rs` | `synthesize_directive()` with decay, LLM generation, style loading | Active (runner.rs:2625) |
| `crates/agent/src/steering.rs` | 15 steering generators, format_directives, pipeline, should_force_break | Active |
| `crates/agent/src/pruning.rs` | Sliding window, micro-compact, LLM summary, token estimation | Active |
| `crates/agent/src/compaction.rs` | Tool failure collection, enhanced summary | Dead code |
| `crates/agent/src/session.rs` | SessionManager: CRUD, summary, active task, work tasks | Active |
| `crates/agent/src/search.rs` | Hybrid search: FTS5 + vector + adaptive weights + cosine similarity | Active |
| `crates/agent/src/search_adapter.rs` | `HybridSearchAdapter`: bridges `hybrid_search()` to bot tool's `HybridSearcher` trait | Active |
| `crates/agent/src/chunking.rs` | Sentence-boundary text chunking with overlap | Active |
| `crates/agent/src/transcript.rs` | Session transcript indexing (post-compaction embedding) | Active (called after sliding-window eviction when embedding provider exists) |
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
   - 3.1 [Secret Scanning](#31-secret-scanning)
   - 3.2 [Summarizer](#32-summarizer)
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
14. [Steering Directives (Ephemeral)](#14-steering-directives-ephemeral)
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
28. [Scoped Memory Architecture](#28-scoped-memory-architecture)
29. [Background Memory Consolidation](#29-background-memory-consolidation)

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
|  Memory Extraction (every 3rd turn + 3 tool calls, debounced 5s)             |
|       | LLM extracts 6 fact categories from last 6 messages                  |
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
|  STEERING DIRECTIVES (ephemeral, never persisted)          |
|  Injected into the dynamic suffix, not the message array   |
|                                                            |
|  15 generators, formatted as [Label] content lines         |
|  Appear in "## Agent Directives" section of system prompt  |
+------------------------------------------------------------+
```

The final prompt sent to the LLM: `enrichedPrompt = staticSystem + dynamicSuffix`

This is placed in `ChatRequest.system`. Each provider maps it to their API format:
- **Anthropic:** `params.System = []TextBlockParam{{Text: req.System}}`
- **OpenAI:** `openai.SystemMessage(req.System)` prepended to messages
- **Gemini:** `SystemInstruction` with text part
- **Ollama:** system role message prepended

---

## 2. Storage Model (Taxonomy v2 — 2026-06-12)

> Redesigned per `docs/design/MEMORY_QUALITY.md`. The `daily/` layer is **retired** — date-as-namespace
> duplicated `created_at` and organized memories on the one axis nobody recalls on. Ongoing work lives
> in the topical layer; ephemera are not stored at all (the session layer owns them). The spine:
> session owns the ephemeral → topical layer owns the ongoing → `tacit/` owns the permanent →
> memory consolidation is the only reaper.

### Layers

| Layer | Namespace Pattern | Lifespan | Use Case | Example Keys |
|-------|-------------------|----------|----------|--------------|
| `tacit` | `tacit/preferences`, `tacit/personality`, `tacit/artifacts` | Permanent (with decay for personality) | Long-term preferences, style observations, produced content | `code-indentation`, `style/humor-dry`, `artifact/landing-page-hero-copy` |
| topical (default `project`) | `project` (agent-declarable slugs land in R2) | Retired by consolidation when done/dated | Ongoing work: goals, decisions, constraints, status | `deck-build`, `123-main-st-closing` |
| `entity` | `entity/default` | Permanent | People, places, things with significance beyond the current task | `person/sarah`, `project/nebo` |

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

`task_context` maps to `daily/<date>` (same as `decisions`).

**File:** `crates/agent/src/memory.rs` lines 147-236

---

## 3. Memory Extraction (Automatic)

### Trigger: Debounced Idle Extraction

**When:** After every agentic loop completion (no more tool calls), gated by two thresholds:

1. **5-second debounce** — new messages reset the timer so extraction only runs when idle
2. **Turn interval** — `EXTRACTION_TURN_INTERVAL = 3` — at least 3 turns must pass since last extraction

> The former `MIN_TOOL_CALLS = 3` gate was removed in `487fe179` (tool-less chats never extracted).
> Turn counts are tracked per session in `MemoryDebouncer`; the counter resets when extraction fires.

**Scope:** From the last user message onward (the pre-compaction flush in `memory_flush.rs` covers ALL messages).

**Flow:**
```
runLoop completes (text-only response, no tool calls)
  -> MemoryDebouncer.schedule(session_id, callback)
    -> Increment turn counter; check turn >= 3 AND tool_calls >= 3
    -> If either threshold unmet, return immediately (no timer scheduled)
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

### Extraction Prompt (v2 — 2026-06-12)

The full prompt text lives in `docs/design/MEMORY_QUALITY.md` and `memory.rs::extract_facts`.
Five arrays (the junk-producing `decisions` and `task_context` categories were deleted —
their durable residue is by definition a topic fact):

1. `preferences` — preferences and corrections about how to work (include the why)
2. `styles` — communication/personality observations (key: `style/trait-name`)
3. `entities` — people/orgs/places **with significance beyond the current task**
4. `topics` — ongoing work: goals, decisions, constraints, status; relative dates converted
   to absolute; each fact names its `topic` slug (default `project`; agent-declared in R2)
5. `artifacts` — important produced content

The prompt enforces a durability bar (matters in a month / hard to re-derive / about the user
not the conversation's mechanics), states "empty arrays are normal", and carries a NEVER list:
standalone times/dates/counts/paths, session mechanics, re-derivable facts, secrets, and a
sensitive-personal-information denylist (protected attributes, government IDs, financial
accounts, health, home addresses) unless the user explicitly asks to remember.

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

### FormatForStorage Mapping (v2)

`format_for_storage()` in `memory.rs`:

| Category | Layer | Namespace | IsStyle | Example Key |
|----------|-------|-----------|---------|-------------|
| `preferences` | `tacit` | `tacit/preferences` | false | `code-indentation` |
| `entities` | `entity` | `entity/default` | false | `person/sarah` |
| `styles` | `tacit` | `tacit/personality` | **true** | `style/humor-dry` |
| `artifacts` | `tacit` | `tacit/artifacts` | false | `artifact/hero-copy` |
| `topics` | topic slug | topic slug (validated, fallback `project`) | false | `deck-build` |

### Stage-0 Write Guard (canonical — both write paths)

`tools::memory_guard::stage0_reject(key, value, explicit)` runs before every persist — in
`store_facts()` (automatic extraction) AND the explicit memory-tool store in `bot_tool.rs`
(which previously bypassed secret/injection checks entirely). Rules, in order: `secret`,
`injection`, `bare-number`, `time-fragment`, `path`, `key-blocklist`, `echo`, `too-thin`
(skipped when `explicit` — short stated facts like "favorite color: blue" survive). Rejections
log the rule name via `tracing` for extractor tuning. The module also hosts the canonical
`contains_secret`/`detect_secret`/`detect_prompt_injection` (moved from `agent::secret_scan`,
which was deleted; `agent::sanitize::detect_prompt_injection` is a re-export).

### Confidence Resolution

`resolve_confidence()` maps the `explicit` field:

| Source | Confidence | Meaning |
|--------|-----------|---------|
| `explicit: true` | 0.9 | User directly stated the fact |
| `explicit: false` | 0.6 | Inferred from context/behavior |
| No explicit field | Raw value clamped 0-1 | Fallback |

### Pre-Compaction Memory Flush

`crates/agent/src/memory_flush.rs` (~240 lines) — a second extraction trigger that fires before compaction on ALL messages (not just last 6). Functions `should_run_memory_flush()` and `run_memory_flush()` with dedup guards (compares `compaction_count` vs `memory_flush_compaction_count`). **Called from runner.rs:1062-1075 — guarded by should_run_memory_flush() dedup check.**

**Overlap guard:** If a flush is already running (`FLUSH_IN_PROGRESS` AtomicBool), the new context is stashed as `PendingFlush` (session_id + user_id). When the in-progress flush finishes, it checks for and runs the pending one. This prevents concurrent extractions from corrupting state while ensuring no context is silently dropped.

**Graceful shutdown tracking:** `track_extraction(handle)` registers spawned background tasks (memory extraction, LLM summary, indexing, personality synthesis) in a global `EXTRACTION_HANDLES` registry (`OnceLock<Mutex<Vec<JoinHandle>>>`). Finished handles are pruned on each registration. `drain_extractions()` awaits all in-flight tasks with a 10-second timeout (`DRAIN_TIMEOUT`), called from the server shutdown path.

**Watermark update:** After successful extraction, updates `sessions.memory_flush_compaction_count` to the current `compaction_count`, preventing re-extraction until the next compaction.

### Known Gaps

- No `IsDuplicate()` check before storing (relies on upsert for collision handling; the prompt
  carries an existing-memories section to discourage duplicates)
- No timeout on extraction (provider stream may hang indefinitely)
- ~~Explicit memory tool stores bypass secret/injection checks~~ — **FIXED 2026-06-12**: both
  write paths run `tools::memory_guard::stage0_reject` (see Stage-0 Write Guard above).

**Files:** `crates/agent/src/memory.rs`, `crates/agent/src/memory_debounce.rs`, `crates/agent/src/memory_flush.rs`

---

### 3.1 Secret Scanning

> **Status: Fully wired (2026-06-12).** Canonical implementation moved to
> `crates/tools/src/memory_guard.rs` (lowest shared crate — `agent` depends on `tools`). Both
> write paths run it via `stage0_reject`: automatic extraction (`store_facts()`) and the explicit
> memory-tool store (`bot_tool.rs`). `crates/agent/src/secret_scan.rs` was deleted.

Pre-write scanner for memory persistence. Scans fact values for common credential patterns before storage.

**15 regex patterns:**

| Pattern Name | Regex Match |
|-------------|-------------|
| AWS Access Key | `AKIA[0-9A-Z]{16}` |
| AWS Secret Key | `aws_secret_access_key\s*=\s*\S{20,}` (case-insensitive) |
| OpenAI API Key | `sk-[A-Za-z0-9]{32,}` |
| Anthropic API Key | `sk-ant-[A-Za-z0-9\-]{40,}` |
| GitHub Token | `gh[pousr]_[A-Za-z0-9]{36,}` |
| Generic API Key | `(api[_-]?key|apikey)\s*[:=]\s*['"]?[A-Za-z0-9\-_.]{20,}` (case-insensitive) |
| Bearer Token | `bearer\s+[A-Za-z0-9\-_.]{20,}` (case-insensitive) |
| Private Key | `-----BEGIN (RSA |EC |DSA |OPENSSH )?PRIVATE KEY-----` |
| Slack Token | `xox[bprs]-[A-Za-z0-9\-]{10,}` |
| Google API Key | `AIza[A-Za-z0-9\-_]{35}` |
| Stripe Key | `(sk|pk)_(live|test)_[A-Za-z0-9]{20,}` |
| Twilio Auth Token | `twilio.*[0-9a-f]{32}` (case-insensitive) |
| SendGrid Key | `SG\.[A-Za-z0-9\-_.]{22,}\.[A-Za-z0-9\-_.]{43}` |
| npm Token | `npm_[A-Za-z0-9]{36}` |
| Heroku API Key | `heroku.*[0-9a-f]{8}-...-[0-9a-f]{12}` (case-insensitive) |

**Two public functions:**
- `contains_secret(text) -> bool` — fast check, returns on first match
- `detect_secret(text) -> Option<&str>` — returns the pattern name (e.g., "AWS Access Key") of the first match

**Compiled regex cached via `OnceLock`** — patterns compiled once on first call, reused across all subsequent calls.

**Wiring plan:** Call `contains_secret()` or `detect_secret()` in `store_facts()` before `upsert_memory()`. Skip or redact entries containing secrets, with debug logging of the pattern name.

---

### 3.2 Summarizer

File: `crates/agent/src/summarizer.rs` (~192 lines)

Two utility functions for cheap LLM summarization:

#### `summarize_tool_batch(providers, tool_calls, tool_results, last_assistant_text) -> Option<String>`

Generates a **1-sentence summary** (past tense, max 80 chars) from tool calls and their results. Example output: *"Read auth config and fixed token validation"*.

**Input truncation:**
- Tool input/output: 300 chars each (`IO_TRUNCATE`)
- Assistant intent context: 200 chars (`INTENT_TRUNCATE`)
- Total prompt cap: 2,000 chars (`PROMPT_CAP`)

Uses `pick_cheapest()` (prefers non-Janus providers) with `temperature=0.0`, `max_tokens=100`.

#### `generate_session_title(providers, user_prompt, model) -> Option<String>`

Generates a **3-7 word title** from the first user prompt. Uses `prefer_non_gateway()` to pick cheapest provider. Input truncated to 300 chars. `temperature=0.3`, `max_tokens=30`. Rejects titles >100 chars.

**Error handling:** Both functions are non-critical — errors are logged via `tracing::warn`/`debug` and swallowed (return `None`).

---

## 4. Personality Synthesis

> **Status: Active.** Called from runner.rs:2625 after every turn's memory extraction.

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
4. **Cross-connection verification:** Read-back on a separate pool connection to detect FTS trigger corruption or persistence failures. Returns a specific error with total memory count and restart suggestion if the verify finds nothing.

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

### User Isolation & Scoped Memory

All queries are user-scoped via `user_id` column. The unique constraint `(namespace, key, user_id)` prevents cross-user memory leakage.

Memory uses a 3-tier `user_id` convention for isolation (no schema migration — the column already stores arbitrary strings):

| Tier | user_id Format | Example | Scope |
|------|---------------|---------|-------|
| User (Layer 1) | `"{user_id}"` | `"user123"` | Main Nebo companion — all chats share this |
| Agent (Layer 2) | `"{user_id}:agent:{agent_id}"` | `"user123:agent:brief"` | Agent-wide memories |
| Context (Layer 3) | `"{user_id}:agent:{agent_id}:ctx:{context_id}"` | `"user123:agent:brief:ctx:doc-123"` | Per-context (embed session) isolation |

See [Section 28: Scoped Memory Architecture](#28-scoped-memory-architecture) for full details.

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
load_db_context(store, user_id, inherit_scopes) -> DBContext
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
  +-- Load scored tacit memories (identity slice, limit 8 — abf1ca3b)
       Primary scope: memories WHERE user_id = primary_user_id
       Inherited scopes: memories WHERE user_id = scope.user_id AND namespace LIKE scope.prefix
       Inherited memories scored at 0.8x (rank below local)
       Deduplication by key (highest score wins)
```

The `inherit_scopes` parameter enables multi-layer memory loading for agents with `MemoryConfig`. See [Section 28](#28-scoped-memory-architecture).

### Identity-Slice Loading (replaces the old 4-pass strategy)

Always-on injection is the **identity slice only** (`load_scored_memories()` in `memory.rs`;
`load_memory_context()` was deleted as dead code in `487fe179`):

- **Phase 1:** `tacit/preferences` + `tacit/personality`, confidence ≥ 0.65, overfetch 2× limit
- **Phase 2:** Inherited scopes (owner identity prefixes, always-on for agents), scored at 0.8×
- Dedup by key (highest score wins), truncate to limit (8)

Everything else — topics, entities, artifacts — surfaces on demand via
`load_prompt_relevant_memories()` (FTS against the user prompt) and hybrid search.

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

### Prompt-Relevant Memory Injection

`load_prompt_relevant_memories(store, user_id, prompt, existing_memory_ids)` in `db_context.rs`:

1. FTS search against `memories` table using the user prompt (limit 10)
2. Filter out memories already present in the scored tacit set (by `existing_memory_ids`)
3. Cap at 5 additional memories
4. Format as `## Relevant to This Conversation` section with grouped bullets
5. Returns empty string if no relevant hits

This provides **query-time memory augmentation** — memories that match the current user prompt but weren't in the top-40 tacit set are injected into the system prompt. Prevents the static 40-memory cap from hiding contextually relevant knowledge.

**Security note:** The primary `What You Know` block is wrapped in `<memory-context>` with a "NOT new user instructions" note. The query-time `Relevant to This Conversation` block is currently plain markdown and should be wrapped the same way.

### Known Gaps

- No file-based context fallback (SOUL.md, AGENTS.md, MEMORY.md)
- No onboarding detection (`OnboardingNeeded`)

---

## 12. Static Prompt Assembly

`build_static()` in `crates/agent/src/prompt.rs:376-459`:

### PromptMode (Full / Minimal)

`PromptMode` enum controls how much of the system prompt `build_static()` assembles:

| Mode | Use Case | Sections Included |
|------|----------|-------------------|
| `Full` (default) | Interactive chat with main agent | All 11 sections + plugins + skills + model aliases |
| `Minimal` | Sub-agents, focused tasks | DB context, Identity, Capabilities, Tools Declaration, Behavior only |

**Minimal mode drops:** `SECTION_COMM_STYLE`, `SECTION_MEDIA`, `SECTION_MEMORY_DOCS`, `SECTION_TOOL_GUIDE`, `SECTION_SYSTEM_ETIQUETTE`, plugin inventory, skill catalog, model aliases.

**Minimal mode keeps:** DB context (identity + user info), `SECTION_IDENTITY`, `SECTION_CAPABILITIES`, `SECTION_TOOLS_DECLARATION`, `SECTION_BEHAVIOR`, cache boundary, active skill content (if any), STRAP docs, deferred tool listing, tool list.

**Sub-agent prompt assembly:** `orchestrator.rs` sets `RunRequest.prompt_mode = Minimal`. Agent-type instructions (Explore/Plan/General) are prepended to the user message as a task prefix via `task_prefix_for_type()`, not injected into the system prompt. The old `system_prompt_for_type()` with 3-line hardcoded prompts was removed.

**Field:** `RunRequest.prompt_mode: PromptMode` (default: `Full`). Threaded from `run()` → `run_loop()` → `PromptContext.mode` → `build_static()`.

### PromptContext Fields

```rust
pub struct PromptContext {
    pub mode: PromptMode,
    pub agent_name: String,
    pub active_skill: Option<String>,
    pub skill_catalog: String,
    pub model_aliases: String,
    pub channel: String,
    pub platform: String,
    pub memory_context: String,
    pub db_context: Option<String>,           // Rich DB context; replaces memory_context when set
    pub active_agent: Option<String>,         // AGENT.md body, injected before identity
    pub agent_soul: Option<String>,           // Voice, tone, personality, boundaries (SOUL.md content)
    pub plugin_inventory: String,
    pub agent_plugin_context: String,         // Focused context for agent-required plugins
    pub agent_self_context: String,           // Agent's workflows, skills, self-awareness
    pub agent_catalog: String,                // Compact installed agents listing
    pub research_prompt: Option<String>,      // Injected when bot(action: "research") activates
    pub context_file: Option<String>,         // Workspace context from .nebo.md or NEBO.md
}
```

### DynamicContext Fields

```rust
pub struct DynamicContext {
    pub provider_name: String,
    pub model_name: String,
    pub active_task: String,
    pub summary: String,
    pub neboai_connected: bool,
    pub channel: String,
    pub work_tasks: Vec<WorkTask>,
    pub tool_doc_cache: Vec<(String, String)>,  // Survives sliding window eviction (max 8k chars)
    pub steering_directives: String,
    pub proactive_context: String,
    pub user_timezone: Option<String>,          // IANA timezone override for date/time
}
```

### Model-Specific Guidance

`build_model_specific_guidance(provider, model)` injects provider-specific enforcement into the dynamic suffix:

| Provider | Sections Added |
|----------|---------------|
| Anthropic (Claude) | None (follows system prompt natively) |
| OpenAI (GPT) | Tool-Use Enforcement + GPT Execution Guidance (tool_persistence, mandatory_tool_use, act_dont_ask, prerequisite_checks, verification, missing_context) |
| Google (Gemini) | Tool-Use Enforcement + Operational Directives (absolute paths, verify_first, dependency_checks, conciseness, parallel_tool_calls, non-interactive_commands, keep_going) |
| Janus | Tool-Use Enforcement (routes to non-Claude models) |
| Ollama | Tool-Use Enforcement |

### Step 1: DB Context (FIRST -- highest priority position)

Source: `db_context::format_for_system_prompt()`. Full 9-section assembly: identity, character, personality learned, comm style, user info, rules, tool notes, what you know, memory instructions.

### Step 2: Separator

`---` between context and capabilities.

### Step 3: Static Sections (9 constants)

| Section | Variable | Content |
|---------|----------|---------|
| Identity | `SECTION_IDENTITY` | "You are {agent_name}..." + Execution Principles (bias toward action, ask only when stuck, finish the job, context unlimited) |
| Capabilities | `SECTION_CAPABILITIES` | Platform-aware capabilities list |
| Tools Declaration | `SECTION_TOOLS_DECLARATION` | Declares available tools, denies training-data tools |
| Comm Style | `SECTION_COMM_STYLE` | Silent tool execution, milestones not steps, no spam, no sycophancy |
| Media | `SECTION_MEDIA` | Image and video embed formats |
| Memory Docs | `SECTION_MEMORY_DOCS` | "You have PERSISTENT MEMORY" -- reading/writing/layers |
| Tool Guide | `SECTION_TOOL_GUIDE` | Non-obvious routes only (file vs shell, browser profiles, ask tool, scheduling, work tasks, skill catalog) |
| Behavior | `SECTION_BEHAVIOR` | Execution rules, safety, conversation, single conversation awareness, code, "What You Are NOT" |
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

`build_dynamic_suffix()` in `crates/agent/src/prompt.rs:509-694`:

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

Also includes NeboAI connection status and message source.

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
## Previous Objective (may be stale)
Earlier in this session, the user was working on: Research competitor pricing strategies
CRITICAL — Task switching rules:
- The user's LATEST message defines what they want NOW. Not this objective.
- People switch tasks without announcing it...
- ONLY continue this objective if the user explicitly references it.
```

### 5. Current Work Tasks
Lists active work tasks with status icons (`completed`, `in_progress`, `pending`). Includes "Do NOT recreate resources that already exist" guard.

### 6. Cached Tool Documentation
Tool docs that survive sliding window eviction (max 8,000 chars total). Entries are `(key, content)` pairs cached by the runner during tool calls.

### 7. Steering Directives + Proactive Context
- `steering_directives`: formatted output from `steering::format_directives()` — `## Agent Directives` section
- `proactive_context`: `[Background Results]` section from proactive inbox items

---

## 14. Steering Directives (Ephemeral)

File: `crates/agent/src/steering.rs` (~950 lines)

Steering directives are:
- **Never persisted** to the database
- **Never shown** to the user
- Injected into the **dynamic suffix** (not as user-role messages) via `format_directives()` → `## Agent Directives` section
- Formatted as `[Label] content` lines
- `{agent_name}` placeholder replaced per directive
- Panic recovery per generator via `std::panic::catch_unwind()`

### Provider-Specific Skip Rules

```rust
let is_claude = ctx.provider_id == "anthropic";  // Direct Anthropic only
let is_ollama = ctx.provider_id == "ollama";

// Claude skips: narration_suppressor, output_discipline, repetition_detector, ask_tool_nudge
// (Claude follows system prompt well without enforcement)
// NOTE: Janus is NOT treated as Claude — it may route to GPT/Gemini

// Ollama skips: janus_quota_warning
```

### The 15 Generators

| # | Generator | Trigger | Priority | Skipped for |
|---|-----------|---------|----------|-------------|
| 1 | `IdentityGuard` | `turns >= 8 && turns % 8 == 0` | 5 | — |
| 2 | `ChannelAdapter` | dm/cli/voice channels | 3 | — |
| 3 | `ToolNudge` | 5+ turns without tool use AND active task | 7 | — |
| 4 | `PendingTaskAction` | Active task, iteration ≥ 2, tools not used recently | 8 | — |
| 5 | `OutputDiscipline` | Verbose output correction (last response >300 chars) | 9 | Claude |
| 6 | `NarrationSuppressor` | 1+ recent assistant messages with BOTH text (>50 chars) AND tool calls | 8 | Claude |
| 7 | `RepetitionDetector` | iteration ≥ 3, 40%+ trigram overlap between consecutive responses (>100 chars) | 9 | Claude |
| 8 | `LoopDetector` | Same-tool loops (3+), stale results, ping-pong, budget pressure, user stop | 6-10 | — |
| 9 | `ErrorRecovery` | 1+ consecutive all-error iterations (priority 9→10) | 9-10 | — |
| 10 | `PresenceAwareness` | User unfocused/away or just returned, iteration ≥ 2 | 4 | — |
| 11 | `ContextPressure` | `iteration >= 15 && iteration % 15 == 0` | 6 | — |
| 12 | `JanusQuotaWarning` | quota_warning string set and non-empty | 7 | Ollama |
| 13 | `TaskTrackingNudge` | iteration == 1, no work tasks, multi-step complexity detected in user prompt | 5 | — |
| 14 | `TaskCompletionNudge` | iteration ≥ 3, all tasks pending despite active tool use | 5 | — |
| 15 | `AskToolNudge` | Last assistant response has question mark or choice phrases, no ask tool call | 7 | Claude |

> **Removed:** `AutomationSpeed` (was #10, efficiency nudge for wait/read/single-tool patterns).

Proactive results are handled separately in `Pipeline::generate()` — proactive items are formatted into a `proactive_context` output (not a steering directive) and injected as `[Background Results]` in the dynamic suffix.

### OutputDiscipline (conditional only)

Fires only when last assistant response exceeds 300 chars (non-Claude models). Measured tone:
- Zero text alongside tool calls
- 1-3 sentences max for results
- Never repeat information already said
- Handle errors silently or try different approach

Previously had an always-on "Tool Enforcement" arm (150+ words every iteration) — removed as redundant with PendingTaskAction and the consolidated prompt.

### TaskTrackingNudge (multi-step task detection)

Fires only on iteration 1 (first response to user message). Conditions:
- No work tasks exist yet
- User prompt contains multi-step complexity signals ("and then", "after that", "first", "next", "finally", "step 1", "1.", "2.", "3.", "multiple", "each", "all of", etc.)

Steers the LLM to create work tasks via `bot(resource: "task")` before executing. Priority 5.

### TaskCompletionNudge (progress tracking)

Fires at iteration ≥ 3 when work tasks exist but ALL are still "pending" despite active tool use. Steers the LLM to update task status (`in_progress` → `completed`) as it works. Priority 5.

### AskToolNudge (interactive input enforcement)

Fires when last assistant message contains question marks or choice phrases ("which do you prefer", "what would you like", etc.) without an ask tool call. Steers LLM to use `agent(resource: "ask")` instead of plain text questions. Skipped for Claude.

### NarrationSuppressor (language-agnostic)

Detects when the LLM narrates alongside tool calls (the "Let me archive your emails..." pattern). Counts recent assistant messages (last 6) that have BOTH text (>50 chars) AND tool calls. Fires on **1st narrating turn** (was 2 — too late for GPT). Demands zero text before, between, or after tool calls.

### RepetitionDetector (NEW — trigram-based)

Fires at iteration ≥ 3. Compares 3-gram overlap between consecutive assistant responses (>100 chars each). If 40%+ trigrams are shared, the LLM is repeating itself. Demands either a new tool action or a final 1-sentence answer.

### LoopDetector (5 detection arms)

Uses hash-based detection: `recent_tool_result_hashes: Vec<(u64, u64, u64)>` — (name_hash, args_hash, result_hash). Last 10 kept. Correctly distinguishes `web(navigate)→web(click)→web(fill)` (legitimate work) from `web(search, "flights")×5` (loop).

- **A. Same-tool-same-args:** Caution at 2 calls (priority 8), warning at 3+ calls (priority 10) with skill catalog and advisor suggestions.
- **B. Stale-result:** Same tool + same args + same result → priority 9. Strongest loop signal.
- **C. Ping-pong:** A→B→A→B alternating pattern (4 calls minimum) → priority 9.
- **D.** (Removed — ErrorRecovery generator handles consecutive errors at 1+, circuit breaker fires at 3.)
- **E. Budget pressure:** Warning at 70% (priority 6), critical at 90% (priority 10) of MAX_ITERATIONS (100).
- **F. User stop signal:** Detects "stop", "cancel", "abort", "halt", "quit", "enough", "break out" in last 3 user messages (<80 chars).

**Plugin-specific recovery:** When the looping tool is "plugin", extra guidance is appended suggesting `--help` flags, parameter format checks, and simpler command variants.

### ErrorRecovery (early error intervention)

Fires after the FIRST consecutive all-error iteration. Prevents blind retries of failing tool calls:
- **1 error iteration** (priority 9): "Do NOT retry with same parameters. Read the error. Fix params, try --help, or try different approach."
- **2+ error iterations** (priority 10): "CRITICAL. STOP retrying. Tell user what's wrong."

This fills a gap where previously 2 error iterations had no error-specific steering.

### `should_force_break()` (Runner Hard Stop)

Called by the runner BEFORE the next LLM call. Returns `Some(reason)` to halt the loop:
- 3+ consecutive error iterations (was 5)
- 4+ same-tool-same-args calls, hash-based (was 6)
- User stop request (iteration > 2)

### Known Gaps

- No `compactionRecovery` generator
- All steering messages use English (acceptable — system messages to the LLM, not shown to users)
- Budget pressure in LoopDetector hardcodes MAX_ITERATIONS=100 (should read from runner config)

---

## 15. Context Management Pipeline

File: `crates/agent/src/pruning.rs` (~492 lines)

### Stage 1: Sliding Window (every iteration)

`apply_sliding_window()`:
- Default `max_tokens = 40,000` (`DEFAULT_WINDOW_MAX_TOKENS`), hard message cap `MAX_MESSAGE_COUNT = 80`
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
- Replaces old tool results with an **informative neutral summary** via `build_tool_summary()` (pruning.rs:453) — e.g. `[system:shell] ls Desktop (68 lines)`, `[web:search] 'flights' (12 results)`, `[web:navigate] url — <page visual>` — instead of the old generic `[trimmed: {tool} result]`. Per-tool intelligence preserves what the call did without keeping the full payload.
- **Error decay (context-pollution fix):** the compacted result is always rewritten with `is_error: false` (pruning.rs:242, 252, 362, 372). Once a failed tool call ages out of the protected-recent window, it no longer reads as a failure to the model. This is the structural half of the "hallucinated failure from context pollution" fix (see `docs/testing/document-upload-scenario.md` Bug 1) — it prevents accumulated past failures from making the model disbelieve its own later successes. Note: it only protects *future* turns; an already-poisoned session must be cleared with `/new`.
- Preserves original `tool_call_id`s on the rewritten result so the orphan filter in `build_messages` still pairs compacted results with their `tool_use`.
- Protects 3 most recent tool results (`MICRO_COMPACT_KEEP_RECENT`)
- Min savings threshold: 1,000 tokens (`MICRO_COMPACT_MIN_SAVINGS`)
- Priority ordering: web → file → shell/system → other
- **Count-based trigger:** If >4 compactable tool results (`MICRO_COMPACT_COUNT_TRIGGER`), strip aggressively regardless of age (keep only `MICRO_COMPACT_KEEP_RECENT` most recent)
- **Time-based trigger:** If >5 minutes (`TIME_BASED_GAP_THRESHOLD_SECS = 300`) since last activity, keep only 1 recent result (`TIME_BASED_KEEP_RECENT = 1`). Matches Claude Code parity.

### Runner-Side Tool-Result Hygiene (`runner.rs`)

Complements pruning. Applied as each iteration's tool results are saved (runner.rs ~3160-3239):
- **Error cap:** error results over `ERROR_CAP = 10_000` chars are truncated to first 5K + last 5K with a `[N characters truncated]` marker (prevents error dumps from dominating context).
- **Success cap:** success results over `RESULT_CAP = 30_000` chars are persisted to `/tmp/nebo-tool-results/<uuid>.txt` and replaced with a 4K preview + a `read` path. `UNIVERSAL_TOOL_RESULT_CAP = 100_000` is the final hard ceiling.
- **Empty-result guard:** an empty non-error result becomes `({tool} completed with no output)` so the model doesn't read it as end-of-output.
- **Comm delivery note:** for `Origin::Comm` runs, any tool result that carried an `image_url` (recorded in `had_image` *before* the sidecar nulls it) gets `✓ Screenshot captured and will be delivered as an attachment…` appended — so the model knows the attachment is on its way (`document-upload-scenario.md` Bug 2).
- **Duplicate-read note:** re-reading a file already read this session appends `(Note: this file was already read earlier in this session)`.

### Hard Message Cap

`MAX_MESSAGE_COUNT = 80` — regardless of token budget, the sliding window enforces a hard cap on message count. Even short messages add serialization and attention overhead at the provider. `80 msgs × ~120 tokens/msg = ~9,600 tokens`, well within budget. If both total tokens fit and message count is under 80, the sliding window short-circuits entirely.

### Graduated Context Thresholds

`ContextThresholds` struct computes graduated warning/error/auto_compact thresholds from the model's context window:

```rust
pub struct ContextThresholds {
    pub warning: usize,       // Micro-compact activates above this
    pub error: usize,         // Log warning about context size
    pub auto_compact: usize,  // Trigger full compaction
}
```

`ContextThresholds::from_context_window(context_window, prompt_overhead)`:
- `auto_compact = min(effective, 500_000)`
- `error = auto_compact - 10,000`
- `warning = auto_compact - 20,000`
- Minimums: `warning >= 40,000`, `error >= 50,000`

The caller passes `ContextThresholds::auto_compact` as `max_tokens` to `apply_sliding_window()`, so eviction only fires when approaching the context limit (like Claude Code's ~83% threshold).

### Key Constants

```
CHARS_PER_TOKEN = 4
IMAGE_CHAR_ESTIMATE = 8000
MICRO_COMPACT_MIN_SAVINGS = 1,000
MICRO_COMPACT_KEEP_RECENT = 3
MICRO_COMPACT_COUNT_TRIGGER = 4
TIME_BASED_GAP_THRESHOLD_SECS = 300 (5 minutes)
TIME_BASED_KEEP_RECENT = 1
MAX_MESSAGE_COUNT = 80
DEFAULT_WINDOW_MAX_TOKENS = 40,000
COMPACTION_MAX_TOKENS = 4000
COMPACTION_CONTENT_CAP = 80,000
```

### Known Gaps

- No two-stage pruning (soft trim + hard clear with head+tail trimming)
- No progressive compaction (keep 10 → 3 → 1)
- No tiered cumulative summaries (Earlier/Recent/Current)
- No file re-injection after eviction
- No base64 image stripping
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

> **Status: Active.** Fully implemented in `crates/agent/src/transcript.rs` and called from `runner.rs` after sliding-window eviction when an embedding provider is configured.

### How It Works

```
Post-eviction hook in runner.rs
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

`crates/agent/src/orchestrator.rs` -- sub-agents use `PromptMode::Minimal` (see §12), which assembles a proper system prompt with identity, capabilities, tools declaration, and behavior sections -- but drops heavy sections like comm style, media, memory docs, tool guide, autonomy, and etiquette (~2.7k tokens saved).

Agent-type constraints are prepended to the user message as a task prefix via `task_prefix_for_type()`:
- **Explore:** `[EXPLORATION agent — search, read, research only. Do NOT modify files...]`
- **Plan:** `[PLANNING agent — analyze, break down steps, identify files...]`
- **General:** `[Execute the task using whatever tools are needed.]`

Sub-agents also set `skip_memory_extract: true` and `channel: "subagent"` (steering generators skip this channel).

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

`SECTION_IDENTITY` in `crates/agent/src/prompt.rs` tells the LLM (under "Execution Principles"):
> **Context is unlimited.** Old messages are automatically compacted as needed — you will never run out of space. There is no need to rush, summarize prematurely, or stop early because the conversation is long.

This prevents premature stopping on long sessions.

---

## 18.5 Continuation & Recovery Mechanisms

Three layers prevent the agent from stopping prematurely.

### Design Philosophy

**The primary continuation signal is the presence of tool_use blocks, NOT pattern
matching on response text.** This aligns with how Claude Code, OpenClaw, and
Hermes Agent all handle continuation — none of them pattern-match the assistant's
text to decide whether to keep going.

Industry comparison (April 2026):

| | Claude Code | OpenClaw | Hermes Agent | Nebo |
|---|---|---|---|---|
| Continue when | tool_use blocks | tool_use + error recovery | tool_calls present | tool_calls + work tasks |
| Text pattern matching | No | No | No | **Removed** (was causing loops) |
| Loop detection | Max turns only | Per-tool call hash tracking (warn@10, block@20, breaker@30) | Per-tool (read/search) blocking at 3rd repeat | Cycle detection on work-task continuation |
| Auto-continue (no tools) | Token budget only (diminishing returns guard) | No | Only on `finish_reason=length` (3 retries) | Only with explicit incomplete work tasks |

**History:** Nebo previously used `looks_like_continuation_pause()` (37 English
patterns like "should I continue", "would you like me to") and
`looks_like_choice_question()` (16 patterns) to detect mid-task pauses. This
caused false-positive loops — e.g. "Would you like me to pull the full daily
stats summary instead?" matched "would you like me to" and triggered 5
auto-continuations of the identical response. Both functions were removed.

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

### 3. Work-Task Auto-Continuation

Only fires when **all** of these are true:
- There is task context (`active_task` set, or user demanded action, or incomplete work tasks)
- There are explicitly **incomplete work tasks** (`work_tasks` with status != "completed")
- Auto-continuation budget not exhausted (`auto_continuations < auto_limit`)
- Response is non-empty

**Cycle detection:** Tracks `prev_auto_content`. If the current response is
identical to the previous auto-continued response, the agent is stuck in a loop
and continuation is aborted. This prevents the 5x-repeat bug even if work tasks
remain incomplete.

`user_demanded_action()` (~17 imperative patterns in last 2 user messages < 120 chars)
acts as an implicit task signal when objective detection hasn't run yet.

### Decision Priority (post-stream, no tool calls)

```
1. Max output recovery (stop_reason=length/max_tokens) → retry up to 3x
2. Token budget (min_iterations > 0, iteration < min) → force-continue
3. Work-task continuation (incomplete tasks + cycle guard) → continue up to limit
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
    +-- micro_compact (trim old tool results, 1k min savings)
    |
    +-- Steering pipeline generates directives (15 generators)
    |    format_directives() into dynamic suffix
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
    |     +-- Work-task continuation: if incomplete tasks + not a cycle, continue
    |     +-- Otherwise: exit loop (text-only response, agent is done)
  |
  v
  After loop exits:
    memory_debouncer.schedule(session_id)
      -> Turn interval (3) + tool call threshold (3) checked first
      -> If thresholds met: 5s debounce -> extract_facts() -> store_facts()
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
    +-- 8. Steering pipeline (15 generators)
    +-- 9. Send to LLM -> stream response, capture stop_reason
    +-- 10. Execute tool calls (hash results for stale detection)
    +-- 10a. Max output recovery (stop_reason=length → retry up to 3x)
    +-- 10b. Token budget continuation (min_iterations → force-continue)
    +-- 10c. Work-task auto-continuation (incomplete tasks + cycle guard → continue up to limit)
  |
  v
  After loop exits:
    11. memory_debouncer.schedule(session_id)
        -> Turn interval (3) + tool call threshold (3) checked
        -> If thresholds met: 5s debounce
        -> extract_facts() -> store_facts() -> style reinforcement

Next Runner.run():
    Step 1 now sees memories from step 11  <- one-turn lag (at minimum 3-turn lag due to extraction interval)
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
| Steering directives | Dynamic suffix `## Agent Directives` | Steering (ephemeral) | Per-iteration (conditional) |
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

6. **Automatic extraction handles the common case** -- Idle extraction (every 3rd turn with 3+ tool calls, 5s debounce, last 6 messages) captures most knowledge without explicit agent action. The turn and tool call gates reduce LLM calls on short Q&A exchanges.

7. **Confidence as quality gate** -- Inferred facts (< 0.65) stay searchable but don't pollute the system prompt until reinforced.

8. **Reinforcement, not overwrite** -- Style observations increment `reinforced_count` and preserve original text. The first observation is canonical.

9. **Decay creates natural selection** -- Both personality observations (14-day half-life per reinforcement) and tacit memories (access-weighted decay scoring) naturally prune stale knowledge.

10. **Single tool injection** -- Tool names listed once with explicit "ONLY tools" language. Deferred tools listing gives LLM awareness of available-but-not-loaded tools.

---

## 27. Gotchas & Edge Cases

1. **One-turn lag for auto-extracted memories in prompt.** Memories extracted in Turn N appear in the system prompt at Turn N+1. The agent CAN search/recall them in the same turn via hybrid search -- they are immediately searchable.

2. **Personality slot competition.** The 10-slot reservation for `tacit/personality` is shared between style observations AND the directive. With many observations, some will be excluded even though they contributed to the directive. Note: `synthesize_directive()` is called from runner.rs:2625 after every turn's memory extraction.

3. **Session chunks in FTS are dampened.** Session transcript chunks participate in FTS with a 0.6× dampening factor, and in vector search via LEFT JOIN. They are less precise than dedicated memory records.

4. **Summary is flat, not tiered.** Each eviction replaces the previous summary. No tier promotion (Earlier/Recent/Current). Long conversations may lose early context.

5. **No memory nudge.** The `MemoryNudge` generator was removed. Auto-extraction is now the sole mechanism. The prompt tells the agent "you do NOT need to call bot(action: store)" — the agent can still explicitly store via the memory tool, but there is no steering nudge to do so.

6. **Background objective survives eviction but yields to user.** The objective persists in `sessions.active_task` and re-injects every dynamic suffix as a soft pin. User's latest message always takes priority.

7. **Embedding model migration invalidates search.** Changing the embedding model leaves old vectors unsearchable. No migration or backfill mechanism exists yet.

8. **Recall does NOT fall back to search.** Unlike the original design, `recall(key="...")` returns "not found" on miss instead of falling back to hybrid search.

9. **Steering messages are invisible to extraction.** Extraction only sees real messages. Steering messages are ephemeral and never persisted.

10. **Style values are never overwritten.** `store_style_observation()` updates metadata but preserves the original observation text.

11. **Embedding cache has no eviction.** No runtime eviction. Long-running instances accumulate stale entries.

12. **FTS uses OR join.** Queries like "golang tutorials" match either word, not both. This casts a wider net but may return less precise results.

13. **Dead code inventory.** `compaction.rs` (tool failure collection) is still fully implemented but not integrated into the active runner path. `transcript.rs` and `embed_memories_async()` are now wired.

14. **Removed steering generators.** The following generators were removed from earlier versions: `ProactiveResults` (×2), `DateTimeRefresh`, `MemoryNudge`, `TaskParameterNudge`, `ObjectiveTaskNudge`, `TaskProgress`, `ActiveObjectiveReminder`, `ProgressNudge`, `AutomationSpeed`. Proactive results are now handled inline in `Pipeline::generate()`, not as a registered generator. Date/time is always in the dynamic suffix (no refresh needed). Task-related nudges were consolidated into `PendingTaskAction`. `AutomationSpeed` was replaced by `TaskTrackingNudge` and `TaskCompletionNudge`.

---

## 28. Hermes Agent Comparison & Improvement Roadmap

> **Sources:** `hermes-agent/docs/SME_MEMORY_SYSTEM.md` and `SME_CONTEXT_MANAGEMENT.md` (2026-05-13)
> Hermes is a Python-based AI agent by Nous Research. This section documents what Nebo can learn from its memory, prompt, and context management architecture.

### Hermes Architecture (Brief)

**Memory:** Three tiers — Active (MEMORY.md + USER.md frozen snapshot, ~1,300 tokens), Episodic (FTS5 session search + LLM summarization), Extended (pluggable MemoryProvider ABC, 8 bundled providers).

**Prompt:** 12-layer system prompt built once per session. Dynamic content (memory recall, context references, plugin injections) injected into user messages at API-call time, never into the cached system prompt. Anthropic `system_and_3` cache breakpoints.

**Compression:** 5-phase pipeline — (1) Tool output pruning with smart per-tool summaries, (2) Boundary determination with token-budget tail protection, (3) LLM summarization with structured 12-section handoff template, (4) Tool pair integrity repair, (5) Assembly + anti-thrashing. Session splitting on compression with lineage tracking. Iterative summary updates on re-compression.

### What Nebo Does Better

| Capability | Nebo | Hermes |
|-----------|------|--------|
| Automatic fact extraction | LLM extracts 6 categories from last 6 msgs, debounced 5s | Manual only — agent must call memory tool |
| Search | Hybrid FTS5 + vector with adaptive query weighting | FTS5 only |
| Confidence scoring | explicit=0.9, inferred=0.6, reinforcement boost | None |
| Decay scoring | `access_count × 0.7^(days/30)` | None |
| Style reinforcement | Tracks observations, boosts confidence asymptotically | None |
| Dynamic prompt augmentation | Per-iteration `load_prompt_relevant_memories()` via FTS | All entries always injected (no filtering) |
| Pre-compaction extraction | `memory_flush.rs` extracts from ALL messages before eviction | Built-in provider doesn't implement `on_pre_compress()` |
| Personality synthesis | `personality.rs` — LLM synthesizes directive from style observations | None |
| Per-iteration dynamic suffix | `build_dynamic_suffix()` with date/time, summary, tasks, steering | System prompt fully frozen per session |
| Steering directives | 15 behavioral generators (loop detection, error recovery, etc.) | None — no equivalent behavioral steering |
| Per-agent persona | AGENT.md (three-tier: embedded → marketplace → user) + AgentProfile (20 fields) | Single SOUL.md file, no per-agent profiles |
| Plugin ecosystem | Native binary plugins (PLUGIN.md + plugin.json) with capabilities, auth, events, hooks, providers | Memory provider ABC only — no general plugin system |
| Multi-agent architecture | Agent registry, sub-agents, delegation, per-agent sessions | Single agent only |

### What Hermes Does Better — Memory

| Gap | Hermes Pattern | Nebo Status |
|-----|---------------|-------------|
| **Context fencing** | `<memory-context>` XML tags + "NOT new user input" system note | Partially done — primary `What You Know` is fenced; prompt-relevant memories are not fenced yet |
| **Memory overflow management** | Hard char limits (2,200 + 1,375) with overflow error + usage stats | No limits — DB grows unbounded, 40-entry prompt cap hides old memories |
| **Usage metrics in prompt** | `[67% — 1,474/2,200 chars]` in memory header | Agent has zero visibility into memory count |
| **LLM-summarized session search** | FTS5 results summarized by auxiliary LLM per-session | Brute-force substring matching in bot_tool.rs session "query" |
| **Memory diff between sessions** | Compares loaded entries vs previous snapshot | No diff — agent doesn't know what changed |
| **Memory provider plugins** | `MemoryProvider` ABC with lifecycle hooks | Single built-in system — but Nebo's plugin system (PLUGIN.md) already supports capability types (tools, hooks, providers, events). A memory provider could be added as a new plugin capability type rather than a separate ABC. |

### What Hermes Does Better — Prompt & Security

> **Note:** Hermes is a single-agent CLI tool that runs inside project directories. Nebo is a multi-agent desktop companion running at the machine level. Patterns like subdirectory hint discovery and project context file walking up to git root don't directly apply, but context references (`@file:`, `@url:`, etc.) are universally useful — users reference files, URLs, and documents regardless of whether there's a project root.

| Gap | Hermes Pattern | Nebo Status |
|-----|---------------|-------------|
| **Cache breakpoints** | Anthropic `system_and_3` — 4 explicit `cache_control` markers | `CACHE_BOUNDARY` marker but no explicit API-level breakpoints |
| **Context references** | `@file:`, `@folder:`, `@diff`, `@url:` injected into user messages at API-call time with token budget protection (hard limit 50%, soft limit 25%) | No equivalent — users can't inline file/URL content into conversations |
| **Secret redaction** | 30+ regex patterns masking API keys, JWTs, private keys before summarization/logs | `secret_scan.rs` blocks automatic memory persistence for common credential patterns; explicit memory tool stores and compression/log redaction still need coverage |

### What Hermes Does Better — Context Compression

| Gap | Hermes Pattern | Nebo Status |
|-----|---------------|-------------|
| **Toolset-based schema filtering** | `get_tool_definitions(enabled_toolsets=..., disabled_toolsets=...)` + platform-specific configs | Contextual filtering is active; remaining gap is tighter per-agent/activity scoping and continued schema reduction |
| **Tool output pruning** | Smart per-tool summaries: `[terminal] ran npm test → exit 0, 47 lines` | `micro_compact` trims to `[trimmed: {tool} result]` — no tool-specific intelligence |
| **Structured summary template** | 12-section handoff (Active Task, Completed Actions, Key Decisions, Remaining Work, etc.) | Single LLM summary or quick fallback plaintext |
| **Iterative summary updates** | 2nd+ compression UPDATES previous summary, preserves info across compressions | Each eviction replaces previous summary — no tiered accumulation |
| **Anti-thrashing** | Skips compression if last 2 each saved <10%; warns user to start fresh | No anti-thrashing guards |
| **Session splitting** | Creates new session with lineage link on compression; `session_search` traverses lineage | No session splitting — single continuous session |
| **Token-budget tail protection** | Dynamic tail size based on token budget, not message count; ensures latest user msg is in tail | Fixed `WINDOW_MAX_MESSAGES=20` count-based window |
| **Focused compression** | `/compact <topic>` prioritizes preserving specific topic details | No equivalent |
| **Tool pair integrity** | Post-compression repair of orphaned tool_call/result pairs with stubs | `sanitize_messages()` removes orphans but doesn't stub |
| **Compression provider hooks** | `on_pre_compress()` lets ALL providers extract before discard | Only `memory_flush.rs` (no plugin hook architecture) |
| **Rate limit coordination** | Cross-session file-backed rate guards preventing retry amplification | No cross-session rate tracking |

### Former Dead Code Now Wired

| Module | What It Does | Gap It Closes |
|--------|-------------|---------------|
| `transcript.rs::index_compacted_messages()` | Groups evicted msgs into blocks of 5, chunks, embeds, stores in `memory_chunks` | Wired after sliding-window eviction when an embedding provider exists |
| `memory.rs::embed_memories_async()` | Chunks memory entries, embeds, stores in `memory_embeddings` | Wired from `store_facts()` when an embedding provider exists |

### Critical Finding: Tool Schema Token Bloat (Provider-Agnostic)

**The problem:** 640:1 input-to-output ratio. 80K avg input tokens, 125 avg output. 7s latency per request. 27.5K tokens (~34%) are tool schemas that are **identical on every request**.

**Why caching won't fix it:** ~80% of traffic routes through DashScope (Qwen 3.5 Flash), which has no prompt caching. ~15% goes to OpenAI (auto-caches >1024 tokens). Anthropic is banned/~0% traffic. So caching only helps ~15% of requests.

**The fix must reduce actual token count.** Three approaches (can combine):

> **Note:** Hermes has this exact same problem (identified as limitation P7 in their SME doc: "No tool schema pruning"). They mitigate via toolset-based filtering, dependency-based removal, and token-aware setup — but haven't fully solved it either.

**A. Activity-scoped tool sets (biggest win, Nebo-side):**
- `tool_filter::filter_tools_with_context()` in `crates/agent/src/tool_filter.rs` currently returns ALL tools unchanged
- Workflows have distinct activities (detect-meeting, send-reply, etc.). Each activity needs only 2-3 of the 9 tools
- Change: filter the `tool_defs` vector (not just `active_contexts`) based on the current activity/context
- Savings: 27.5K → ~6K per request (~78% reduction)
- Hermes equivalent: `get_tool_definitions(enabled_toolsets=["file", "terminal"], disabled_toolsets=["browser"])` + platform-specific toolset configs
- Impact on the math: 80K input → ~25K, latency 7s → 2-3s, ratio 640:1 → ~200:1

**B. Schema compression (medium win, Nebo-side):**
- The `os` tool alone is ~8-10K tokens (25 resources × nested properties)
- Reduce descriptions to 1-2 sentences, remove example values, use $ref for shared types
- Savings: ~10-15K tokens

**C. Deferred-by-default for heavy tools (easy win, Nebo-side):**
- Currently only `loop`, `work`, `execute`, `plugin`, `publisher` are deferred
- Make `os`, `web`, `desktop` deferred too — activate on first keyword match
- Savings: ~15K tokens until activation
- Already partially implemented in `tool_filter.rs`

**Janus-side note:** When traffic eventually routes to providers with caching (Anthropic, OpenAI), Janus should pass through cache markers. Currently `janus/internal/ai/types.go:69-73` `ToolDefinition` has no `CacheControl` field, so markers are stripped. Also: ensure tools come BEFORE dynamic conversation content in the API payload so Anthropic's prefix-based caching covers them.

**The math:**

| Scenario | Input Tokens | Latency (est.) |
|----------|-------------|----------------|
| Current (all 9 tools, no cache) | ~80K | 7s |
| Activity-scoped (2-3 tools) | ~25K | 2-3s |
| Scoped + schema compression | ~15K | 1-2s |

### Prioritized Improvement List

> **Revised 2026-05-13** after peer review. Key changes: structured compression promoted to P0, tool output pruning demoted to P1, Janus cache demoted to P2, sub-agent tool scoping and OS deferral added, anti-thrashing elevated for multi-agent safety.

#### P0 — Do This Week

1. **Tool schema reduction + sub-agent scoping** — Contextual filtering and `os` deferral are active, but the next win is tighter per-agent/activity scopes and schema compression for large tools.
2. **Wire plugin env injection into ExecuteTool registration** — `ExecuteTool` supports `with_plugin_store()`, but registry construction currently registers it without the plugin store, so skill scripts may not receive plugin binary env vars.
3. **Secret scan explicit memory stores** — Add `secret_scan` and prompt-injection checks to `agent(resource: "memory", action: "store")`, not only automatic extraction.
4. **Fence prompt-relevant memories** — The main memory section is fenced; query-time `Relevant to This Conversation` should use the same `<memory-context>` wrapper.
5. **Structured compression summary** — Replace single LLM summary with 12-section handoff template (Active Task, Completed Actions, Key Decisions, Remaining Work, etc.). Second-biggest context quality issue after tool schemas. When sliding window evicts 20 messages, the quality of what survives determines whether the agent stays on-task. For 4-step workflows, losing active task context mid-workflow causes multi-minute burns. Prerequisite for iterative summary updates.

#### P1 — Next Sprint

7. **Iterative summary updates** — On re-compression, UPDATE previous summary instead of replacing. Critical for multi-step workflows: detect-meeting → classify-email → check-calendar → send-reply may hit 2-3 compressions. Without iterative updates, by step 4 the agent only remembers step 3's summary — steps 1-2 are gone. Depends on P0.6 (structured template).
8. **Smart tool output pruning** — Upgrade `micro_compact` from generic `[trimmed: {tool} result]` to tool-specific summaries (`[os:shell] ran npm test → exit 0, 47 lines`). Nice but marginal token savings — the 640:1 ratio is driven by input schemas, not output trimming.
9. **Anti-thrashing** — Skip compression if last 2 attempts each saved <10%. For single-agent CLI this is nice-to-have; for Nebo's multi-agent workflows where sub-agents run autonomously, an infinite compression loop can silently consume quota and time. Should detect and abort the sub-agent with a clear error rather than looping.
10. **Memory usage metrics** — Add count + capacity display to "What You Know" header.
11. **Secret redaction** — Add regex patterns for API keys, JWTs, private keys in `sanitize.rs`. Apply before compression summaries. More critical for Nebo than CLI agents since it has machine-level access to user's files and credentials.
12. **LLM-summarized session search** — Upgrade bot_tool.rs session "query" from brute-force substring matching to hybrid search + auxiliary LLM summarization.

#### P2 — Backlog

13. **Token-budget tail protection** — Switch from fixed `WINDOW_MAX_MESSAGES=20` count to dynamic token-budget-based tail sizing.
14. **Session splitting on compression** — Create new session with lineage link on compression. Enable search across lineage.
15. **OS tool decomposition** — Split `os` into 3-4 smaller tools (`os_shell`, `os_desktop`, `os_media`, `os_pim`) for independent deferral and per-agent scoping. Proper fix for the quick deferral in P0.2.
16. ~~**Memory overflow management**~~ — **DONE (2026-05-15).** Background consolidation implemented in `memory_consolidation.rs`. Sweeps every 30 min, LLM-driven dedup/merge/prune per user_id scope. Gate chain: enabled check → 24h cooldown → 20+ memories → scope lock.
17. **Context references** — `@file:`, `@url:` injection into user messages at API-call time with token budget protection.
18. **Tool pair integrity** — Stub missing tool results after compression instead of removing orphans.
19. **Janus tool cache passthrough** — Add `CacheControl` field to `janus/internal/ai/types.go:ToolDefinition` and forward to Anthropic/OpenAI providers. Premature while ~80% of traffic is DashScope (no caching) and Anthropic is ~0%. Do when provider mix changes.
20. **Memory provider as plugin capability** — Add `memory` to plugin capability types (alongside tools, hooks, providers, events). Enables external memory backends (Mem0, Hindsight, etc.) as plugins.

### Personality Synthesis — Active, Needs Verification

`personality.rs` is actively called from `runner.rs:2625` after every turn's memory extraction. It synthesizes a personality directive from style observations. This is a major Nebo advantage over Hermes (which has nothing equivalent). The outstanding question is not "wire it in" but "verify it's working correctly" — confirm the synthesized directive is actually influencing behavior and appearing in the prompt. If it is, document it as a proven differentiator.

### Anthropic Cost Revisit

The plan notes Anthropic is "banned/~0% traffic." If by policy, fine. But the prompt caching calculus is worth revisiting: with Anthropic's prompt caching, 80K-token requests would cost ~90% less on cache hits. Tool schema reduction + caching together could make per-request cost comparable to DashScope. The 7s latency would drop to ~1-2s. Running the math on total cost (tokens × price × volume) might show Anthropic with caching is cheaper than DashScope without caching at current volume.

---

## 28. Scoped Memory Architecture

> **Added 2026-05-15.** Three-tier memory isolation + read inheritance for multi-agent memory scoping.

### Problem

Nebo is a multi-agent platform. The main Nebo companion learns about the user. Installed agents like Brief (legal) handle domain-specific work with sensitive per-client data. Before this change:

1. **No context-level isolation** — Brief stored Client A and Client B memories in the same bucket. Client A's privileged facts could leak into Client B's documents.
2. **No shared user layer** — Brief couldn't access the user's timezone or communication preferences stored by main Nebo.
3. **No consolidation** — memories accumulated but were never reviewed, deduplicated, or pruned across sessions.

### Design: Extend the user_id Convention

No schema migration needed. The existing `user_id` column already stores composite strings. We added a third tier:

```
Layer 1 (User):     user_id = "user123"                           ← main Nebo
Layer 2 (Agent):    user_id = "user123:agent:brief"               ← agent-wide
Layer 3 (Context):  user_id = "user123:agent:brief:ctx:doc-123"   ← per-context
```

### MemoryConfig (agent.json)

File: `crates/napp/src/agent.rs`

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryConfig {
    pub inherit_user: bool,      // READ user's tacit/preferences (read-only)
    pub context_isolated: bool,  // Memories scoped per contextId from SDK embed
}
```

Added to `AgentConfig` as `memory: MemoryConfig` (defaults to both `false`).

```json
{
  "memory": {
    "inherit_user": true,
    "context_isolated": true
  }
}
```

### Behavior Matrix

| Agent Config | Writes to | Reads from |
|-------------|-----------|------------|
| No memory config (default) | `user_id:agent:X` | `user_id:agent:X` only |
| `inherit_user: true` | `user_id:agent:X` | `user_id:agent:X` + `tacit/preferences` from `user_id` (read-only) |
| `context_isolated: true` | `user_id:agent:X:ctx:CTX` | `user_id:agent:X:ctx:CTX` + `user_id:agent:X` (agent-wide) |
| Both | `user_id:agent:X:ctx:CTX` | all 3 layers |

### Context ID Source

The `contextId` flows through the system via the session key:

- SDK: `chat.mount(el, { contextId: 'doc-123' })`
- Embed URL: `?ctx=doc-123`
- Session key: `"agent:brief:app:doc-123"`

Extracted in `runner.rs` by splitting the session key: `agent:{agent_id}:{channel}:{context_id}` — the 4th segment (if present) is the `context_id`.

### Memory vs Chat/Thread Scoping

Memories are scoped by `user_id`, NOT by chat_id or session_id. This is intentional:

- **`/new` (rotate_chat)**: Creates a new conversation under the same session. Memories persist — all chats share the same memory pool.
- **Threads**: Same — threads within a session share the same memory pool.
- **Main Nebo**: All companion chats share `user_id = "user123"` memories.
- **Agents**: All chats for the same agent share `user_id = "user123:agent:brief"`. With `context_isolated: true`, embed chats with different `contextId` get separate memory pools.

### Inherited Memory Scoring

Inherited memories (from parent scopes) are scored at 0.8x their normal score, so they rank below local memories but still appear when relevant. Deduplication by key keeps only the highest-scored version.

### Implementation Files

| File | Change |
|------|--------|
| `crates/napp/src/agent.rs` | `MemoryConfig` struct + `memory` field on `AgentConfig` |
| `crates/agent/src/runner.rs` | Extract context_id from session key; build 3-tier `memory_user_id`; build inherit chain; pass to db_context; fix `ToolContext.user_id` to use `memory_user_id` |
| `crates/agent/src/db_context.rs` | `InheritScope` struct; `load_db_context()` accepts `&[InheritScope]` |
| `crates/agent/src/memory.rs` | `load_scored_memories()` merges primary + inherited scopes with dedup |

### Critical Fix: ToolContext.user_id

Before this change, `run_loop` received the raw `user_id` and set it in `ToolContext.user_id`. This meant bot tool memory actions (`agent(resource: "memory", action: "store")`) bypassed agent scoping entirely — they wrote to the raw `user_id`, not the agent-scoped one. Now `ToolContext.user_id` is set to `memory_user_id` (the fully scoped value).

---

## 29. Background Memory Consolidation

> **Added 2026-05-15.** Periodic LLM-driven deduplication, merging, and pruning of memories.

File: `crates/agent/src/memory_consolidation.rs`

### Gate Chain (cheapest → most expensive)

1. **Feature check**: `get_plugin_setting("nebo", "memory_consolidation")` — enabled by default, disable with value `"disabled"`
2. **Time gate**: >= 24h since last consolidation for this scope (in-memory timestamp map)
3. **Memory count**: >= 20 memories in the scope
4. **Scope lock**: in-memory mutex per user_id (prevents concurrent consolidation of same scope)

### Scheduling

- Background `tokio::spawn` task, runs every 30 minutes
- Iterates over all distinct `user_id` values in the memories table
- Each scope that passes all 4 gates gets consolidated

### Consolidation Prompt

```
You are a memory curator. Review these N facts stored for a user and:
1. Identify duplicates — keep the most complete version, mark others for deletion
2. Identify contradictions — newer facts (higher id) supersede older ones
3. Identify stale or irrelevant facts that should be removed
4. Identify facts that can be merged into a single, more useful entry

Return JSON: { "keep": [...ids], "update": [{id, value}], "delete": [...ids] }
```

### Scope Isolation

Each distinct `user_id` is consolidated independently. `"user123:agent:brief:ctx:doc-A"` is **never** merged with `"user123:agent:brief:ctx:doc-B"`. This preserves the 3-tier memory isolation.

### DB Method

`Store::get_distinct_memory_user_ids()` — returns all unique user_id values with memory counts, ordered by count descending.

### Startup

Spawned from `crates/server/src/lib.rs` after runner creation:

```rust
agent::memory_consolidation::spawn_sweep(store.clone(), runner.providers());
```

Uses `prefer_non_gateway()` to pick the cheapest available provider (avoids burning Janus credits on background work).
