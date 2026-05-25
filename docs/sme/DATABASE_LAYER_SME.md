# DATABASE LAYER SME

Comprehensive reference for the Nebo database layer (`crates/db/`). Covers architecture,
schema, query patterns, connection pooling, migration system, and cross-crate interactions.

Last updated: 2026-05-15

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [File Layout](#2-file-layout)
3. [Dependencies](#3-dependencies)
4. [SQLite Configuration and Pragmas](#4-sqlite-configuration-and-pragmas)
5. [Connection Pool](#5-connection-pool)
6. [Store Abstraction](#6-store-abstraction)
7. [Migration System](#7-migration-system)
8. [Schema Overview](#8-schema-overview)
9. [Model Structs](#9-model-structs)
10. [Query Layer Patterns](#10-query-layer-patterns)
11. [Error Handling Patterns](#11-error-handling-patterns)
12. [Full-Text Search (FTS5)](#12-full-text-search-fts5)
13. [Embedding and Vector Storage](#13-embedding-and-vector-storage)
14. [Transaction Handling](#14-transaction-handling)
15. [Concurrent Access Patterns](#15-concurrent-access-patterns)
16. [Cross-System Interactions](#16-cross-system-interactions)
17. [Key Functions Reference](#17-key-functions-reference)
18. [Performance Considerations](#18-performance-considerations)
19. [Data Lifecycle and Cleanup](#19-data-lifecycle-and-cleanup)
20. [Schema Relationship Diagram](#20-schema-relationship-diagram)

---

## 1. Architecture Overview

The database layer is a single Rust crate (`nebo-db`) that wraps SQLite via `rusqlite` with
an r2d2 connection pool. It provides a `Store` struct that exposes typed query methods as
`impl Store` blocks spread across 25 query module files. There is no ORM -- all SQL is
hand-written with parameterized queries.

```
                          +------------------------------+
                          |         Entry Points         |
                          |  cli/main.rs  src-tauri/     |
                          +------+-------+------+--------+
                                 |       |      |
                                 v       v      v
                          +------------------------------+
                          |       crates/server/         |
                          |  AppState { store: Arc<Store>}|
                          +------+-------+------+--------+
                                 |       |      |
               +-----------------+       |      +------------------+
               |                         |                         |
               v                         v                         v
    +-------------------+    +-------------------+    +-------------------+
    |   crates/agent/   |    |   crates/tools/   |    |   crates/napp/    |
    | memory, runner,   |    | domain tools,     |    | plugin install,   |
    | orchestrator      |    | bot_tool queries  |    | skill loader      |
    +--------+----------+    +--------+----------+    +--------+----------+
             |                        |                        |
             +------------------------+------------------------+
                                      |
                                      v
                          +------------------------------+
                          |        crates/db/            |
                          |                              |
                          |  Store (pub struct)          |
                          |    |                         |
                          |    +-- pool.rs (r2d2 pool)   |
                          |    +-- migrate.rs            |
                          |    +-- models.rs (30 structs)|
                          |    +-- queries/ (25 modules) |
                          +------+-------+------+--------+
                                 |       |      |
                                 v       v      v
                          +------------------------------+
                          |     SQLite (WAL mode)        |
                          |  ~/.nebo/data/nebo.db        |
                          +------------------------------+
```

### Key Design Decisions

- **No ORM**: Raw SQL for full control over queries and performance. Avoids the abstraction
  overhead of Diesel or SeaORM.
- **Single DB file**: All data in one SQLite database. No sharding, no read replicas.
- **WAL mode**: Enables concurrent reads during writes -- critical for a desktop app with
  background agents running alongside the UI.
- **Connection pool**: r2d2 manages connection reuse. Each `self.conn()` call checks out a
  connection from the pool.
- **Embedded migrations**: SQL files are compiled into the binary via `rust-embed`. No
  external migration tool needed at runtime.
- **Query modules as `impl Store`**: Each domain area (agents, chats, memories, etc.) adds
  methods to the same `Store` struct. No trait indirection.

---

## 2. File Layout

```
crates/db/
  Cargo.toml
  src/
    lib.rs              # Crate root, re-exports, OptionalExt + DbErrExt traits
    pool.rs             # create_pool(), SqlitePragmas customizer
    store.rs            # Store struct: new() + conn()
    migrate.rs          # run_migrations(), goose compat, rust-embed
    models.rs           # 30+ model structs (Serialize/Deserialize)
    queries/
      mod.rs            # Module declarations for all 25 query files
      agents.rs         # Agent CRUD + workflow bindings
      memories.rs       # Memory CRUD + FTS health + namespace queries
      chats.rs          # Chat + ChatMessage CRUD + pagination
      sessions.rs       # Session lifecycle, counters, overrides
      users.rs          # User CRUD + password reset
      auth_profiles.rs  # Provider auth profiles + cooldown + rotation
      settings.rs       # Global settings + plugin settings + skill secrets
      plugins.rs        # Plugin registry (.napp) + settings
      workflows.rs      # Workflow CRUD + runs + activity results + stats
      pending_tasks.rs  # Task queue + tracking items + task lists
      notifications.rs  # Notification CRUD + read/unread
      cron_jobs.rs      # Cron job CRUD + history
      embeddings.rs     # Embedding cache + memory chunks + FTS search
      entity_config.rs  # Per-entity config (heartbeat, permissions, etc.)
      mcp_integrations.rs # MCP server integration + OAuth flow
      commander.rs      # Commander visual editor (teams, edges, positions)
      a2ui_surfaces.rs  # Agent-to-UI surfaces
      advisors.rs       # Advisor personas
      agent_profile.rs  # Agent personality profile
      user_profile.rs   # User profile + onboarding
      refresh_tokens.rs # JWT refresh tokens
      provider_models.rs # AI model catalog
      event_dedup.rs    # Event fingerprint deduplication
      license_keys.rs   # License key cache (NeboLoop marketplace)
  migrations/
    0001_initial_schema.sql ... 0092_agent_soul.sql   # 90+ migration files
```

---

## 3. Dependencies

From `crates/db/Cargo.toml`:

| Crate         | Purpose                                         |
|---------------|--------------------------------------------------|
| `rusqlite`    | SQLite bindings (bundled libsqlite3)             |
| `r2d2`        | Generic connection pool                          |
| `r2d2_sqlite` | SQLite adapter for r2d2                          |
| `serde`       | Serialize/Deserialize for model structs          |
| `serde_json`  | JSON parsing (metadata columns, dynamic queries) |
| `tracing`     | Structured logging (migration progress, errors)  |
| `chrono`      | Timestamp handling                               |
| `thiserror`   | Error type definitions (via `types` crate)       |
| `rust-embed`  | Embed migration SQL files into binary            |
| `uuid`        | UUID generation for new record IDs               |
| `types`       | Workspace `NeboError` enum                       |

The `rusqlite` dependency uses workspace-level feature flags to bundle `libsqlite3-sys` with
FTS5 and JSON1 extensions enabled.

---

## 4. SQLite Configuration and Pragmas

Every connection acquired from the pool applies the following pragmas via the
`SqlitePragmas` customizer (`pool.rs`):

```sql
PRAGMA journal_mode = WAL;           -- Write-Ahead Logging for concurrent reads
PRAGMA synchronous = NORMAL;         -- Balanced durability vs performance
PRAGMA foreign_keys = ON;            -- Enforce FK constraints
PRAGMA busy_timeout = 5000;          -- Wait 5s on lock contention before SQLITE_BUSY
PRAGMA cache_size = -20000;          -- 20MB page cache (negative = KB)
PRAGMA temp_store = MEMORY;          -- Temp tables and indexes in RAM
```

### Pragma Rationale

| Pragma               | Value      | Why                                                      |
|----------------------|------------|----------------------------------------------------------|
| `journal_mode`       | `WAL`      | Allows concurrent readers during writes. Essential for a  |
|                      |            | desktop app where background agents write while UI reads. |
| `synchronous`        | `NORMAL`   | WAL mode guarantees durability with NORMAL. FULL would    |
|                      |            | add an extra fsync per commit with no WAL-mode benefit.   |
| `foreign_keys`       | `ON`       | Cascade deletes depend on this. SQLite defaults to OFF.   |
| `busy_timeout`       | `5000`     | 5 second wait prevents immediate SQLITE_BUSY errors       |
|                      |            | during concurrent writes. Pool size 10 can cause lock     |
|                      |            | contention on write-heavy workloads.                      |
| `cache_size`         | `-20000`   | 20MB cache. Negative value = kilobytes. Keeps hot pages   |
|                      |            | in memory for frequently accessed tables.                 |
| `temp_store`         | `MEMORY`   | ORDER BY / GROUP BY temp tables stay in RAM.              |

---

## 5. Connection Pool

### Pool Architecture

```
         +---------------------+
         |    Application      |
         |  (multiple threads) |
         +---+---+---+---+----+
             |   |   |   |
             v   v   v   v
         +---------------------+
         |    r2d2::Pool       |
         |  max_size: 10       |
         |  min_idle: 1        |
         |  timeout: 30s       |
         +---+---+---+---+----+
             |   |   |   |
             v   v   v   v
         +---+ +-+ +-+ +-+---+
         |C1 | |C2| |C3| |...| (up to 10 SQLiteConnections)
         +---+ +--+ +--+ +---+
                    |
                    v
         +---------------------+
         |   nebo.db (WAL)     |
         |  ~/.nebo/data/      |
         +---------------------+
```

### Configuration

```rust
// pool.rs
pub fn create_pool(db_path: &str) -> Result<DbPool, NeboError> {
    // Ensures parent directory exists
    let manager = SqliteConnectionManager::file(db_path);
    let pool = Pool::builder()
        .max_size(10)                              // Max 10 connections
        .min_idle(Some(1))                         // Keep at least 1 warm
        .connection_timeout(Duration::from_secs(30)) // 30s to acquire
        .connection_customizer(Box::new(SqlitePragmas)) // Apply pragmas
        .build(manager)?;
    Ok(pool)
}
```

### Pool Type

```rust
pub type DbPool = Pool<SqliteConnectionManager>;
```

### Connection Checkout

Every query method calls `self.conn()` which checks out a pooled connection:

```rust
pub(crate) fn conn(&self)
    -> Result<r2d2::PooledConnection<SqliteConnectionManager>, NeboError>
{
    self.pool.get()
        .map_err(|e| NeboError::Database(format!("failed to get connection: {e}")))
}
```

The `PooledConnection` is returned to the pool when it goes out of scope (RAII). There is
no explicit release call.

### Pool Sizing Rationale

- **max_size = 10**: SQLite only supports one writer at a time (even in WAL mode). 10
  connections allow up to 10 concurrent readers, with writes serialized by SQLite's internal
  locking. For a desktop app this is generous.
- **min_idle = 1**: Avoids cold-start latency for the first query after idle periods.
- **connection_timeout = 30s**: Generous timeout. If all 10 connections are in use for 30s,
  something is seriously wrong (likely a deadlock or runaway query).

---

## 6. Store Abstraction

### Definition

```rust
// store.rs
pub struct Store {
    pool: DbPool,
}
```

The `Store` is the single entry point for all database operations. It is created once at
startup and shared via `Arc<Store>` across all server threads.

### Initialization

```rust
impl Store {
    pub fn new(db_path: &str) -> Result<Self, NeboError> {
        let pool = crate::create_pool(db_path)?;
        // Run migrations on a dedicated connection
        {
            let conn = pool.get()?;
            migrate::run_migrations(&conn)?;
        }
        Ok(Self { pool })
    }
}
```

Initialization order:
1. Create connection pool (ensures parent directory exists)
2. Acquire a single connection
3. Run all pending migrations
4. Release the migration connection back to the pool
5. Return the `Store` ready for use

### Usage in AppState

```rust
// server/src/state.rs
pub struct AppState {
    pub store: Arc<Store>,
    // ... 30+ other fields
}

// cli/src/main.rs
let store = Arc::new(db::Store::new(&cfg.database.sqlite_path)?);
```

### Database Path

The default path is `~/.nebo/data/nebo.db`, configured via `etc/nebo.yaml`:

```yaml
database:
  sqlite_path: ~/.nebo/data/nebo.db
```

The `create_pool()` function creates the parent directory (`~/.nebo/data/`) if it does not
exist.

---

## 7. Migration System

### Overview

Nebo uses a custom migration runner (`migrate.rs`) that is compatible with
[goose](https://github.com/pressly/goose) migration files. Migrations are embedded into the
binary at compile time via `rust-embed`.

### How It Works

```
  +-----------------------------------+
  |  1. Create _nebo_migrations table |
  |     (if not exists)               |
  +---+-------------------------------+
      |
      v
  +-----------------------------------+
  |  2. Reconcile goose_db_version    |
  |     (import from Go era)          |
  +---+-------------------------------+
      |
      v
  +-----------------------------------+
  |  3. Get list of applied versions  |
  |     from _nebo_migrations         |
  +---+-------------------------------+
      |
      v
  +-----------------------------------+
  |  4. Sort embedded SQL files       |
  |     by filename (0001, 0002...)   |
  +---+-------------------------------+
      |
      v
  +---+-------------------------------+
  |  5. For each unapplied migration: |
  |     a. Extract goose "Up" section |
  |     b. BEGIN transaction          |
  |     c. Execute SQL                |
  |     d. Record in _nebo_migrations |
  |     e. COMMIT (or ROLLBACK)       |
  +-----------------------------------+
```

### Migration Tracking Table

```sql
CREATE TABLE IF NOT EXISTS _nebo_migrations (
    version INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    applied_at TEXT NOT NULL DEFAULT (datetime('now'))
);
```

### Goose Compatibility

The system detects and imports from goose's `goose_db_version` table if present. This
enables seamless migration from the original Go codebase to the Rust rewrite:

```rust
fn reconcile_goose_versions(conn: &Connection) -> Result<(), NeboError> {
    // Check if goose table exists
    // Import goose versions we haven't tracked yet
    conn.execute_batch(
        "INSERT OR IGNORE INTO _nebo_migrations (version, name)
         SELECT version_id, CAST(version_id AS TEXT) || '_goose_imported.sql'
         FROM goose_db_version
         WHERE version_id > 0 AND is_applied = 1;"
    )?;
}
```

### Filename Convention

Migration files follow the pattern `NNNN_description.sql` where `NNNN` is a zero-padded
version number:

```
0001_initial_schema.sql       -> version 1
0008_chats.sql                -> version 8
0092_agent_soul.sql           -> version 92
```

The version is extracted by splitting on `_` and parsing the first segment as i64.

### Goose Section Parsing

Migration files may contain goose markers:

```sql
-- +goose Up
CREATE TABLE foo (id INT);
-- +goose Down
DROP TABLE foo;
```

The `extract_goose_up()` function returns only the "Up" section. If no goose markers are
found, the entire file is used.

### No Rollback Support

There is no automatic rollback mechanism. Migrations are forward-only. If a migration fails
mid-execution, the transaction is rolled back and the migration is not recorded. The server
will retry on next startup.

### Migration Count

As of the current codebase: **92 migrations** (0001 through 0092, with some gaps -- e.g.,
0002 is missing, 0068 is missing, 0076 is missing).

---

## 8. Schema Overview

### Core Tables

| Table                           | Purpose                                   | PK Type   |
|---------------------------------|-------------------------------------------|-----------|
| `users`                         | User accounts                             | TEXT (UUID)|
| `user_preferences`              | Per-user settings                         | TEXT (FK)  |
| `user_profiles`                 | Extended profile (bio, occupation, etc.)   | TEXT (FK)  |
| `refresh_tokens`                | JWT refresh tokens                        | TEXT (UUID)|
| `sessions`                      | Agent/channel sessions                    | TEXT (UUID)|
| `chats`                         | Conversations (linked to sessions)        | TEXT (UUID)|
| `chat_messages`                 | Individual messages within chats          | TEXT (UUID)|
| `agents`                        | Installed agents (from .napp packages)    | TEXT (UUID)|
| `agent_workflows`               | Workflow bindings per agent               | INTEGER    |
| `memories`                      | Key-value memory store                    | INTEGER    |
| `memory_chunks`                 | Chunked text for embeddings               | INTEGER    |
| `memory_embeddings`             | Vector embeddings for chunks              | INTEGER    |
| `embedding_cache`               | Dedup cache for embeddings                | TEXT (hash)|
| `memories_fts`                  | FTS5 virtual table for memory search      | (virtual)  |
| `memory_chunks_fts`             | FTS5 virtual table for chunk search       | (virtual)  |
| `auth_profiles`                 | AI provider credentials                   | TEXT (UUID)|
| `settings`                      | Global app settings (singleton, id=1)     | INTEGER    |
| `notifications`                 | User notifications                        | TEXT (UUID)|
| `cron_jobs`                     | Scheduled tasks                           | INTEGER    |
| `cron_history`                  | Cron execution history                    | INTEGER    |
| `pending_tasks`                 | Task queue for agent work                 | TEXT (UUID)|
| `mcp_integrations`              | MCP server connections                    | TEXT (UUID)|
| `mcp_integration_credentials`   | OAuth tokens for MCP                      | TEXT (UUID)|
| `plugin_registry`               | Installed plugins (.napp)                 | TEXT       |
| `plugin_settings`               | Plugin configuration                      | TEXT       |
| `workflows`                     | Workflow definitions                      | TEXT (UUID)|
| `workflow_tool_bindings`        | Tool interface bindings                   | INTEGER    |
| `workflow_runs`                 | Workflow execution history                | TEXT (UUID)|
| `workflow_activity_results`     | Per-activity execution results            | INTEGER    |
| `entity_config`                 | Per-entity overrides (heartbeat, perms)   | INTEGER    |
| `provider_models`               | AI model catalog                          | TEXT (UUID)|
| `leads`                         | Email capture / landing page              | TEXT (UUID)|
| `error_logs`                    | Application error log                     | INTEGER    |
| `commander_teams`               | Commander UI teams                        | TEXT (UUID)|
| `commander_team_members`        | Team membership (junction)                | composite  |
| `commander_node_positions`      | Visual node positions                     | TEXT       |
| `commander_edges`               | Visual graph edges                        | TEXT (UUID)|
| `a2ui_surfaces`                 | Agent-to-UI surfaces                      | TEXT (UUID)|
| `event_dedup`                   | Event fingerprint deduplication           | TEXT       |
| `license_keys`                  | Cached license keys (NeboLoop)            | TEXT       |
| `oauth_connections`             | OAuth provider connections                | TEXT (UUID)|
| `channels`                      | Communication channels                    | TEXT (UUID)|
| `agent_profile`                 | Agent personality (singleton)             | INTEGER    |
| `advisors`                      | Advisor personas                          | INTEGER    |

### ID Generation Strategy

- **TEXT UUIDs**: Most tables use `TEXT PRIMARY KEY` with UUIDs generated in Rust via
  `uuid::Uuid::new_v4().to_string()`. Some use `hex(randomblob(16))` in SQL.
- **INTEGER AUTOINCREMENT**: Used for tables where sequential ordering matters (memories,
  memory_chunks, cron_jobs, agent_profile, advisors).
- **Composite keys**: `agent_workflows` uses `UNIQUE(agent_id, binding_name)`;
  `memories` uses `UNIQUE(namespace, key, user_id)`.

### Timestamp Conventions

Two timestamp patterns coexist due to the Go-to-Rust migration:

| Pattern                          | Usage                           |
|----------------------------------|---------------------------------|
| `strftime('%s', 'now')`          | Original Go-era tables (users)  |
| `unixepoch()`                    | Newer Rust-era tables           |
| `CURRENT_TIMESTAMP`              | Some text-format timestamps     |
| `datetime('now')`                | Some text-format timestamps     |

All integer timestamps are Unix epoch seconds. Some columns use ISO 8601 text format
(memories.created_at, cron_jobs.last_run).

---

## 9. Model Structs

All model structs live in `models.rs`. They derive `Debug, Clone, Serialize, Deserialize`
and use `#[serde(rename_all = "camelCase")]` for JSON API compatibility.

### Serialization Helpers

Three custom serializers handle SQLite integer-as-boolean and JSON-string-as-array patterns:

```rust
fn i64_as_bool<S>(val: &i64, s: S) -> Result<S::Ok, S::Error>
    // Serializes 0/1 as false/true

fn opt_i64_as_bool<S>(val: &Option<i64>, s: S) -> Result<S::Ok, S::Error>
    // Serializes None/0/1 as false/false/true

fn json_string_as_array<S>(val: &Option<String>, s: S) -> Result<S::Ok, S::Error>
    // Deserializes a JSON string column as a proper JSON array
```

### Key Model Structs

**Session** -- 24 fields, tracks conversation state:
```rust
pub struct Session {
    pub id: String,
    pub name: Option<String>,            // Session key (e.g., "agent:abc:chat")
    pub scope: Option<String>,           // "agent", "channel", etc.
    pub scope_id: Option<String>,        // Agent/channel ID
    pub summary: Option<String>,         // Compacted conversation summary
    pub token_count: Option<i64>,        // Estimated token usage
    pub message_count: Option<i64>,      // Total messages in session
    pub last_compacted_at: Option<i64>,  // Last compaction timestamp
    pub compaction_count: Option<i64>,   // How many compactions occurred
    pub memory_flush_at: Option<i64>,    // Last memory extraction
    pub active_chat_id: Option<String>,  // Current conversation in multi-chat
    pub model_override: Option<String>,  // Per-session model override
    pub active_task: Option<String>,     // Current agent task
    // ... and more
}
```

**Agent** -- 18 fields, installed agents:
```rust
pub struct Agent {
    pub id: String,
    pub kind: Option<String>,           // Agent kind/category
    pub name: String,
    pub agent_md: String,               // AGENT.md content (capabilities)
    pub frontmatter: String,            // YAML frontmatter (inputs, config)
    pub is_enabled: i64,
    pub napp_path: Option<String>,      // Path to .napp archive
    pub is_app: Option<i64>,            // Has UI + optional sidecar
    pub app_ui_path: Option<String>,    // Static UI directory
    pub app_binary_path: Option<String>,// Sidecar binary
    pub soul: Option<String>,           // SOUL.md content (personality)
    // ...
}
```

**Memory** -- 11 fields, key-value memory store:
```rust
pub struct Memory {
    pub id: i64,
    pub namespace: String,       // Hierarchical: "tacit/preferences", "entity/"
    pub key: String,             // Memory key (descriptive name)
    pub value: String,           // Memory content
    pub tags: Option<String>,    // JSON array of tags
    pub metadata: Option<String>,// JSON metadata (confidence, source, etc.)
    pub access_count: Option<i64>,// Usage frequency for ranking
    pub user_id: String,         // Scoped: "{user_id}" or "{user_id}:agent:{agent_id}"
    // ...
}
```

**ChatMessage** -- 12 fields including a transient `html` field:
```rust
pub struct ChatMessage {
    pub id: String,
    pub chat_id: String,
    pub role: String,           // "user", "assistant", "system", "tool"
    pub content: String,
    pub tool_calls: Option<String>,    // JSON array of tool call requests
    pub tool_results: Option<String>,  // JSON array of tool results
    pub token_estimate: Option<i64>,
    pub day_marker: Option<String>,    // Date string for day-grouping
    #[serde(skip_deserializing, default)]
    pub html: Option<String>,          // Server-rendered HTML (not stored)
    // ...
}
```

---

## 10. Query Layer Patterns

### Pattern: `impl Store` Extension

Every query module adds methods to the `Store` struct via `impl Store`. This avoids trait
indirection and keeps the API surface flat:

```rust
// queries/agents.rs
impl Store {
    pub fn list_agents(&self, limit: i64, offset: i64) -> Result<Vec<Agent>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare("SELECT ... FROM agents ...")?;
        let rows = stmt.query_map(params![limit, offset], row_to_agent)?;
        rows.collect::<Result<Vec<_>, _>>().db_err("list_agents collect")
    }
}
```

### Pattern: Row Mapper Functions

Each query module defines private `row_to_*` functions that map `rusqlite::Row` to model
structs:

```rust
fn row_to_agent(row: &rusqlite::Row) -> rusqlite::Result<Agent> {
    Ok(Agent {
        id: row.get(0)?,
        kind: row.get(1)?,
        name: row.get(2)?,
        // ... positional indexing
    })
}
```

Two indexing styles exist:
- **Positional** (`row.get(0)?`): Used in agents.rs, workflows.rs, commander.rs
- **Named** (`row.get("id")?`): Used in chats.rs, sessions.rs, memories.rs, pending_tasks.rs

Named indexing is more robust against column reordering but slightly slower. Positional
indexing requires matching the exact SELECT column order.

### Pattern: RETURNING Clause

For INSERT operations that need the created row, SQLite's `RETURNING *` is used:

```rust
conn.query_row(
    "INSERT INTO chats (id, title, ...) VALUES (?1, ?2, ...) RETURNING *",
    params![id, title],
    row_to_chat,
)
```

### Pattern: Upsert with ON CONFLICT

Idempotent writes use SQLite's `ON CONFLICT` clause:

```rust
// memories.rs
"INSERT INTO memories (namespace, key, value, ...)
 VALUES (?1, ?2, ?3, ...)
 ON CONFLICT(namespace, key, user_id) DO UPDATE SET
    value = excluded.value,
    tags = COALESCE(excluded.tags, tags),
    updated_at = CURRENT_TIMESTAMP"
```

### Pattern: COALESCE for Partial Updates

Partial updates use `COALESCE` to only overwrite non-NULL parameters:

```rust
"UPDATE agents SET name = ?1, description = ?2,
    soul = COALESCE(?7, soul),
    updated_at = unixepoch()
 WHERE id = ?8"
```

### Pattern: Pagination

Standard LIMIT/OFFSET pagination:

```rust
pub fn list_agents(&self, limit: i64, offset: i64) -> Result<Vec<Agent>, NeboError>
pub fn list_memories(&self, limit: i64, offset: i64) -> Result<Vec<Memory>, NeboError>
```

Chat messages also support cursor-based pagination for "load more" and budget-based
pagination:

```rust
// Cursor-based: fetch messages older than a given message ID
pub fn get_chat_messages_paginated(
    &self, chat_id: &str, limit: i64, before: Option<&str>
) -> Result<Vec<ChatMessage>, NeboError>

// Budget-based: fetch newest messages up to a character budget
pub fn get_chat_messages_budgeted(
    &self, chat_id: &str, max_chars: i64, before: Option<&str>
) -> Result<Vec<ChatMessage>, NeboError>
```

### Pattern: Dynamic SQL Generation

The `update_settings()` and `update_workflow_run()` methods build SQL dynamically when
only some fields are being updated:

```rust
// settings.rs -- dynamic SET clause with raw_bind_parameter
let mut updates = Vec::new();
if autonomous_mode.is_some() {
    updates.push(format!("autonomous_mode = ?{}", param_idx));
    param_idx += 1;
}
// ...
let sql = format!("UPDATE settings SET {} WHERE id = 1", updates.join(", "));
```

### Pattern: OptionalExt Trait

Multiple query modules define a local `OptionalExt` trait to convert
`QueryReturnedNoRows` into `Ok(None)`. This is also defined once in `lib.rs` and
re-exported:

```rust
pub(crate) trait OptionalExt<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error>;
}

impl<T> OptionalExt<T> for Result<T, rusqlite::Error> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error> {
        match self {
            Ok(val) => Ok(Some(val)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}
```

Note: The trait is defined locally in many query files (chats.rs, sessions.rs, memories.rs,
users.rs, pending_tasks.rs, cron_jobs.rs, auth_profiles.rs, embeddings.rs). The crate-level
version in `lib.rs` is used by agents.rs and a2ui_surfaces.rs via `use crate::OptionalExt`.

---

## 11. Error Handling Patterns

### NeboError::Database

All database errors are converted to `NeboError::Database(String)`:

```rust
conn.execute("...", params![...])
    .map_err(|e| NeboError::Database(e.to_string()))?;
```

### DbErrExt Trait

A more ergonomic error conversion that logs before converting:

```rust
pub(crate) trait DbErrExt<T> {
    fn db_err(self, context: &str) -> Result<T, types::NeboError>;
}

// Usage:
let rows = stmt.query_map(params![...], row_to_agent)
    .db_err("list_agents query")?;
```

This logs `database error` at WARN level with structured context before returning the
error. Used in agents.rs, notifications.rs, and workflows.rs. Older query files use the
manual `.map_err(|e| NeboError::Database(e.to_string()))` pattern.

### NeboError::Migration

Migration-specific errors have their own variant:

```rust
NeboError::Migration(format!("migration {filename} failed: {e}"))
```

---

## 12. Full-Text Search (FTS5)

### memories_fts

FTS5 virtual table for searching memories by key, value, and tags:

```sql
CREATE VIRTUAL TABLE memories_fts USING fts5(
    key, value, tags,
    content='memories',
    content_rowid='id'
);
```

Kept in sync by three triggers:
- `memories_ai` -- AFTER INSERT: inserts into FTS
- `memories_au` -- AFTER UPDATE: deletes old, inserts new
- `memories_ad` -- AFTER DELETE: deletes from FTS

### memory_chunks_fts

FTS5 for searching memory chunks:

```sql
CREATE VIRTUAL TABLE memory_chunks_fts USING fts5(
    text, source, path,
    content='memory_chunks',
    content_rowid='id'
);
```

### FTS Query Sanitization

User queries are sanitized before FTS5 MATCH:

```rust
fn sanitize_fts_query(query: &str) -> String {
    // Each word wrapped in quotes, joined with OR
    // "hello world" -> "\"hello\" OR \"world\""
}
```

### FTS Health Check

At startup, `ensure_fts_healthy()` validates the FTS table:

1. Check if `memories_fts` exists in `sqlite_master`
2. Run FTS5 integrity check: `INSERT INTO memories_fts(memories_fts) VALUES('integrity-check')`
3. If either fails, call `rebuild_fts()` which drops and recreates the table + triggers

This was added to handle a known issue where migration 0021 (DROP + RENAME of memories
table) broke the FTS5 content table binding, causing silent insert rollbacks.

---

## 13. Embedding and Vector Storage

### Storage Architecture

```
  memories (key-value pairs)
      |
      | 1:N (memory_id FK)
      v
  memory_chunks (text segments + metadata)
      |
      | 1:1 (chunk_id FK)
      v
  memory_embeddings (binary blob vectors)

  embedding_cache (content_hash -> blob, for dedup)
```

### Embedding Cache

Avoids recomputing embeddings for identical content:

```sql
CREATE TABLE embedding_cache (
    content_hash TEXT PRIMARY KEY,
    embedding BLOB NOT NULL,
    model TEXT NOT NULL,
    dimensions INTEGER NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
```

### Vector Storage Format

Embeddings are stored as raw `BLOB` (binary `Vec<u8>` in Rust). The actual vector format
(f32 array, etc.) is encoded/decoded by the calling code in `crates/agent/src/memory.rs`.

### Hybrid Search

The embeddings module supports two search paths:
- **FTS5 search**: `search_memories_fts()` and `search_chunks_fts()` for text matching
- **Vector retrieval**: `get_all_embeddings_by_user()` returns all embeddings for
  in-memory cosine similarity computation

---

## 14. Transaction Handling

### Migration Transactions

Each migration runs in an explicit transaction:

```rust
conn.execute_batch("BEGIN;")?;
match conn.execute_batch(&up_sql) {
    Ok(()) => {
        conn.execute("INSERT INTO _nebo_migrations ...", ...)?;
        conn.execute_batch("COMMIT;")?;
    }
    Err(e) => {
        conn.execute_batch("ROLLBACK;");
        return Err(...);
    }
}
```

### Query-Level Transactions

Most query methods do NOT use explicit transactions. Each `conn.execute()` or
`conn.query_row()` runs as an implicit auto-commit transaction. This is acceptable because:

1. Most operations are single-statement (INSERT, UPDATE, DELETE)
2. The connection pool ensures each method gets its own connection
3. WAL mode handles concurrent readers without blocking

### Multi-Statement Operations

A few methods perform multiple statements on the same connection without explicit
transactions:

- `upsert_memory()`: INSERT followed by a verification SELECT (same connection)
- `create_chat_message_for_runner()`: INSERT OR IGNORE into chats, then INSERT into
  chat_messages
- `set_commander_team_members()`: DELETE all + INSERT loop (no transaction wrapper)
- `save_commander_node_positions()`: UPSERT loop (no transaction wrapper)
- `seed_task_list()`: INSERT loop (no transaction wrapper)

These could benefit from explicit transaction wrapping for atomicity, but the risk is
low for a single-user desktop application.

---

## 15. Concurrent Access Patterns

### WAL Mode Benefits

```
  Thread A (UI)          Thread B (Agent)        Thread C (Heartbeat)
      |                      |                        |
      |  SELECT messages     |  INSERT memory         |  UPDATE entity_config
      |  (shared read)       |  (exclusive write)     |  (waits for B)
      |       |              |       |                 |       |
      |  [reads WAL          |  [writes to WAL]       |  [busy_timeout
      |   snapshot]          |                         |   5000ms wait]
      |                      |                         |
      |  [OK, returns data]  |  [COMMIT, releases      |  [acquires write
      |                      |   write lock]           |   lock, proceeds]
```

### Write Contention

SQLite in WAL mode allows ONE writer at a time. Multiple readers are fine. The
`busy_timeout = 5000` pragma means a blocked writer waits up to 5 seconds before
returning SQLITE_BUSY.

Potential write contention scenarios in Nebo:
- Agent memory extraction (writes memories) vs. heartbeat (writes entity_config)
- Chat message persistence (writes chat_messages) vs. memory flush (writes memories)
- Multiple agent workflows executing simultaneously

The 5-second timeout is generous for a desktop app. If contention becomes an issue, the
pool size could be reduced or writes could be serialized through a write queue.

### Connection Pool and Thread Safety

The `Store` struct is `Send + Sync` because:
- `r2d2::Pool` is `Send + Sync`
- Each thread gets its own `PooledConnection` (no shared mutable state)
- rusqlite `Connection` is not `Send`, but `PooledConnection` handles this correctly

---

## 16. Cross-System Interactions

### Crates That Use `Store`

| Crate          | Usage                                                              |
|----------------|--------------------------------------------------------------------|
| `cli`          | Creates `Store::new()`, wraps in `Arc<Store>`, passes to server    |
| `server`       | `AppState.store` -- all HTTP handlers access DB through this       |
| `agent`        | Memory extraction, session management, compaction, prompt assembly |
| `tools`        | Domain tools (bot, system, loop) query DB for context              |
| `napp`         | Plugin/agent installation reads/writes agents, plugins, workflows  |
| `workflow`     | Workflow engine reads definitions, writes run history               |
| `auth`         | Token validation reads users, refresh_tokens                       |

### Server Handlers

Server handlers receive `Store` via Axum's `State(state): State<AppState>` extractor:

```rust
// handlers/chat.rs (typical pattern)
pub async fn send_message(
    State(state): State<AppState>,
    // ...
) -> impl IntoResponse {
    let store = &state.store;
    let session = store.get_session_by_name(&session_key)?;
    // ...
}
```

### Agent Memory System

The agent crate (`crates/agent/src/memory.rs`) is the heaviest DB consumer:

- `store_memories()` -- Upserts extracted memories with metadata
- `build_memory_context()` -- Reads tacit memories with confidence filtering
- `embed_memories_async()` -- Inserts memory chunks and embeddings
- `ensure_fts_healthy()` -- Startup FTS integrity check

### Session Lifecycle

```
  Session Created              Active Use                   Compaction
  +-----------+          +-------------------+         +------------------+
  | create_   |          | increment_session |         | update_session_  |
  | session() |  ------> | _message_count()  | ------> | summary()        |
  +-----------+          | set_session_      |         | reset_session_   |
                         | active_task()     |         | _counters()      |
                         | set_session_      |         +------------------+
                         | active_chat_id()  |
                         +-------------------+
```

---

## 17. Key Functions Reference

### Store Lifecycle

| Function                  | Signature                                          | Purpose                     |
|---------------------------|----------------------------------------------------|-----------------------------|
| `Store::new`              | `(db_path: &str) -> Result<Self, NeboError>`       | Create store + run migrations|
| `Store::conn`             | `(&self) -> Result<PooledConnection, NeboError>`   | Check out pool connection   |
| `create_pool`             | `(db_path: &str) -> Result<DbPool, NeboError>`     | Build r2d2 pool             |
| `run_migrations`          | `(conn: &Connection) -> Result<(), NeboError>`     | Apply pending migrations    |

### Agents

| Function                  | Returns                    | Purpose                          |
|---------------------------|----------------------------|----------------------------------|
| `list_agents`             | `Vec<Agent>`               | Paginated agent list             |
| `get_agent`               | `Option<Agent>`            | Get agent by ID                  |
| `get_agent_by_name`       | `Option<Agent>`            | Case-insensitive name lookup     |
| `create_agent`            | `Agent`                    | Insert new agent                 |
| `update_agent`            | `()`                       | Update agent fields              |
| `delete_agent`            | `()`                       | Delete agent by ID               |
| `set_agent_app_fields`    | `()`                       | Set app-specific columns         |
| `toggle_agent`            | `()`                       | Toggle is_enabled                |
| `upsert_agent_workflow`   | `()`                       | Create/update workflow binding   |
| `list_agent_workflows`    | `Vec<AgentWorkflow>`       | List workflow bindings           |
| `delete_agent_chats`      | `usize`                    | Cleanup: delete agent's chats    |
| `delete_agent_sessions`   | `usize`                    | Cleanup: delete agent's sessions |
| `delete_agent_memories`   | `usize`                    | Cleanup: delete agent's memories |

### Memories

| Function                               | Returns                 | Purpose                        |
|----------------------------------------|-------------------------|--------------------------------|
| `upsert_memory`                        | `()`                    | Create/update memory (verified)|
| `get_memory_by_key_and_user`           | `Option<Memory>`        | Scoped lookup                  |
| `get_tacit_memories_by_user`           | `Vec<Memory>`           | Tacit namespace + user         |
| `get_tacit_memories_with_min_confidence`| `Vec<Memory>`          | Confidence-filtered tacit      |
| `search_memories_by_user`              | `Vec<Memory>`           | LIKE search scoped to user     |
| `increment_memory_access`              | `()`                    | Bump access count              |
| `ensure_fts_healthy`                   | `()`                    | Startup FTS integrity check    |
| `search_memories_fts`                  | `Vec<(i64, f64)>`       | FTS5 search with rank          |

### Chats

| Function                           | Returns                       | Purpose                     |
|------------------------------------|-------------------------------|-----------------------------|
| `create_chat_for_session`          | `Chat`                        | New chat linked to session  |
| `create_chat_message_for_runner`   | `ChatMessage`                 | Insert with auto-chat-create|
| `get_chat_messages_budgeted`       | `Vec<ChatMessage>`            | Budget-based pagination     |
| `get_chat_messages_paginated`      | `Vec<ChatMessage>`            | Cursor-based pagination     |
| `get_recent_chat_messages_with_tools`| `Vec<ChatMessage>`           | Last N with all roles       |
| `list_chats_by_session_enriched`   | `Vec<(Chat, i64, String)>`    | With msg count + preview    |
| `find_tool_output`                 | `Option<(String, bool)>`      | Search tool results JSON    |

### Sessions

| Function                        | Returns            | Purpose                          |
|---------------------------------|--------------------|----------------------------------|
| `get_or_create_scoped_session`  | `Session`          | Upsert with scope conflict key   |
| `update_session_summary`        | `()`               | Post-compaction summary update   |
| `update_session_memory_flush`   | `()`               | Track memory extraction state    |
| `set_session_active_chat_id`    | `()`               | Switch active conversation       |
| `reset_session_counters`        | `()`               | Clear per-conversation state     |

---

## 18. Performance Considerations

### Indexes

Key indexes in the schema:

```sql
-- Users
idx_users_email ON users(email)

-- Chats
idx_chats_updated_at ON chats(updated_at DESC)
idx_chats_session_name ON chats(session_name, updated_at DESC)

-- Chat Messages (critical for pagination)
idx_chat_messages_chat_id ON chat_messages(chat_id)
idx_chat_messages_created_at ON chat_messages(created_at)
idx_chat_messages_chat_created ON chat_messages(chat_id, created_at DESC, id DESC)

-- Memories
idx_memory_chunks_memory_id ON memory_chunks(memory_id)
idx_memory_chunks_model ON memory_chunks(model)
idx_memory_embeddings_chunk_id ON memory_embeddings(chunk_id)

-- Agents
idx_agents_name ON agents(name)

-- Workflow Runs
idx_workflow_runs_workflow_id ON workflow_runs(workflow_id)
idx_workflow_runs_status ON workflow_runs(status)

-- Event Dedup
idx_event_dedup_created_at ON event_dedup(created_at)

-- Refresh Tokens
idx_refresh_tokens_user_id ON refresh_tokens(user_id)
idx_refresh_tokens_expires ON refresh_tokens(expires_at)
```

### Composite Index for Chat Messages

Migration 0085 added a critical composite index for chat message queries:

```sql
CREATE INDEX idx_chat_messages_chat_created
    ON chat_messages(chat_id, created_at DESC, id DESC);
```

This covers the most common access pattern: fetching messages for a specific chat in
reverse chronological order (for pagination, recent messages, budgeted fetch).

### Query Optimization Notes

- **Memory search**: Uses LIKE patterns for text search, which cannot use indexes. For
  performance-critical memory lookups, the FTS5 path (`search_memories_fts`) should be
  preferred.
- **Auth profile selection**: `get_best_auth_profile()` uses a multi-column ORDER BY with
  CASE expressions. For the typical small number of auth profiles (< 10), this is fine.
- **Budget-based pagination**: `get_chat_messages_budgeted()` fetches up to 50 messages
  newest-first, then applies the character budget in Rust. This avoids complex SQL but
  does read more data than strictly necessary.

### Page Cache

The 20MB page cache (`PRAGMA cache_size = -20000`) keeps approximately 5,000 4KB pages
in memory. For a typical Nebo database (10-100MB), this means most hot data stays cached.

---

## 19. Data Lifecycle and Cleanup

### Automatic Cleanup

Several query methods handle TTL-based cleanup:

| Function                       | Target               | Retention               |
|--------------------------------|----------------------|-------------------------|
| `delete_completed_tasks()`     | `pending_tasks`      | 7 days after completion |
| `cleanup_old_task_lists()`     | `pending_tasks`      | Configurable days       |
| `cleanup_event_dedup()`        | `event_dedup`        | Configurable TTL (secs) |
| `gc_license_keys()`           | `license_keys`       | Past expiration         |
| `delete_old_notifications()`   | `notifications`      | Before timestamp        |
| `cleanup_orphaned_runs()`      | `workflow_runs`      | Running -> cancelled    |

### Cascade Deletes

Foreign key cascades handle related data:

| Parent Table     | Cascade Target              | Trigger                         |
|------------------|-----------------------------|---------------------------------|
| `chats`          | `chat_messages`             | ON DELETE CASCADE               |
| `memories`       | `memory_chunks`             | ON DELETE CASCADE               |
| `memory_chunks`  | `memory_embeddings`         | ON DELETE CASCADE               |
| `users`          | `user_preferences`          | ON DELETE CASCADE               |
| `users`          | `refresh_tokens`            | ON DELETE CASCADE               |

### Agent Deletion Cascade

Agent deletion is handled programmatically (not via FK cascade) due to cross-table
relationships:

```
delete_agent(id)
  |
  +-- delete_agent_chats(id)       -- chats WHERE session_name LIKE 'agent:{id}:%'
  |     +-- (FK cascade)           -- chat_messages
  +-- delete_agent_sessions(id)    -- sessions WHERE scope='agent' AND scope_id=id
  +-- delete_agent_memories(id)    -- memories WHERE user_id LIKE '%:agent:{id}'
  |     +-- (FK cascade)           -- memory_chunks -> memory_embeddings
  +-- delete_agent_workflows(id)   -- agent_workflows WHERE agent_id=id
  +-- delete_agent_workflow_runs(id)-- workflow_runs WHERE workflow_id='agent:{id}'
  |     +-- (FK cascade)           -- workflow_activity_results
  +-- delete_cron_jobs_by_prefix   -- cron_jobs WHERE name LIKE '{prefix}%'
```

### Memory Compaction

Session compaction does not delete messages from the database. Instead:
1. A summary is generated from recent messages
2. `update_session_summary()` stores it on the session
3. `reset_session_counters()` resets token/message counts
4. The prompt assembly layer uses the summary instead of raw messages

### Memory Flush

Pre-compaction memory flush extracts tacit knowledge before conversation history is
summarized:
1. Agent reads recent messages
2. LLM extracts memory entries
3. `upsert_memory()` persists each entry
4. `update_session_memory_flush()` records the flush checkpoint

---

## 20. Schema Relationship Diagram

```
                                +------------------+
                                |     users        |
                                |  id (PK)         |
                                |  email           |
                                |  password_hash   |
                                +--------+---------+
                                         |
                    +--------------------+--------------------+
                    |                    |                    |
                    v                    v                    v
           +----------------+   +----------------+   +----------------+
           |user_preferences|   |  user_profiles |   |refresh_tokens  |
           | user_id (FK)   |   |  user_id (FK)  |   | user_id (FK)   |
           +----------------+   +----------------+   +----------------+

  +-------------------+         +------------------+
  |     agents        |         |    sessions      |
  |  id (PK)          |<------->|  id (PK)         |
  |  name             |  scope  |  name            |
  |  agent_md         |  scoped |  scope           |
  |  frontmatter      |    to   |  scope_id (FK)   |
  |  soul             |         |  summary         |
  |  is_app           |         |  active_chat_id--+------+
  |  app_ui_path      |         |  token_count     |      |
  |  app_binary_path  |         |  message_count   |      |
  +--------+----------+         +--------+---------+      |
           |                             |                 |
           | 1:N                         | 1:N             |
           v                             v                 v
  +-------------------+         +------------------+   +------------------+
  | agent_workflows   |         |     chats        |   |     chats        |
  |  id (PK)          |         |  id (PK)         |<--| (active_chat_id) |
  |  agent_id (FK)    |         |  session_name    |   +------------------+
  |  binding_name     |         |  title           |
  |  trigger_type     |         |  user_id         |
  |  trigger_config   |         +--------+---------+
  |  activities       |                  |
  |  connections      |                  | 1:N (FK CASCADE)
  +-------------------+                  v
                                +------------------+
                                |  chat_messages   |
                                |  id (PK)         |
                                |  chat_id (FK)    |
                                |  role            |
                                |  content         |
                                |  tool_calls      |
                                |  tool_results    |
                                |  token_estimate  |
                                |  day_marker      |
                                +------------------+

  +-------------------+
  |    memories       |
  |  id (PK, INTEGER) |
  |  namespace        |-----+
  |  key              |     | UNIQUE(namespace, key, user_id)
  |  value            |     |
  |  tags             |     |
  |  metadata         |     |
  |  user_id          |-----+
  |  access_count     |
  +--------+----------+
           |
           | 1:N (FK CASCADE)
           v
  +-------------------+         +------------------+
  |  memory_chunks    |         |  memories_fts    |
  |  id (PK, INTEGER) |         |  (FTS5 virtual)  |
  |  memory_id (FK)   |         |  key, value, tags|
  |  chunk_index      |         +------------------+
  |  text             |
  |  source           |         +------------------+
  |  model            |         | memory_chunks_fts|
  |  user_id          |         |  (FTS5 virtual)  |
  +--------+----------+         |  text, source    |
           |                    +------------------+
           | 1:1 (FK CASCADE)
           v
  +-------------------+         +------------------+
  | memory_embeddings |         | embedding_cache  |
  |  id (PK)          |         |  content_hash PK |
  |  chunk_id (FK)    |         |  embedding BLOB  |
  |  model            |         |  model           |
  |  dimensions       |         |  dimensions      |
  |  embedding BLOB   |         +------------------+
  +-------------------+

  +-------------------+         +------------------+
  |   auth_profiles   |         |  provider_models |
  |  id (PK)          |         |  id (PK)         |
  |  name             |         |  provider        |
  |  provider         |         |  model_id        |
  |  api_key          |         |  display_name    |
  |  model            |         |  context_window  |
  |  priority         |         |  input_price     |
  |  is_active        |         |  output_price    |
  |  cooldown_until   |         +------------------+
  |  error_count      |
  +-------------------+

  +-------------------+         +------------------+         +------------------+
  |    workflows      |         |  workflow_runs   |         | workflow_activity |
  |  id (PK)          |<--------|  workflow_id(FK) |<--------|   _results       |
  |  code             |         |  id (PK)         |         |  run_id (FK)     |
  |  name             |  1:N    |  trigger_type    |   1:N   |  activity_id     |
  |  definition       |         |  status          |         |  status          |
  |  skill_md         |         |  total_tokens    |         |  tokens_used     |
  |  napp_path        |         |  error           |         +------------------+
  +-------------------+         |  output          |
                                +------------------+

  +-------------------+         +------------------+
  |  pending_tasks    |         |   cron_jobs      |
  |  id (PK)          |         |  id (PK,INTEGER) |
  |  task_type        |         |  name (UNIQUE)   |
  |  status           |         |  schedule        |
  |  session_key      |         |  command         |
  |  prompt           |         |  enabled         |
  |  lane             |         +--------+---------+
  |  priority         |                  |
  |  list_id          |                  | 1:N
  |  seq              |                  v
  +-------------------+         +------------------+
                                |  cron_history    |
                                |  job_id (FK)     |
                                |  started_at      |
                                |  success         |
                                +------------------+

  +-------------------+         +------------------+
  | mcp_integrations  |         |   settings       |
  |  id (PK)          |         |  id = 1 (single) |
  |  name             |         |  autonomous_mode |
  |  server_type      |         |  auto_approve_*  |
  |  oauth_*          |         |  developer_mode  |
  +--------+----------+         +------------------+
           |
           | 1:N
           v
  +-------------------+         +------------------+
  | mcp_integration   |         | plugin_registry  |
  |  _credentials     |         |  id (PK)         |
  |  integration_id   |         |  slug (UNIQUE)   |
  |  credential_type  |         |  name            |
  |  credential_value |         |  binary_path     |
  +-------------------+         +--------+---------+
                                         |
                                         | 1:N
                                         v
                                +------------------+
                                | plugin_settings  |
                                |  plugin_id (FK)  |
                                |  setting_key     |
                                |  setting_value   |
                                |  is_secret       |
                                +------------------+

  +-------------------+         +------------------+         +------------------+
  |  entity_config    |         | commander_teams  |         |  a2ui_surfaces   |
  |  entity_type      |         |  id (PK)         |         |  id (PK)         |
  |  entity_id        |         |  name            |         |  agent_id        |
  |  heartbeat_*      |         |  color           |         |  view_id         |
  |  permissions      |         |  position_x/y    |         |  surface_type    |
  |  model_preference |         +--------+---------+         |  components      |
  +-------------------+                  |                    |  data_model      |
                                         | M:N               +------------------+
                                         v
                                +------------------+
                                |commander_team    |
                                |  _members        |
                                |  team_id (FK)    |
                                |  agent_id (FK)   |
                                +------------------+

  +-------------------+         +------------------+
  |  event_dedup      |         |  license_keys    |
  |  fingerprint (PK) |         |  artifact_id(PK) |
  |  source           |         |  encrypted_key   |
  |  created_at       |         |  expires_at      |
  +-------------------+         +------------------+
```

### Key Relationships

1. **Sessions own Chats**: `chats.session_name` references `sessions.name`. A session can
   have multiple chats (multi-chat mode). `sessions.active_chat_id` points to the current
   conversation.

2. **Agents own Sessions**: `sessions.scope = 'agent'` and `sessions.scope_id = agent.id`
   links sessions to their parent agent.

3. **Agents own Workflows**: `agent_workflows.agent_id` references `agents.id`. Each agent
   can have multiple workflow bindings.

4. **Memories are user-scoped**: `memories.user_id` can be a plain user ID or a compound
   key like `"{user_id}:agent:{agent_id}"` for per-agent memory isolation.

5. **Memories have Chunks and Embeddings**: `memory_chunks.memory_id` -> `memories.id`,
   `memory_embeddings.chunk_id` -> `memory_chunks.id`. Both cascade on delete.

6. **Workflow Runs track execution**: `workflow_runs.workflow_id` can be a workflow table ID
   or a synthetic key like `"agent:{agent_id}"` for agent-scoped runs.

---

## Appendix: Migration History Highlights

| Migration | Description                                              |
|-----------|----------------------------------------------------------|
| 0001      | Initial schema: users, preferences, refresh_tokens, leads|
| 0008      | Chat system: chats + chat_messages tables                |
| 0009      | Companion mode: sessions table                           |
| 0010      | Auth profiles for AI providers                           |
| 0016      | Vector embeddings: chunks, embeddings, FTS, cache        |
| 0022      | Pending tasks queue                                      |
| 0045      | Unified messages: migrate session_messages to chat_messages, add tool_calls/tool_results columns |
| 0050      | Workflows: definitions, tool bindings, runs              |
| 0054      | Rebuild memories FTS (fix broken content binding)        |
| 0066      | Commander visual editor (teams, edges, positions)        |
| 0069      | Multi-session support                                    |
| 0070      | Rename roles to agents                                   |
| 0075      | Session conversations (active_chat_id, chats.session_name)|
| 0077      | Recreate agents table (fix development breakage)         |
| 0078      | A2UI surfaces                                            |
| 0083      | Plugin registry .napp fields                             |
| 0084      | License key cache                                        |
| 0085      | Chat messages composite index (performance)              |
| 0088      | App agents (is_app, app_ui_path, app_binary_path)        |
| 0090      | Event deduplication table                                |
| 0092      | Agent soul column                                        |
