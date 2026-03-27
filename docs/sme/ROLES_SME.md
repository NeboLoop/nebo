# Role System & Workflows — Rust SME Reference

> Definitive reference for the Nebo Rust role and workflow system. Covers role
> definitions (ROLE.md + role.json), the workflow engine, trigger types (schedule,
> heartbeat, event, manual), the cron scheduler, event dispatcher, RoleWorker
> runtime, database schema, HTTP endpoints, and known issues.

**Last verified against source:** 2026-03-26

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [Role Definition Format](#2-role-definition-format)
3. [Role Configuration (role.json)](#3-role-configuration-rolejson)
4. [Role Loader](#4-role-loader)
5. [Database Schema](#5-database-schema)
6. [Database Queries](#6-database-queries)
7. [HTTP Endpoints](#7-http-endpoints)
8. [Role Lifecycle](#8-role-lifecycle)
9. [Workflow System](#9-workflow-system)
10. [Workflow Engine](#10-workflow-engine)
11. [Trigger System](#11-trigger-system)
12. [Cron Scheduler](#12-cron-scheduler)
13. [Event System](#13-event-system)
14. [RoleWorker Runtime](#14-roleworker-runtime)
15. [Workflow Binding Processing](#15-workflow-binding-processing)
16. [Toggle / Enable / Disable](#16-toggle--enable--disable)
17. [End-to-End Flows](#17-end-to-end-flows)
18. [Error Handling & Recovery](#18-error-handling--recovery)
19. [Known Issues](#19-known-issues)
20. [Key Files Reference](#20-key-files-reference)

---

## 1. Architecture Overview

```
                    ┌─────────────────────────────────────────────────────┐
                    │                   ROLE SYSTEM                       │
                    │                                                     │
  ┌──────────┐     │  ┌───────────┐     ┌──────────────┐                 │
  │ ROLE.md  │────>│  │  roles DB │<───>│ role.json /  │                 │
  │ (persona)│     │  │  (SQLite) │     │ frontmatter  │                 │
  └──────────┘     │  └─────┬─────┘     └──────┬───────┘                 │
                    │        │                  │                         │
                    │        ▼                  ▼                         │
                    │  ┌─────────────────────────────────┐               │
                    │  │       role_workflows table       │               │
                    │  │  (binding_name, trigger_type,    │               │
                    │  │   trigger_config, is_active,     │               │
                    │  │   activities, emit)              │               │
                    │  └──────────┬──────────────────────┘               │
                    └─────────────┼───────────────────────────────────────┘
                                  │
                    ┌─────────────┼───────────────────────────────────────┐
                    │             ▼      TRIGGER LAYER                   │
                    │                                                     │
  ┌──────────┐   ┌──────────────┐   ┌───────────────┐   ┌──────────┐   │
  │  Cron    │   │  RoleWorker  │   │EventDispatcher│   │  Manual  │   │
  │Scheduler │   │  (heartbeat  │   │  (real-time)  │   │  (REST)  │   │
  │(60s tick)│   │   interval)  │   │               │   │          │   │
  └────┬─────┘   └──────┬───────┘   └───────┬───────┘   └────┬─────┘   │
       │                │                    │                │         │
       └────────────────┴────────────────────┴────────────────┘         │
                         │                                               │
                         ▼                                               │
                ┌─────────────────┐                                      │
                │ WorkflowManager │  (trait: run / run_inline)           │
                │   Impl          │                                      │
                └────────┬────────┘                                      │
                         │                                               │
              ┌──────────┴──────────┐                                    │
              │                     │                                    │
              ▼                     ▼                                    │
     ┌────────────────┐   ┌─────────────────┐                           │
     │ Workflow Engine │   │  Standalone run  │                          │
     │ (activities)   │   │  (DB workflows)  │                          │
     └────────┬───────┘   └─────────────────┘                           │
              │                                                          │
              ▼                                                          │
     ┌─────────────┐       ┌─────────────┐                              │
     │  emit tool  │──────>│  EventBus   │──── feeds back ──────>───────┘
     │ (in activity│       │ (mpsc chan)  │
     │  context)   │       └─────────────┘
     └─────────────┘
```

**Core design:** A Role is a job description with a schedule. It contains:
- **ROLE.md** -- persona/instructions (system prompt identity)
- **role.json** -- operational config (workflow bindings, triggers, inputs, skills, pricing)

Workflow bindings are owned by roles. Each binding has a trigger type (schedule,
heartbeat, event, manual) and zero or more inline activities. Activities execute
as LLM agent tasks with full tool access.

---

## 2. Role Definition Format

### ROLE.md

Pure prose document that becomes the agent's system prompt identity. Two formats
are supported:

**Modern (preferred) -- pure prose, no frontmatter:**
```markdown
# Chief of Staff

You manage the executive's daily rhythm. You monitor email, calendar, and
project channels to surface what matters and handle what doesn't.
```

**Legacy -- YAML frontmatter:**
```markdown
---
name: "Chief of Staff"
description: "Executive assistant that manages daily rhythm"
skills:
  - "@nebo/skills/briefing-writer@^1.0.0"
pricing:
  model: "monthly_fixed"
  cost: 47.0
---

You manage the executive's daily rhythm.
```

**Parsing** (`crates/napp/src/role.rs:451-470`):

```rust
pub fn parse_role(content: &str) -> Result<RoleDef, NappError> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        // Pure prose -- no frontmatter
        return Ok(RoleDef { id: String::new(), name: String::new(),
                            description: String::new(), body: content.trim().to_string() });
    }
    // Legacy frontmatter format
    let (yaml_str, body) = split_frontmatter(content)?;
    let mut def: RoleDef = serde_yaml::from_str(&yaml_str)?;
    def.body = body;
    Ok(def)
}
```

**RoleDef struct:**
```rust
pub struct RoleDef {
    pub id: String,           // Empty for modern roles
    pub name: String,         // From frontmatter or empty
    pub description: String,  // From frontmatter or empty
    pub body: String,         // Markdown body after frontmatter
}
```

The handler also parses ROLE.md frontmatter separately in `roles.rs:36-56` using
a `RoleFrontmatter` struct that extracts `name`, `description`, `skills`, and
`pricing` fields for DB storage.

---

## 3. Role Configuration (role.json)

**File:** `crates/napp/src/role.rs`

The `role.json` file carries the operational structure: inline workflow
definitions, triggers, dependencies, pricing, and input fields.

### RoleConfig

```rust
pub struct RoleConfig {
    pub workflows: HashMap<String, WorkflowBinding>,  // binding_name -> binding
    pub skills: Vec<String>,                           // Qualified skill refs
    pub pricing: Option<RolePricing>,
    pub defaults: Option<RoleDefaults>,
    pub inputs: Vec<RoleInputField>,                   // Dynamic form fields
}
```

### WorkflowBinding

```rust
pub struct WorkflowBinding {
    pub trigger: RoleTrigger,                          // When this runs
    pub description: String,                           // Human-readable
    pub inputs: HashMap<String, serde_json::Value>,    // Default inputs
    pub activities: Vec<RoleActivity>,                 // Inline procedure (empty = chat-only)
    pub budget: RoleBudget,                            // Token constraints
    pub emit: Option<String>,                          // Event to announce on completion
}
```

**Key methods:**

- `has_activities()` -- returns true if `activities` is non-empty
- `to_workflow_json(name)` -- serializes to a `WorkflowDef`-compatible JSON string

### RoleTrigger (tagged enum)

```rust
#[serde(tag = "type")]
pub enum RoleTrigger {
    Schedule { cron: String },                    // Cron expression
    Heartbeat { interval: String, window: Option<String> },  // "30m|08:00-18:00"
    Event { sources: Vec<String> },               // ["email.*", "cal.changed"]
    Manual,                                        // User-initiated
}
```

### RoleActivity

```rust
pub struct RoleActivity {
    pub id: String,
    pub intent: String,           // Task description
    pub skills: Vec<String>,      // Skill references
    pub mcps: Vec<String>,        // MCP server references
    pub cmds: Vec<String>,        // Command references (emit, exit)
    pub model: String,            // Model override
    pub steps: Vec<String>,
    pub token_budget: RoleTokenBudget,  // Default max: 4096
    pub on_error: RoleOnError,          // Default: retry 1, fallback NotifyOwner
}
```

### RoleInputField

```rust
pub struct RoleInputField {
    pub key: String,              // Unique key for workflows
    pub label: String,            // Display label
    pub name: Option<String>,     // NeboLoop alias for key
    pub description: Option<String>,
    pub field_type: String,       // text, textarea, number, select, checkbox, radio
    pub required: bool,
    pub default: Option<serde_json::Value>,
    pub placeholder: Option<String>,
    pub options: Vec<RoleInputOption>,
}
```

### Example role.json

```json
{
  "workflows": {
    "morning-briefing": {
      "trigger": { "type": "schedule", "cron": "0 7 * * *" },
      "description": "Daily morning briefing",
      "activities": [{
        "id": "gather",
        "intent": "Gather news and calendar events",
        "model": "sonnet",
        "steps": ["Fetch top headlines", "Check today's calendar"]
      }],
      "budget": { "total_per_run": 5000 },
      "emit": "briefing.ready"
    },
    "day-monitor": {
      "trigger": { "type": "heartbeat", "interval": "30m", "window": "08:00-18:00" }
    },
    "interrupt": {
      "trigger": { "type": "event", "sources": ["calendar.changed", "email.urgent"] }
    }
  },
  "skills": ["@nebo/skills/briefing-writer@^1.0.0"],
  "pricing": { "model": "monthly_fixed", "cost": 47.0 },
  "inputs": [
    { "key": "timezone", "label": "Timezone", "type": "text", "default": "US/Eastern" }
  ]
}
```

### Validation (`validate_role_config`)

1. Event triggers must have at least one source
2. Activity IDs must be unique within each binding
3. Skill refs that aren't qualified names (`@org/skills/name` or `SKIL-XXXX-XXXX`) get a warning but don't reject

---

## 4. Role Loader

**File:** `crates/napp/src/role_loader.rs`

Filesystem scanner that loads roles from two directories:

| Directory | Source Type | Description |
|-----------|-----------|-------------|
| `<data_dir>/nebo/roles/` | `Installed` | Marketplace .napp archives (sealed) |
| `<data_dir>/user/roles/` | `User` | User-created loose files |

### LoadedRole

```rust
pub struct LoadedRole {
    pub role_def: RoleDef,              // From ROLE.md
    pub config: Option<RoleConfig>,     // From role.json (if exists)
    pub source: RoleSource,             // Installed or User
    pub napp_path: Option<PathBuf>,
    pub source_path: PathBuf,
    pub version: Option<String>,        // From manifest.json
}
```

### Scanning

- `load_from_dir(dir, source)` -- loads ROLE.md + optional role.json + manifest.json
- `scan_installed_roles(dir)` -- walks for `ROLE.md` marker files
- `scan_user_roles(dir)` -- shallow read_dir, checks for `ROLE.md` in each subdirectory
- Falls back to directory name if ROLE.md has no frontmatter name

The `list_roles` handler merges DB roles with filesystem roles, deduplicating by name.

---

## 5. Database Schema

### roles

```sql
CREATE TABLE roles (
    id TEXT PRIMARY KEY,
    kind TEXT,                          -- Marketplace code (was 'code', renamed in 0058)
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    role_md TEXT NOT NULL,              -- Full ROLE.md content
    frontmatter TEXT NOT NULL,          -- role.json as JSON string
    pricing_model TEXT,
    pricing_cost REAL,
    is_enabled INTEGER NOT NULL DEFAULT 1,
    installed_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    napp_path TEXT,                     -- Filesystem path (added 0051)
    input_values TEXT NOT NULL DEFAULT '{}'  -- User-supplied values (added 0064)
);
CREATE INDEX idx_roles_kind ON roles(kind);
```

**Key points:**
- `kind` allows multiple instances of the same marketplace role (not UNIQUE)
- `frontmatter` stores the full role.json content as a JSON string
- `input_values` stores user-supplied form values, separate from the schema in frontmatter

### role_workflows

```sql
CREATE TABLE role_workflows (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    role_id TEXT NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    binding_name TEXT NOT NULL,
    workflow_ref TEXT NOT NULL DEFAULT '',   -- Legacy, unused
    workflow_id TEXT,                         -- Legacy, unused
    trigger_type TEXT NOT NULL,              -- schedule, heartbeat, event, manual
    trigger_config TEXT NOT NULL,            -- Cron expr / "30m|08:00-18:00" / "email.*,cal.changed"
    description TEXT,
    inputs TEXT,                             -- JSON
    is_active INTEGER NOT NULL DEFAULT 1,
    last_fired TEXT,                         -- ISO 8601 or Unix timestamp (added 0056)
    emit TEXT,                               -- Event name to emit (added 0059)
    activities TEXT,                         -- JSON of activity definitions (added 0060)
    UNIQUE(role_id, binding_name)
);
CREATE INDEX idx_role_workflows_role ON role_workflows(role_id);
```

**Key points:**
- `is_active` controls whether a binding fires -- toggled via REST endpoint
- `activities` stores the inline activity definitions as JSON (denormalized from frontmatter)
- FK cascade: deleting a role auto-deletes its role_workflows

### workflows (standalone)

```sql
CREATE TABLE workflows (
    id TEXT PRIMARY KEY,
    code TEXT UNIQUE,
    name TEXT NOT NULL,
    version TEXT NOT NULL DEFAULT '1.0',
    definition TEXT NOT NULL,               -- workflow.json content
    skill_md TEXT,
    manifest TEXT,
    is_enabled INTEGER NOT NULL DEFAULT 1,
    installed_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    napp_path TEXT                           -- Filesystem path (added 0051)
);
```

### workflow_runs

```sql
CREATE TABLE workflow_runs (
    id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL,               -- "role:{role_id}" for inline runs, workflow ID for standalone
    trigger_type TEXT NOT NULL,              -- schedule, event, manual, cron, heartbeat
    trigger_detail TEXT,
    status TEXT NOT NULL DEFAULT 'running',  -- running, completed, failed, cancelled, exited
    inputs TEXT,                             -- JSON
    current_activity TEXT,
    total_tokens_used INTEGER DEFAULT 0,
    error TEXT,
    error_activity TEXT,
    session_key TEXT,
    output TEXT,                             -- Accumulated activity output (added 0062)
    started_at INTEGER NOT NULL DEFAULT (unixepoch()),
    completed_at INTEGER
);
```

**Key point:** No FK on `workflow_id` -- removed in migration 0061 to support
inline role runs where `workflow_id = "role:{role_id}"`.

### workflow_activity_results

```sql
CREATE TABLE workflow_activity_results (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
    activity_id TEXT NOT NULL,
    status TEXT NOT NULL,                    -- completed, failed, exited
    tokens_used INTEGER DEFAULT 0,
    attempts INTEGER DEFAULT 1,
    error TEXT,
    started_at INTEGER NOT NULL,
    completed_at INTEGER
);
```

### cron_jobs

```sql
CREATE TABLE cron_jobs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT UNIQUE NOT NULL,
    schedule TEXT NOT NULL,                  -- Cron expression
    command TEXT DEFAULT '',                 -- Shell cmd / workflow ID / "role:id:binding"
    task_type TEXT DEFAULT 'bash',           -- bash, shell, agent, workflow, role_workflow
    message TEXT DEFAULT '',                 -- Agent prompt
    deliver TEXT DEFAULT '',
    instructions TEXT,                       -- System prompt for agent tasks (added 0042)
    enabled INTEGER DEFAULT 1,
    last_run DATETIME,
    run_count INTEGER DEFAULT 0,
    last_error TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
```

### cron_history

```sql
CREATE TABLE cron_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id INTEGER NOT NULL REFERENCES cron_jobs(id) ON DELETE CASCADE,
    started_at DATETIME NOT NULL,
    finished_at DATETIME,
    success INTEGER DEFAULT 0,
    output TEXT,
    error TEXT
);
```

### entity_config

```sql
CREATE TABLE entity_config (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_type TEXT NOT NULL CHECK(entity_type IN ('main', 'role', 'channel')),
    entity_id   TEXT NOT NULL,
    heartbeat_enabled          INTEGER,     -- NULL=inherit, 0/1
    heartbeat_interval_minutes INTEGER,
    heartbeat_content          TEXT,
    heartbeat_window_start     TEXT,         -- HH:MM
    heartbeat_window_end       TEXT,
    permissions     TEXT,                    -- JSON: {"web": true, ...}
    resource_grants TEXT,                    -- JSON
    model_preference    TEXT,
    personality_snippet TEXT,
    allowed_paths       TEXT,                -- JSON array of allowed dirs (added 0065)
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(entity_type, entity_id)
);
```

### Key Relationships

```
roles (1) ──→ (many) role_workflows ──→ (via trigger registration) cron_jobs
roles ──→ (via "role:{id}") workflow_runs ──→ (many) workflow_activity_results
cron_jobs (1) ──→ (many) cron_history
entity_config ── standalone per (entity_type, entity_id) pair
```

---

## 6. Database Queries

### Role Queries (`crates/db/src/queries/roles.rs`)

| Method | SQL | Purpose |
|--------|-----|---------|
| `list_roles(limit, offset)` | `SELECT ... FROM roles ORDER BY installed_at DESC` | List all roles |
| `count_roles()` | `SELECT COUNT(*) FROM roles` | Total role count |
| `get_role(id)` | `SELECT ... FROM roles WHERE id = ?1` | Get single role |
| `create_role(...)` | `INSERT INTO roles ... RETURNING ...` | Create role |
| `update_role(...)` | `UPDATE roles SET ...` | Update role fields |
| `delete_role(id)` | `DELETE FROM roles WHERE id = ?1` | Delete (cascades to role_workflows) |
| `toggle_role(id)` | `UPDATE roles SET is_enabled = NOT is_enabled` | Toggle enabled state |
| `set_role_enabled(id, bool)` | `UPDATE roles SET is_enabled = ?1` | Set explicit state |
| `set_role_napp_path(id, path)` | `UPDATE roles SET napp_path = ?1` | Set filesystem path |
| `update_role_input_values(id, json)` | `UPDATE roles SET input_values = ?1` | Store user inputs |
| `role_installed_by_name(name)` | `SELECT COUNT(*) ... WHERE LOWER(name) = LOWER(?1)` | Check if installed |

### Role Workflow Queries

| Method | SQL | Purpose |
|--------|-----|---------|
| `upsert_role_workflow(...)` | `INSERT ... ON CONFLICT DO UPDATE` | Create or update binding |
| `list_role_workflows(role_id)` | `SELECT ... WHERE role_id = ?1` | List bindings for role |
| `delete_single_role_workflow(role_id, binding)` | `DELETE ... WHERE role_id AND binding_name` | Delete one binding |
| `toggle_role_workflow(role_id, binding)` | `UPDATE SET is_active = NOT is_active` | Toggle + return new state |
| `delete_role_workflows(role_id)` | `DELETE ... WHERE role_id = ?1` | Delete all for role |
| `list_active_event_triggers()` | `WHERE trigger_type='event' AND is_active=1 AND r.is_enabled=1` | Active event bindings |
| `update_role_workflow_last_fired(...)` | `UPDATE SET last_fired = ?1` | Record last execution |
| `list_emit_sources()` | `WHERE emit IS NOT NULL AND is_active=1` | Available event sources |
| `delete_cron_jobs_by_prefix(prefix)` | `DELETE ... WHERE name LIKE ?1%` | Cleanup cron jobs for role |

### Workflow Run Queries (`crates/db/src/queries/workflows.rs`)

| Method | Purpose |
|--------|---------|
| `create_workflow_run(...)` | Create run record |
| `update_workflow_run(...)` | Update status/activity (dynamic SET) |
| `complete_workflow_run(...)` | Mark completed with output |
| `list_workflow_runs(workflow_id, limit, offset)` | List runs (ordered by started_at DESC) |
| `get_workflow_run(id)` | Get single run |
| `cleanup_orphaned_runs()` | Mark "running" as "cancelled" on restart |
| `create_activity_result(...)` | Record activity completion/failure |
| `list_activity_results(run_id)` | Results for a run (ordered by started_at ASC) |
| `role_workflow_stats(role_id)` | Aggregate stats (total/completed/failed/cancelled/running/tokens) |
| `role_recent_errors(role_id, limit)` | Recent failure messages |

---

## 7. HTTP Endpoints

### Roles -- `/api/v1/roles`

| Method | Path | Handler | Description |
|--------|------|---------|-------------|
| GET | `/roles` | `list_roles` | List all roles (DB + filesystem merged) |
| POST | `/roles` | `create_role` | Create role from ROLE.md + optional role.json |
| GET | `/roles/active` | `list_active_roles` | Currently active roles from RoleRegistry |
| GET | `/roles/event-sources` | `list_event_sources` | Available emit names from active bindings |
| GET | `/roles/{id}` | `get_role` | Get role with version and normalized inputFields |
| PUT | `/roles/{id}` | `update_role` | Update role fields |
| DELETE | `/roles/{id}` | `delete_role` | Delete role + cleanup triggers + filesystem |
| POST | `/roles/{id}/toggle` | `toggle_role` | Toggle is_enabled, start/stop worker |
| POST | `/roles/{id}/activate` | `activate_role` | Activate + add to registry + start worker |
| POST | `/roles/{id}/deactivate` | `deactivate_role` | Deactivate + remove from registry + stop worker |
| POST | `/roles/{id}/duplicate` | `duplicate_role` | Deep copy with "(Copy)" suffix, auto-activate |
| POST | `/roles/{id}/install-deps` | `install_deps` | Force-resolve skill dependencies |
| POST | `/roles/{id}/check-update` | `check_role_update` | Check NeboLoop for newer version |
| POST | `/roles/{id}/apply-update` | `apply_role_update` | Download and apply latest from NeboLoop |
| POST | `/roles/{id}/reload` | `reload_role` | Re-read ROLE.md + role.json from filesystem |
| POST | `/roles/{id}/setup` | `trigger_role_setup` | Broadcast setup event for frontend wizard |
| PUT | `/roles/{id}/inputs` | `update_role_inputs` | Store user-supplied input values |
| GET | `/roles/{id}/stats` | `role_stats` | Aggregate workflow run statistics |
| GET | `/roles/{id}/runs` | `list_role_runs` | List workflow runs for role |
| POST | `/roles/{id}/chat` | `chat_with_role` | Send message to role's agent |

### Role Workflows -- `/api/v1/roles/{id}/workflows`

| Method | Path | Handler | Description |
|--------|------|---------|-------------|
| GET | `/roles/{id}/workflows` | `list_role_workflows` | List bindings for a role |
| POST | `/roles/{id}/workflows` | `create_role_workflow` | Create new binding |
| PUT | `/roles/{id}/workflows/{binding}` | `update_role_workflow` | Update existing binding |
| DELETE | `/roles/{id}/workflows/{binding}` | `delete_role_workflow` | Delete binding + triggers |
| POST | `/roles/{id}/workflows/{binding}/toggle` | `toggle_role_workflow` | Toggle is_active + register/unregister triggers |

### Standalone Workflows -- `/api/v1/workflows`

| Method | Path | Description |
|--------|------|-------------|
| GET | `/workflows` | List workflows |
| POST | `/workflows` | Create workflow |
| GET | `/workflows/{id}` | Get workflow |
| PUT | `/workflows/{id}` | Update workflow |
| DELETE | `/workflows/{id}` | Delete workflow |
| POST | `/workflows/{id}/toggle` | Toggle enabled |
| POST | `/workflows/{id}/run` | Execute (body: `{inputs}`) |
| GET | `/workflows/{id}/runs` | List runs |
| GET | `/workflows/{id}/runs/{runId}` | Run status |
| POST | `/workflows/{id}/runs/{runId}/cancel` | Cancel run |

### Entity Config -- `/api/v1/entity-config`

| Method | Path | Description |
|--------|------|-------------|
| GET | `/entity-config/{type}/{id}` | Get resolved config |
| PUT | `/entity-config/{type}/{id}` | Patch-update overrides |
| DELETE | `/entity-config/{type}/{id}` | Reset to inherited defaults |

---

## 8. Role Lifecycle

### Create (`create_role`)

1. Parse ROLE.md frontmatter (name, description, skills, pricing)
2. Generate UUID v4 as role ID
3. Merge skills from ROLE.md frontmatter and role.json
4. Build frontmatter JSON (full role.json with merged skills)
5. Insert into `roles` table
6. Write ROLE.md, role.json, and manifest.json to `user/roles/{name}/`
7. Set `napp_path` on the role record
8. Process workflow bindings (upsert to role_workflows, register triggers)
9. Resolve dependency cascade (skills)
10. Broadcast `role_installed` WebSocket event

### Activate (`activate_role`)

1. Set `is_enabled = true` in DB
2. Parse RoleConfig from frontmatter
3. Build `ActiveRole` struct and insert into `RoleRegistry` (in-memory `RwLock<HashMap>`)
4. Start `RoleWorker` (registers all triggers)
5. Register agent in NeboLoop personal loop (async, best-effort)
6. Broadcast `role_activated` WebSocket event

### Deactivate (`deactivate_role`)

1. Set `is_enabled = false` in DB
2. Stop `RoleWorker` (cancels all triggers, running workflows)
3. Remove from `RoleRegistry`
4. Deregister agent from NeboLoop (async, best-effort)
5. Broadcast `role_deactivated` WebSocket event

### Delete (`delete_role`)

1. Stop `RoleWorker`
2. Remove from `RoleRegistry`
3. Unregister all cron triggers
4. Unsubscribe all event triggers from EventDispatcher
5. Delete from `roles` table (cascades to `role_workflows`)
6. Clean up filesystem (napp_path, nebo/roles/, user/roles/)
7. Deregister from NeboLoop (async)
8. Broadcast `role_uninstalled`

### Update (`update_role`)

1. Load existing role from DB
2. Parse ROLE.md frontmatter
3. Body fields take priority over frontmatter (allows renaming without editing ROLE.md)
4. Preserve existing frontmatter workflows
5. Persist to DB
6. Sync in-memory role_registry if role is active
7. Broadcast `role_updated`

### Duplicate (`duplicate_role`)

1. Load source role
2. Generate new UUID, append " (Copy)" to name
3. Update frontmatter name in ROLE.md
4. Insert new role record
5. Copy all role_workflow bindings from source
6. Auto-activate the copy
7. Start worker, broadcast events

---

## 9. Workflow System

### WorkflowDef (`crates/workflow/src/parser.rs`)

```rust
pub struct WorkflowDef {
    pub version: String,
    pub id: String,
    pub name: String,
    pub inputs: HashMap<String, InputParam>,
    pub activities: Vec<Activity>,
    pub dependencies: Dependencies,
    pub budget: Budget,
}

pub struct Activity {
    pub id: String,
    pub intent: String,
    pub skills: Vec<String>,
    pub mcps: Vec<String>,
    pub cmds: Vec<String>,
    pub model: String,
    pub steps: Vec<String>,
    pub token_budget: TokenBudget,  // Default max: 4096
    pub on_error: OnError,           // Default: retry 1, NotifyOwner
}

pub enum Fallback { NotifyOwner, Skip, Abort }
```

**Triggers are no longer part of workflow.json** -- they are owned by Roles via
role.json. Legacy `triggers` fields are silently ignored on parse (via
`#[serde(default)]`).

### Validation (`validate_workflow`)

1. `id` and `name` must be non-empty
2. At least one activity required
3. Activity IDs must be unique
4. If `budget.total_per_run > 0`, activity budgets sum must not exceed it

### WorkflowManager Trait (`crates/tools/src/workflows/manager.rs`)

```rust
pub trait WorkflowManager: Send + Sync {
    fn list() -> Vec<WorkflowInfo>;
    fn install(code) -> Result<WorkflowInfo>;
    fn uninstall(id) -> Result<()>;
    fn resolve(name_or_id) -> Result<WorkflowInfo>;
    fn run(id, inputs, trigger_type) -> Result<String>;     // Returns run_id
    fn run_inline(def_json, inputs, trigger, role_id, emit) -> Result<String>;
    fn run_status(run_id) -> Result<WorkflowRunInfo>;
    fn list_runs(workflow_id, limit) -> Vec<WorkflowRunInfo>;
    fn toggle(id) -> Result<bool>;
    fn create(name, definition) -> Result<WorkflowInfo>;
    fn cancel(run_id) -> Result<()>;
    fn cancel_runs_for_role(role_id) -> ();
}
```

### WorkflowManagerImpl (`crates/server/src/workflow_manager.rs`)

Concrete implementation. Key internals:

- `active_runs: Mutex<HashMap<String, CancellationToken>>` -- token per running workflow
- `role_runs: Mutex<HashMap<String, Vec<String>>>` -- role_id to list of active run_ids
- `event_bus: Option<EventBus>` -- for injecting emit tool
- `skill_loader: Option<Arc<Loader>>` -- for resolving skill content

**run_inline() flow:**
1. Parse definition from JSON string
2. Create `workflow_runs` record with `workflow_id = "role:{role_id}"`
3. Store CancellationToken + track in role_runs
4. `tokio::spawn` background task:
   - Get provider, build tool wrappers, load skill content
   - Call `workflow::engine::execute_workflow()`
   - Update run status on completion/failure
   - Broadcast WebSocket events
5. Return run_id immediately

---

## 10. Workflow Engine

**File:** `crates/workflow/src/engine.rs`

### execute_workflow()

```rust
pub async fn execute_workflow(
    def: &WorkflowDef,
    inputs: Value,
    trigger_type: &str,
    trigger_detail: Option<&str>,
    store: &Arc<Store>,
    provider: &dyn Provider,
    resolved_tools: &[Box<dyn DynTool>],
    existing_run_id: Option<&str>,
    cancel_token: Option<&CancellationToken>,
    skill_content: Option<&HashMap<String, String>>,
    event_bus: Option<&EventBus>,
    emit_source: Option<String>,
    progress_tx: Option<UnboundedSender<WorkflowProgress>>,
) -> Result<(String, String), WorkflowError>
```

**Activity loop** for each activity in `def.activities`:

1. **Cancellation check** -- bail if token is cancelled
2. **Update run** -- set `current_activity` in DB
3. **Inject tools** -- emit tool (if event_bus available) + exit tool (always)
4. **Execute with retry** -- up to `activity.on_error.retry` attempts
5. **Record result** -- `workflow_activity_results` row
6. **Accumulate context** -- prior results passed to next activity as `[Activity 'X' result]: ...`
7. **Circuit breaker** -- 3 consecutive failures with same error pattern = abort
8. **Budget check** -- fail if total_tokens > `budget.total_per_run`
9. **Error handling** -- `Skip` continues, `Abort`/`NotifyOwner` fails the run

### execute_activity()

Single activity execution -- a lean agentic loop (no steering, no memory, no personality):

1. Build system prompt via `build_activity_prompt()`
2. Build tool definitions from available tools
3. Start with user message = activity.intent
4. **Loop** (max 20 iterations):
   - Build ChatRequest -> `provider.stream()`
   - Accumulate text + tool_calls
   - If no tool_calls -> return (response, tokens)
   - Execute each tool call
   - Check for `EXIT_SENTINEL` (`"__WORKFLOW_EXIT__:"`) in results -> early exit
   - Check for repeated "tool not found" (3x consecutive = abort)
   - Append tool results to messages
   - Continue loop

### System Prompt Construction (build_activity_prompt)

Order:
1. **Skills** -- injected from SKILL.md content (matched by activity.skills)
2. **Available Tools** -- explicit list prevents hallucination
3. **Task** -- activity.intent
4. **Steps** -- numbered list
5. **Inputs** -- key-value pairs (excluding `_` prefixed keys)
6. **Prior Results** -- accumulated from prior activities
7. **Workflow Controls** -- exit/emit hints (from activity.cmds)
8. **Browser Guide** -- injected if web tool is available
9. **Output** -- emit instruction on last activity (if emit_source set)

### WorkflowError

```rust
pub enum WorkflowError {
    Parse(String),
    Validation(String),
    MissingDependency(String),
    UnresolvedInterface(String),
    MaxIterations(String),
    BudgetExceeded { activity_id, used, limit },
    ActivityFailed(String, String),
    NotFound(String),
    Database(String),
    Provider(String),
    Exited(String),        // Early exit by agent decision -- not a failure
    Cancelled,
    CircuitBreak(String),
    Other(String),
}
```

---

## 11. Trigger System

**File:** `crates/workflow/src/triggers.rs`

### Trigger Types

| Type | Config Format | Registration | Execution Path |
|------|--------------|-------------|----------------|
| `schedule` | Cron expression (`"0 7 * * 1-5"`) | Creates `cron_jobs` row | Cron scheduler -> `execute_role_workflow_task()` |
| `heartbeat` | `"30m"` or `"30m\|08:00-18:00"` | RoleWorker spawns `tokio::interval` task | RoleWorker -> `manager.run_inline()` |
| `event` | Comma-separated patterns (`"email.*,cal.changed"`) | EventDispatcher subscription | EventDispatcher -> `manager.run_inline()` |
| `manual` | Empty string | No registration | User triggers via REST API |

### Schedule Trigger Registration

```rust
pub fn register_role_triggers(role_id: &str, bindings: &[RoleWorkflow], store: &Store) {
    for binding in bindings {
        if binding.trigger_type == "schedule" {
            let name = format!("role-{}-{}", role_id, binding.binding_name);
            let command = format!("role:{}:{}", role_id, binding.binding_name);
            store.upsert_cron_job(&name, &binding.trigger_config, &command, "role_workflow", ...);
        }
    }
}
```

### Unregistration

| Function | Scope |
|----------|-------|
| `unregister_single_role_trigger(role_id, binding)` | One cron job: `"role-{id}-{binding}"` |
| `unregister_role_triggers(role_id)` | All cron jobs with prefix `"role-{id}-"` |
| `unregister_triggers(workflow_id)` | Standalone workflow trigger: `"workflow-{id}"` |

All use DB deletion by cron_job name/prefix.

---

## 12. Cron Scheduler

**File:** `crates/server/src/scheduler.rs`

### Spawn

```rust
pub fn spawn(store, runner, hub, snapshot_store, workflow_manager)
```

10s boot delay -> 60s tick loop. Each tick:
1. Cleanup completed tasks older than 7 days (`delete_completed_tasks()`)
2. Query all `enabled` cron_jobs
3. For each job: parse schedule, check if due, dispatch by task_type

### Cron Resolution

```rust
let schedule: Schedule = job.schedule.parse()?;
let next = schedule.after(&last_run).next()?;
if next > now { continue; }  // Not due
```

`last_run` parsing: tries `i64` Unix timestamp, then `NaiveDateTime` format `"%Y-%m-%d %H:%M:%S"`, defaults to epoch 0. Schedule normalization uses `tools::RoleTool::normalize_cron()` to handle stale 5-field expressions.

### Task Type Dispatch

| task_type | Command format | Execution |
|-----------|----------------|-----------|
| `"bash"` / `"shell"` / `""` | Shell command | `sh -c {command}` subprocess |
| `"agent"` | Prompt text | `runner.run()` with `Origin::System`, session `"cron-{name}"` |
| `"workflow"` | Workflow ID | `manager.run(id, null, "cron")` |
| `"role_workflow"` | `"role:{role_id}:{binding}"` | Parse -> load role -> `manager.run_inline()` |

### execute_role_workflow_task() -- **CRITICAL PATH**

```rust
async fn execute_role_workflow_task(
    manager: &dyn WorkflowManager,
    store: &Store,
    command: &str,  // "role:{role_id}:{binding_name}"
) -> (bool, String, Option<String>) {
    let parts = command.splitn(3, ':');  // ["role", role_id, binding_name]
    let role = store.get_role(role_id)?;
    let config = napp::role::parse_role_config(&role.frontmatter)?;
    let binding = config.workflows.get(binding_name)?;
    if !binding.has_activities() { return error; }
    let def_json = binding.to_workflow_json(binding_name);
    let emit_source = binding.emit.map(|e| format!("{}.{}", slug, e));
    manager.run_inline(def_json, inputs, "schedule", role_id, emit_source).await
}
```

### History Tracking

```rust
store.create_cron_history(job.id);                    // Before execution
store.update_cron_job_last_run(job.id, err_msg);      // After execution
store.update_cron_history(h.id, success, output, error);  // After execution
```

---

## 13. Event System

### EventBus (`crates/tools/src/events.rs`)

```rust
pub struct Event {
    pub source: String,               // "email.urgent", "chief-of-staff.briefing.ready"
    pub payload: serde_json::Value,
    pub origin: String,               // Trace: "workflow:email-triage:run-550e"
    pub timestamp: u64,               // Unix epoch seconds
}

pub struct EventBus {
    tx: mpsc::UnboundedSender<Event>,
}
```

Best-effort delivery via unbounded channel. Events dropped if receiver is gone.

### Emit Tool

Auto-injected into every workflow activity. Not in the normal tool registry.
```rust
// Input: { "source": "email.urgent", "payload": {...} }
// Creates Event and sends to EventBus
```

When `emit_source` is set on the last activity, the engine instructs the LLM
to call emit with that source name and its actual output as payload.

### EventDispatcher (`crates/workflow/src/events.rs`)

```rust
pub struct EventSubscription {
    pub pattern: String,                    // "email.*" or "email.urgent"
    pub default_inputs: serde_json::Value,
    pub role_source: String,                // Role ID
    pub binding_name: String,
    pub definition_json: Option<String>,    // Inline workflow JSON
    pub emit_source: Option<String>,
}

pub struct EventDispatcher {
    subscriptions: Arc<RwLock<Vec<EventSubscription>>>,
}
```

**Spawn loop:**
```rust
while let Some(event) = rx.recv().await {
    let matches = self.match_event(&event).await;
    for sub in matches {
        let mut inputs = sub.default_inputs.clone();
        inputs["_event_source"] = json!(event.source);
        inputs["_event_payload"] = event.payload.clone();
        inputs["_event_origin"] = json!(event.origin);
        manager.run_inline(def_json, inputs, "event", &sub.role_source, ...).await;
    }
}
```

**Pattern matching:**
- Exact: `"email.urgent"` matches `"email.urgent"`
- Wildcard suffix: `"email.*"` matches `"email.urgent"`, `"email.info"`, etc.
- No deeper wildcard support (e.g., no `"**"` or `"email.*.done"`)

**Methods:**
- `subscribe(sub)` -- add a subscription
- `unsubscribe_binding(role_id, binding_name)` -- remove subscriptions for one binding
- `unsubscribe_role(role_id)` -- remove all subscriptions for a role
- `set_subscriptions(subs)` -- replace all subscriptions
- `clear()` -- remove all

---

## 14. RoleWorker Runtime

**File:** `crates/agent/src/role_worker.rs`

### RoleWorker

```rust
pub struct RoleWorker {
    pub role_id: String,
    pub name: String,
    cancel: CancellationToken,
    event_dispatcher: Arc<EventDispatcher>,
    workflow_manager: Arc<dyn WorkflowManager>,
}
```

### start()

1. Load role workflow bindings from DB (`list_role_workflows`)
2. Load role config from DB frontmatter (`parse_role_config`)
3. Register schedule triggers via `register_role_triggers()` (creates cron_jobs)
4. For each binding, spawn trigger task by type:

| trigger_type | Worker behavior |
|-------------|-----------------|
| `schedule` | No-op (handled by cron scheduler via registered cron_jobs) |
| `heartbeat` | Spawns `tokio::interval` task -> `run_inline()` on tick |
| `event` | Subscribes patterns to EventDispatcher |
| `manual` | No-op (user-triggered via REST) |

### Heartbeat Worker Detail

```rust
let (duration, window) = parse_heartbeat(&binding.trigger_config);
// Config format: "30m" or "30m|08:00-18:00"

tokio::spawn(async move {
    let mut interval = tokio::time::interval(duration);
    interval.tick().await;  // Skip first immediate tick
    loop {
        select! {
            _ = interval.tick() => {
                if let Some((start, end)) = &window {
                    let now = chrono::Local::now().time();
                    if now < *start || now > *end { continue; }
                }
                mgr.run_inline(def_json, inputs, "heartbeat", &role, emit_source).await;
            }
            _ = token.cancelled() => break,
        }
    }
});
```

Requires `has_activities()` -- heartbeat bindings without activities are skipped.

### Event Worker Detail

For each source pattern in `trigger_config.split(',')`:
```rust
dispatcher.subscribe(EventSubscription {
    pattern,
    default_inputs: inputs.clone(),
    role_source: role_id.clone(),
    binding_name: binding.binding_name.clone(),
    definition_json: def_json.clone(),
    emit_source: event_emit_source.clone(),
}).await;
```

### stop()

```rust
fn stop(&self, store: &Store) {
    self.cancel.cancel();                           // Cancel all spawned tasks
    workflow::triggers::unregister_role_triggers();  // Remove cron jobs
    mgr.cancel_runs_for_role(&role_id);             // Cancel running workflows
    dispatcher.unsubscribe_role(&role_id);           // Remove event subscriptions
}
```

### RoleWorkerRegistry

```rust
pub struct RoleWorkerRegistry {
    workers: RwLock<HashMap<String, RoleWorker>>,
    store: Arc<Store>,
    workflow_manager: Arc<dyn WorkflowManager>,
    event_dispatcher: Arc<EventDispatcher>,
}
```

- `start_role(id, name)` -- stops existing worker first (clean re-registration), then starts new
- `stop_role(id)` -- stops and removes worker
- `stop_all()` -- stops all workers (shutdown)

---

## 15. Workflow Binding Processing

**File:** `crates/server/src/handlers/roles.rs:594-713`

### process_role_bindings()

Called during role creation and reload. For each binding in `RoleConfig.workflows`:

1. Determine `trigger_type` and `trigger_config` from `RoleTrigger` enum
2. Serialize `inputs` and `activities` to JSON
3. Upsert to `role_workflows` table
4. After all bindings: `register_role_triggers()` for schedule triggers
5. Build and register `EventSubscription`s for event triggers

### Dual Storage

Workflow bindings are stored in two places:
1. **`roles.frontmatter`** -- the full role.json as a JSON string (source of truth for the definition)
2. **`role_workflows` table** -- tracking rows for trigger registration, is_active state, last_fired

Both are updated on create/update/delete. The `frontmatter` is also written to
the filesystem as `role.json` via `write_role_json_to_fs()`.

### CRUD Handlers

**create_role_workflow:** Checks for conflict (binding exists), builds trigger
JSON, inserts into frontmatter, upserts tracking row, registers triggers,
writes to filesystem.

**update_role_workflow:** Merges provided fields over existing binding, handles
trigger type changes (unregister old, register new), updates both frontmatter
and tracking row.

**delete_role_workflow:** Removes from frontmatter, deletes tracking row,
unregisters triggers (cron + event), writes to filesystem.

---

## 16. Toggle / Enable / Disable

### Role Toggle (`toggle_role`)

```rust
store.toggle_role(&id);  // Flip is_enabled
if role.is_enabled != 0 {
    state.role_workers.start_role(&id, &role.name).await;
} else {
    state.role_workers.stop_role(&id).await;
}
```

### Workflow Binding Toggle (`toggle_role_workflow`)

```rust
let is_active = store.toggle_role_workflow(&id, &binding_name);
if is_active {
    register_binding_triggers(&id, &binding_name, ...).await;
} else {
    workflow::triggers::unregister_single_role_trigger(&id, &binding_name, &store);
    state.event_dispatcher.unsubscribe_binding(&id, &binding_name).await;
}
```

**Toggle -> OFF:**
- Schedule: deletes cron_job named `"role-{id}-{binding}"`
- Event: removes subscription from EventDispatcher
- Heartbeat: **NOT directly stopped** -- the RoleWorker's heartbeat tokio task
  is not individually cancellable. It continues running until the entire
  RoleWorker is stopped.

**Toggle -> ON:**
- Re-registers triggers (cron job and/or event subscription)

---

## 17. End-to-End Flows

### User Creates Schedule Automation

1. **Frontend:** Automate tab -> "New Automation"
2. AutomationEditor: name "Morning Briefing", steps, schedule 7:30 AM weekdays
3. **API:** `POST /roles/{id}/workflows` with bindingName, triggerType, triggerConfig, activities
4. **Handler:** Builds trigger JSON, inserts into frontmatter, upserts role_workflows row
5. **Trigger registration:** Creates cron_job named `"role-{role_id}-morning-briefing"`
6. **Scheduler tick (60s):** Finds cron job due -> `execute_role_workflow_task()`
7. Parses `"role:{role_id}:morning-briefing"` -> loads role config -> `run_inline()`
8. **Engine:** Creates workflow_runs record -> executes activities sequentially
9. If emit configured: last activity calls `emit` tool -> EventBus -> EventDispatcher

### Event Chain: Workflow A -> Workflow B

1. Workflow A completes, last activity calls `emit_tool` with source `"triage.done"`
2. **EventBus:** Event published to unbounded channel
3. **EventDispatcher:** Matches `"triage.done"` against subscription pattern `"triage.*"`
4. Found: Workflow B subscribed with `pattern: "triage.*"`
5. Injects `_event_source`, `_event_payload`, `_event_origin` into inputs
6. Calls `manager.run_inline()` for Workflow B
7. Workflow B executes with event data as input context

### User Toggles Automation OFF

1. **Frontend:** Toggle switch on automation card
2. **API:** `POST /roles/{id}/workflows/{binding}/toggle`
3. **DB:** `is_active` flipped to 0
4. **Handler:** Deletes cron_job, unsubscribes events
5. **Next scheduler tick:** Job no longer in `list_enabled_cron_jobs()` (cron_job deleted)
6. **EventDispatcher:** Subscription removed, events no longer matched

### User Cancels Running Workflow

1. **Frontend:** Cancel button on running workflow
2. **API:** `POST /workflows/{id}/runs/{runId}/cancel`
3. **WorkflowManager:** Looks up CancellationToken -> `token.cancel()`
4. **Engine:** Next activity check sees `token.is_cancelled()` -> returns `WorkflowError::Cancelled`
5. Run status updated to `"cancelled"` in DB
6. WebSocket broadcast: `"workflow_run_cancelled"`

---

## 18. Error Handling & Recovery

### Scheduler Errors

| Error | Handling |
|-------|----------|
| Invalid cron expression | Logged, job skipped |
| DB errors on history | Best-effort, non-critical |
| Agent run failure | Captured in (success, output, error) tuple |
| Subprocess failure | Exit code + stderr captured |

### Workflow Engine Errors

| Error Type | Fallback | Behavior |
|-----------|----------|----------|
| Provider error | Immediate | Return error, mark run failed |
| Token budget exceeded | Per-activity or global | Mark run failed |
| Tool not found | Continue | `ToolResult::error()`, activity continues |
| 3x consecutive tool-not-found | Abort | Early termination |
| Cancellation | Before each activity | Return `WorkflowError::Cancelled` |
| Exit tool | Propagate | Mark run "exited" (distinct from "failed") |
| Retry exhausted | `on_error.fallback` | Skip / Abort / NotifyOwner |
| Circuit breaker (3 same errors) | Abort | Mark run failed |

### Fallback Strategies

```rust
pub enum Fallback {
    Skip,          // Continue to next activity
    Abort,         // Fail entire workflow
    NotifyOwner,   // Same as Abort (notification not yet implemented)
}
```

### Orphan Cleanup

On server restart, `cleanup_orphaned_runs()` marks all "running" workflow_runs
as "cancelled" with error "server restart".

---

## 19. Known Issues

### BUG: Disabled Schedule Automations Still Execute

**Severity:** High

When a user toggles a role_workflow binding to `is_active = 0`:
- The toggle handler **correctly** deletes the cron_job (unregisters the schedule trigger)
- However, if the cron_job somehow still exists (e.g., re-registered on role worker restart),
  `execute_role_workflow_task()` in `scheduler.rs` **does NOT check `role_workflows.is_active`**

The execution path in `scheduler.rs:209-253`:
```rust
async fn execute_role_workflow_task(manager, store, command) {
    let parts = command.splitn(3, ':');
    let role = store.get_role(role_id)?;
    let config = napp::role::parse_role_config(&role.frontmatter)?;
    let binding = config.workflows.get(binding_name)?;
    // ** NO CHECK: does not query role_workflows.is_active **
    manager.run_inline(def_json, inputs, "schedule", role_id, ...).await
}
```

The role config comes from `roles.frontmatter` (which is the static role.json
definition) -- it has no concept of `is_active`. The `is_active` flag only
exists in the `role_workflows` tracking table.

**Event triggers DO check correctly** via `list_active_event_triggers()` which
has `WHERE rw.is_active = 1 AND r.is_enabled = 1`.

**Heartbeat triggers** have a partial mitigation: they run in the RoleWorker's
tokio task, which is started on role activation and stopped on deactivation.
However, individual heartbeat bindings cannot be toggled without restarting the
entire worker.

**Fix:** Add an `is_active` check in `execute_role_workflow_task()`:
```rust
// After loading role and config, check tracking table:
let bindings = store.list_role_workflows(role_id)?;
let is_active = bindings.iter()
    .find(|b| b.binding_name == binding_name)
    .map(|b| b.is_active != 0)
    .unwrap_or(false);
if !is_active { return (true, "skipped: binding inactive".into(), None); }
```

### Heartbeat Toggle Granularity

Individual heartbeat bindings cannot be toggled without stopping the entire
RoleWorker. The toggle handler deletes the cron_job (which is irrelevant for
heartbeats) and unsubscribes events (also irrelevant). The tokio::interval task
spawned by the worker continues until the worker is stopped.

**Impact:** Toggling a heartbeat binding to OFF has no effect until the role
itself is deactivated and reactivated.

### Dual Storage Drift

Workflow binding definitions exist in two places:
1. `roles.frontmatter` (JSON string of role.json)
2. `role_workflows` table rows

The CRUD handlers update both, but there is no reconciliation mechanism. If one
is modified outside the handler (e.g., direct DB edit, filesystem edit without
reload), they can drift out of sync. The `reload_role` endpoint addresses
filesystem drift but not direct DB edits.

### Event Channel Unbounded

`EventBus` uses `mpsc::unbounded_channel()` -- no backpressure. In a scenario
where workflow chains produce events faster than they are consumed, memory could
grow unboundedly.

### Scheduler Single-Tick Grid

All cron jobs share a 60s tick. Jobs due within the same minute all fire in one
batch. No per-job scheduling granularity below 1 minute.

### RoleWorker Heartbeat Overlaps with Server Heartbeat

Two independent heartbeat mechanisms exist:
1. **`heartbeat.rs`** (server-level) -- entity_config-driven, uses `run_chat()` on HEARTBEAT lane
2. **`role_worker.rs`** (role-level) -- from role_workflows with trigger_type "heartbeat", uses `run_inline()`

These are different systems serving different purposes. The server heartbeat is
a general "wake up and check" mechanism. The role worker heartbeat runs specific
inline workflow activities. They can overlap if both are configured for the same role.

### NotifyOwner Fallback is Stubbed

`Fallback::NotifyOwner` behaves identically to `Fallback::Abort` -- the
notification mechanism is not implemented. Both abort the workflow on activity failure.

---

## 20. Key Files Reference

### Core Role System

| File | Lines | Description |
|------|-------|-------------|
| `crates/server/src/handlers/roles.rs` | ~1754 | All role HTTP handlers + workflow binding CRUD |
| `crates/napp/src/role.rs` | ~665 | RoleConfig, WorkflowBinding, RoleTrigger, parse_role_config() |
| `crates/napp/src/role_loader.rs` | ~141 | Filesystem scanner (LoadedRole, scan_installed/user_roles) |
| `crates/db/src/queries/roles.rs` | ~353 | All role + role_workflow DB queries |
| `crates/db/src/models.rs:570-647` | ~78 | Role, RoleWorkflow, EmitSource, RoleWorkflowStats structs |
| `crates/tools/src/role_tool.rs:14-29` | ~16 | ActiveRole struct, RoleRegistry type alias |

### Workflow Engine

| File | Lines | Description |
|------|-------|-------------|
| `crates/workflow/src/lib.rs` | ~52 | WorkflowError enum, module exports |
| `crates/workflow/src/parser.rs` | ~226 | WorkflowDef, Activity, parse_workflow(), validation |
| `crates/workflow/src/engine.rs` | ~603 | execute_workflow(), execute_activity(), build_activity_prompt() |
| `crates/workflow/src/loader.rs` | ~110 | Filesystem loader (LoadedWorkflow, scan directories) |
| `crates/workflow/src/triggers.rs` | ~125 | register/unregister schedule/role triggers |
| `crates/workflow/src/events.rs` | ~174 | EventDispatcher, EventSubscription, source_matches() |
| `crates/server/src/workflow_manager.rs` | ~400+ | WorkflowManagerImpl (trait implementation) |
| `crates/tools/src/workflows/manager.rs` | ~93 | WorkflowManager trait definition |

### Scheduling & Runtime

| File | Lines | Description |
|------|-------|-------------|
| `crates/server/src/scheduler.rs` | ~254 | Cron scheduler (tick loop, task dispatch, role_workflow execution) |
| `crates/agent/src/role_worker.rs` | ~415 | RoleWorker, RoleWorkerRegistry, heartbeat/event trigger tasks |
| `crates/db/src/queries/cron_jobs.rs` | ~296 | All cron_job + cron_history DB queries |
| `crates/db/src/queries/workflows.rs` | ~508 | Workflow, run, activity_result queries + stats |

### Database Migrations

| Migration | Description |
|-----------|-------------|
| `0050_workflows.sql` | Initial: workflows, workflow_runs, workflow_activity_results, roles |
| `0051_workflows_to_fs.sql` | Add napp_path to workflows + roles |
| `0053_role_workflows.sql` | Create role_workflows table (binding_name, trigger, is_active) |
| `0056_role_workflow_last_fired.sql` | Add last_fired column |
| `0058_role_instances.sql` | Rename code->kind (not UNIQUE), add workflow_ref/workflow_id |
| `0059_role_workflow_emit.sql` | Add emit column |
| `0060_role_workflow_activities.sql` | Add activities column |
| `0061_workflow_runs_drop_fk.sql` | Drop FK on workflow_runs.workflow_id (support inline runs) |
| `0062_workflow_run_output.sql` | Add output column to workflow_runs |
| `0064_role_input_values.sql` | Add input_values column to roles |
| `0015_cron_jobs.sql` | Initial: cron_jobs + cron_history |
| `0042_cron_skill.sql` | Add instructions column to cron_jobs |
| `0057_entity_config.sql` | Create entity_config table |

---

*Last updated: 2026-03-26*
