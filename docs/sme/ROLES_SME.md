# Role System — Rust SME Reference

> Definitive reference for the Nebo Rust role system. Covers the role definition
> format (ROLE.md + role.json), trigger types, inline activities, dependency cascade,
> database schema, HTTP endpoints, filesystem storage, event subscriptions, agent
> persona injection, the RoleWorker runtime, and the full installation-to-execution lifecycle.

**Canonical spec:** [platform-taxonomy.md](platform-taxonomy.md)

---

## Table of Contents

1. [System Overview](#1-system-overview)
2. [Role Definition Format](#2-role-definition-format)
3. [Role Configuration (role.json)](#3-role-configuration-rolejson)
4. [Trigger Types](#4-trigger-types)
5. [Role Loader](#5-role-loader)
6. [Database Schema](#6-database-schema)
7. [Database Queries](#7-database-queries)
8. [HTTP Endpoints](#8-http-endpoints)
9. [Workflow Binding Processing](#9-workflow-binding-processing)
10. [Trigger Registration](#10-trigger-registration)
11. [Event Subscriptions](#11-event-subscriptions)
12. [Dependency Cascade](#12-dependency-cascade)
13. [Agent Persona Injection](#13-agent-persona-injection)
14. [Filesystem & Package Storage](#14-filesystem--package-storage)
15. [Code Installation (ROLE-XXXX-XXXX)](#15-code-installation-role-xxxx-xxxx)
16. [Migration](#16-migration)
17. [RoleWorker Runtime](#17-roleworker-runtime)
18. [Complete Lifecycle](#18-complete-lifecycle)
19. [Integration Points](#19-integration-points)
20. [Cross-Reference to Go Docs](#20-cross-reference-to-go-docs)
21. [Frontend — Role Agent Pages](#21-frontend--role-agent-pages)

---

## 1. System Overview

A **Role** is a job description with a schedule. Roles can define automation via **inline activities** (self-contained) or by referencing **external workflows**:

```
ROLE (schedule of intent)
  ├─ ROLE.md          — persona / job description (pure prose)
  ├─ role.json        — operational config (bindings + triggers)
  │
  ├─ BINDING 1 (inline activities)
  │  ├─ Activity A    — intent + skills + model + steps
  │  └─ Activity B
  │
  ├─ BINDING 2 (external workflow ref)
  │  └─ WORKFLOW      — procedure (what to do)
  │     └─ SKILL A    — domain knowledge
  │
  └─ SKILL X          — role-level skill dependency
```

> **Key principle:** The workflow does not decide when it runs. The Role does.

**Key properties:**
- Roles **own triggers** — cron schedules, heartbeat intervals, event subscriptions, manual
- Roles **define bindings** — each binding has a trigger, and either inline activities or an external workflow ref
- Roles **support inline activities** — activities defined directly in role.json, no separate workflow needed
- Roles **support emit chains** — a binding can emit a named event on completion, triggering other bindings
- Roles **declare skill dependencies** — skills required by the role itself (beyond those in activities)
- Roles **define the agent's persona** — ROLE.md replaces the default identity in the system prompt
- Installing a role **cascades downward** — auto-installs required workflows and skills
- Roles are **executed by RoleWorker** — a per-role runtime that spawns heartbeat loops and event subscriptions

---

## 2. Role Definition Format

**Source:** `crates/napp/src/role.rs`

A role consists of two files:

### ROLE.md — Agent Persona

Pure prose. The agent's job description, communication style, and behavioral guidelines. No frontmatter required (though legacy frontmatter is supported for backward compatibility).

```markdown
# Chief of Staff

You manage the executive's daily rhythm. Every morning at 7 AM, you prepare
a concise briefing covering calendar, emails, and market news.

## Communication Style
- Direct and efficient
- Lead with the most important item
- Flag anything requiring immediate decision

## Boundaries
- Never commit the executive to meetings without confirmation
- Always include source links for market data
```

### Parsing (parse_role)

```rust
pub struct RoleDef {
    pub id: String,           // Empty for pure-prose roles
    pub name: String,         // Empty for pure-prose roles
    pub description: String,  // Empty for pure-prose roles
    pub body: String,         // Full markdown content (skip serialization)
}

pub fn parse_role(content: &str) -> Result<RoleDef, NappError>
```

- **Pure prose** (no `---` at start): Returns `RoleDef` with empty identity fields, full content as `body`
- **Legacy frontmatter**: Extracts `id`, `name`, `description` from YAML between `---` delimiters, rest as `body`

### Handler-Level Parsing (parse_role_md)

The HTTP handler has its own frontmatter parser for the create/update flow:

```rust
struct RoleFrontmatter {
    name: String,
    description: String,
    skills: Vec<String>,       // Skill refs from frontmatter
    pricing: Option<RolePricing>,
}
```

This extracts dependency refs directly from ROLE.md frontmatter, which are merged with role.json refs during role creation.

---

## 3. Role Configuration (role.json)

**Source:** `crates/napp/src/role.rs`

### RoleConfig

```rust
pub struct RoleConfig {
    pub workflows: HashMap<String, WorkflowBinding>,
    pub skills: Vec<String>,
    pub pricing: Option<RolePricing>,
    pub defaults: Option<RoleDefaults>,
}
```

### WorkflowBinding

Bindings define **what** to run and **when**. Activities can be defined inline (no external workflow needed):

```rust
pub struct WorkflowBinding {
    pub trigger: RoleTrigger,
    pub description: String,
    pub inputs: HashMap<String, serde_json::Value>,
    pub activities: Vec<RoleActivity>,     // Inline activity definitions
    pub budget: RoleBudget,                // Token/cost budget for this binding
    pub emit: Option<String>,              // Event name to emit on completion
}
```

**Key methods:**
- `to_workflow_json(&self, name: &str) -> String` — Serializes binding into a workflow definition JSON string (used by scheduler and RoleWorker to execute inline activities)
- `has_activities(&self) -> bool` — Returns true if binding has inline activities defined

### RoleActivity

Defines a single step within a binding. Each activity is an agent task with its own model, skills, and instructions:

```rust
pub struct RoleActivity {
    pub id: String,                        // Unique within binding
    pub intent: String,                    // What this activity should accomplish
    pub skills: Vec<String>,               // Skill refs for context
    pub mcps: Vec<String>,                 // MCP server refs
    pub cmds: Vec<String>,                 // Shell commands to enable
    pub model: String,                     // AI model to use
    pub steps: Vec<String>,               // Step-by-step instructions
    pub token_budget: RoleTokenBudget,     // Per-activity token limit
    pub on_error: RoleOnError,             // Error handling policy
}
```

### RoleTokenBudget

```rust
pub struct RoleTokenBudget {
    pub max: u32,    // Default: 4096
}
```

### RoleOnError

```rust
pub struct RoleOnError {
    pub retry: u32,               // Default: 1
    pub fallback: RoleFallback,   // Default: NotifyOwner
}

pub enum RoleFallback {
    NotifyOwner,
    Skip,
    Abort,
}
```

### RoleBudget

```rust
pub struct RoleBudget {
    pub total_per_run: u32,
    pub cost_estimate: String,
}
```

### RolePricing

```rust
pub struct RolePricing {
    pub model: String,    // "monthly_fixed", "per_run", etc.
    pub cost: f64,        // Dollar amount
}
```

### RoleDefaults

```rust
pub struct RoleDefaults {
    pub timezone: String,             // "user_local", "America/New_York", etc.
    pub configurable: Vec<String>,    // JSON paths user can override
}
```

### Parsing & Validation

```rust
pub fn parse_role_config(json_str: &str) -> Result<RoleConfig, NappError>
pub fn validate_role_config(config: &RoleConfig) -> Result<(), NappError>
```

**Validation rules:**
- Skill refs must be qualified names (`@org/skills/name`) or install codes (`SKIL-XXXX-XXXX`)
- Event triggers must have at least one source
- Activity IDs must be unique within a binding
- Empty config `{}` is valid (no workflows or skills)

### Skill Reference Validation

```rust
fn is_qualified_skill_ref(s: &str) -> bool
```

Accepts two formats:
- **Qualified name:** `@org/skills/name` or `@org/skills/name@version`
- **Install code:** `SKIL-XXXX-XXXX`

### Example role.json

```json
{
  "workflows": {
    "morning-briefing": {
      "trigger": { "type": "schedule", "cron": "0 7 * * *" },
      "description": "Generate daily briefing at 7 AM",
      "inputs": { "department": "engineering" },
      "activities": [
        {
          "id": "gather",
          "intent": "Gather calendar and email data",
          "skills": ["@nebo/skills/email-reader@^1.0.0"],
          "mcps": [],
          "cmds": [],
          "model": "claude-sonnet-4",
          "steps": ["Check calendar for today", "Scan inbox for urgent items"],
          "token_budget": { "max": 4096 },
          "on_error": { "retry": 2, "fallback": "Skip" }
        },
        {
          "id": "compose",
          "intent": "Write the briefing summary",
          "skills": ["@nebo/skills/briefing-writer@^1.0.0"],
          "mcps": [],
          "cmds": [],
          "model": "claude-sonnet-4",
          "steps": ["Synthesize into concise briefing"],
          "token_budget": { "max": 4096 },
          "on_error": { "retry": 1, "fallback": "NotifyOwner" }
        }
      ],
      "budget": { "total_per_run": 8192, "cost_estimate": "$0.02" },
      "emit": "briefing.ready"
    },
    "day-monitor": {
      "trigger": { "type": "heartbeat", "interval": "30m", "window": "08:00-18:00" },
      "description": "Monitor for changes during business hours",
      "activities": [
        {
          "id": "check",
          "intent": "Check for notable changes",
          "skills": [],
          "mcps": [],
          "cmds": [],
          "model": "claude-haiku-4",
          "steps": ["Review recent activity"],
          "token_budget": { "max": 2048 },
          "on_error": { "retry": 1, "fallback": "Skip" }
        }
      ],
      "budget": { "total_per_run": 2048, "cost_estimate": "$0.005" }
    },
    "interrupt": {
      "trigger": { "type": "event", "sources": ["calendar.changed", "email.urgent"] },
      "description": "Handle urgent interrupts",
      "activities": [
        {
          "id": "triage",
          "intent": "Assess urgency and respond",
          "skills": [],
          "mcps": [],
          "cmds": [],
          "model": "claude-sonnet-4",
          "steps": ["Evaluate the event", "Notify if action needed"],
          "token_budget": { "max": 4096 },
          "on_error": { "retry": 1, "fallback": "NotifyOwner" }
        }
      ],
      "budget": { "total_per_run": 4096, "cost_estimate": "$0.01" }
    },
    "ad-hoc": {
      "trigger": { "type": "manual" },
      "description": "Run on demand",
      "activities": [],
      "budget": { "total_per_run": 8192, "cost_estimate": "$0.02" }
    }
  },
  "skills": ["@nebo/skills/briefing-writer@^1.0.0"],
  "pricing": { "model": "monthly_fixed", "cost": 47.0 },
  "defaults": {
    "timezone": "user_local",
    "configurable": ["workflows.morning-briefing.trigger.cron"]
  }
}
```

---

## 4. Trigger Types

**Source:** `crates/napp/src/role.rs`

```rust
#[serde(tag = "type")]
pub enum RoleTrigger {
    Schedule { cron: String },
    Heartbeat { interval: String, window: Option<String> },
    Event { sources: Vec<String> },
    Manual,
}
```

| Trigger | Fields | Description | Implementation |
|---|---|---|---|
| `schedule` | `cron` (string) | Fires on a cron schedule. Standard 5-field cron expression. | Cron job via `store.upsert_cron_job()`, task_type `"role_workflow"` |
| `heartbeat` | `interval` (string), `window?` (string) | Recurring interval within optional time window. | RoleWorker spawns async loop per binding |
| `event` | `sources` (string array) | Fires when matching event is emitted. Pattern supports wildcards. | RoleWorker subscribes via `EventDispatcher` |
| `manual` | — | Explicit user trigger only. | Stored in DB, triggered via chat or API |

### Trigger Config Serialization

When stored in `role_workflows.trigger_config`:
- **Schedule:** cron expression string (e.g., `"0 7 * * *"`)
- **Heartbeat:** `"{interval}"` or `"{interval}|{window}"` (e.g., `"30m|08:00-18:00"`)
- **Event:** comma-separated sources (e.g., `"calendar.changed,email.urgent"`)
- **Manual:** empty string

### Duration Parsing (Heartbeat)

**Source:** `crates/agent/src/role_worker.rs`

Heartbeat intervals are parsed by `parse_duration()`:
- `"30m"` → 30 minutes
- `"1h"` → 1 hour
- `"5s"` → 5 seconds
- `"2h30m"` → 2 hours 30 minutes
- Bare number → treated as minutes

Time windows parsed by `parse_time_window()`:
- `"08:00-18:00"` → NaiveTime range
- Handles wrap-around (e.g., `"22:00-06:00"`)

---

## 5. Role Loader

**Source:** `crates/napp/src/role_loader.rs`

### LoadedRole

```rust
pub struct LoadedRole {
    pub role_def: RoleDef,
    pub config: Option<RoleConfig>,
    pub source: RoleSource,
    pub napp_path: Option<PathBuf>,
    pub source_path: PathBuf,
    pub version: Option<String>,       // Extracted from manifest.json if present
}

pub enum RoleSource {
    Installed,    // nebo/roles/ (sealed .napp)
    User,         // user/roles/ (loose files)
}
```

### Loading Functions

```rust
/// Load a role from a directory containing ROLE.md (+ optional role.json).
pub fn load_from_dir(dir: &Path, source: RoleSource) -> Result<LoadedRole, NappError>
```

1. Read `ROLE.md` from directory → `parse_role()` → `RoleDef`
2. If `role.json` exists → `parse_role_config()` → `RoleConfig`
3. If `manifest.json` exists → extract `version` field
4. Return `LoadedRole` with all data

```rust
/// Scan installed (nebo/) roles directory for extracted role directories.
pub fn scan_installed_roles(dir: &Path) -> Vec<LoadedRole>
```

Uses `reader::walk_for_marker(dir, "ROLE.md", ...)` to find all directories containing `ROLE.md`. Stops recursion when marker found.

```rust
/// Scan user roles directory for loose role directories.
pub fn scan_user_roles(dir: &Path) -> Vec<LoadedRole>
```

Reads immediate subdirectories of `dir`, checks for `ROLE.md` in each.

---

## 6. Database Schema

### roles (post-migration 0058)

```sql
CREATE TABLE IF NOT EXISTS roles (
    id TEXT PRIMARY KEY,
    kind TEXT,                            -- Marketplace kind (not unique, allows multiple instances)
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    role_md TEXT NOT NULL,                -- Full ROLE.md content
    frontmatter TEXT NOT NULL,            -- Metadata JSON (workflows, skills, pricing)
    pricing_model TEXT,
    pricing_cost REAL,
    is_enabled INTEGER NOT NULL DEFAULT 1,
    installed_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    napp_path TEXT                        -- Path to directory or .napp (migration 0051)
);
CREATE INDEX idx_roles_kind ON roles(kind);
```

> **Migration 0058:** Renamed `code` → `kind`, removed UNIQUE constraint to allow multiple role instances sharing the same marketplace kind.

### role_workflows (post-migrations 0053, 0058, 0059, 0060)

```sql
CREATE TABLE IF NOT EXISTS role_workflows (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    role_id TEXT NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    binding_name TEXT NOT NULL,            -- Key from role.json workflows map
    workflow_ref TEXT NOT NULL DEFAULT '', -- @org/workflows/name (legacy, optional)
    workflow_id TEXT,                      -- Resolved local workflow ID (legacy, optional)
    trigger_type TEXT NOT NULL,            -- schedule, event, heartbeat, manual
    trigger_config TEXT NOT NULL,          -- Cron, interval|window, sources CSV, or empty
    description TEXT,
    inputs TEXT,                          -- Default inputs JSON
    is_active INTEGER NOT NULL DEFAULT 1,
    last_fired TEXT,                      -- ISO timestamp of last execution
    emit TEXT,                            -- Event name to emit on completion (migration 0059)
    activities TEXT,                      -- Serialized activities JSON (migration 0060)
    UNIQUE(role_id, binding_name)
);
CREATE INDEX idx_role_workflows_role ON role_workflows(role_id);
```

**Cascade behavior:** Deleting a role automatically deletes all its `role_workflows` entries.

### workflow_runs (post-migration 0061, 0062)

```sql
CREATE TABLE IF NOT EXISTS workflow_runs (
    id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL,             -- workflow ID or "role:{role_id}" for inline
    trigger_type TEXT NOT NULL,
    trigger_detail TEXT,
    status TEXT NOT NULL DEFAULT 'running',
    inputs TEXT,
    current_activity TEXT,
    total_tokens_used INTEGER DEFAULT 0,
    error TEXT,
    error_activity TEXT,
    session_key TEXT,
    output TEXT,                           -- Workflow completion output (migration 0062)
    started_at INTEGER NOT NULL DEFAULT (unixepoch()),
    completed_at INTEGER
);
```

> **Migration 0061:** Dropped FK constraint on `workflow_id` to allow inline workflow runs using `"role:{role_id}"` format.

### Rust Models

```rust
pub struct Role {
    pub id: String,
    pub kind: Option<String>,            // Was `code`, renamed in migration 0058
    pub name: String,
    pub description: String,
    pub role_md: String,
    pub frontmatter: String,
    pub pricing_model: Option<String>,
    pub pricing_cost: Option<f64>,
    pub is_enabled: i64,
    pub installed_at: i64,
    pub updated_at: i64,
    pub napp_path: Option<String>,
}

pub struct RoleWorkflow {
    pub id: i64,
    pub role_id: String,
    pub binding_name: String,
    pub trigger_type: String,
    pub trigger_config: String,
    pub description: Option<String>,
    pub inputs: Option<String>,
    pub is_active: i64,                  // Serialized as bool via i64_as_bool
    pub emit: Option<String>,            // Event to emit on completion
    pub activities: Option<serde_json::Value>,  // Inline activities JSON (skip_deserializing)
}

pub struct EmitSource {
    pub emit: String,
    pub role_id: String,
    pub role_name: String,
    pub binding_name: String,
}
```

> **Note:** `RoleWorkflow` model does not expose `workflow_ref`, `workflow_id`, or `last_fired` fields to the application layer. The `activities` field uses `skip_deserializing` — it is only populated via explicit DB query.

---

## 7. Database Queries

**Source:** `crates/db/src/queries/roles.rs`

```rust
// CRUD
pub fn list_roles(&self, limit: i64, offset: i64) -> Result<Vec<Role>>
pub fn count_roles(&self) -> Result<i64>
pub fn get_role(&self, id: &str) -> Result<Option<Role>>
pub fn create_role(&self, id, kind, name, description, role_md, frontmatter,
                   pricing_model, pricing_cost) -> Result<Role>
pub fn update_role(&self, id, name, description, role_md, frontmatter,
                   pricing_model, pricing_cost) -> Result<()>
pub fn delete_role(&self, id: &str) -> Result<()>
pub fn set_role_napp_path(&self, id: &str, napp_path: &str) -> Result<()>
pub fn toggle_role(&self, id: &str) -> Result<()>

// Role-Workflow Bindings
pub fn upsert_role_workflow(&self, role_id, binding_name, trigger_type,
                            trigger_config, description, inputs,
                            emit, activities) -> Result<()>
pub fn list_role_workflows(&self, role_id: &str) -> Result<Vec<RoleWorkflow>>
pub fn delete_role_workflows(&self, role_id: &str) -> Result<()>
pub fn delete_single_role_workflow(&self, role_id: &str, binding_name: &str) -> Result<()>
pub fn toggle_role_workflow(&self, role_id: &str, binding_name: &str) -> Result<bool>
pub fn list_active_event_triggers(&self) -> Result<Vec<RoleWorkflow>>
pub fn update_role_workflow_last_fired(&self, role_id, binding_name, fired_at) -> Result<()>
pub fn list_emit_sources(&self) -> Result<Vec<EmitSource>>
pub fn delete_cron_jobs_by_prefix(&self, prefix: &str) -> Result<i64>
```

---

## 8. HTTP Endpoints

**Source:** `crates/server/src/handlers/roles.rs`

### Core CRUD

| Method | Path | Handler | Purpose |
|---|---|---|---|
| GET | `/roles` | `list_roles` | List roles (paginated, max 100) |
| POST | `/roles` | `create_role` | Create role + process bindings + cascade deps |
| GET | `/roles/{id}` | `get_role` | Get single role |
| PUT | `/roles/{id}` | `update_role` | Update ROLE.md + metadata + reprocess bindings |
| DELETE | `/roles/{id}` | `delete_role` | Delete + unregister all triggers |
| POST | `/roles/{id}/toggle` | `toggle_role` | Enable/disable |
| POST | `/roles/{id}/install-deps` | `install_deps` | Force-install all missing dependencies |

### Lifecycle

| Method | Path | Handler | Purpose |
|---|---|---|---|
| POST | `/roles/{id}/activate` | `activate_role` | Start RoleWorker, register agent in Loop |
| POST | `/roles/{id}/deactivate` | `deactivate_role` | Stop RoleWorker, deregister agent from Loop |
| POST | `/roles/{id}/duplicate` | `duplicate_role` | Clone role with new ID and name |
| POST | `/roles/{id}/chat` | `chat_with_role` | Send a message to role's agent chat |

### Workflow Binding CRUD

| Method | Path | Handler | Purpose |
|---|---|---|---|
| GET | `/roles/{id}/workflows` | `list_role_workflows` | List all bindings for a role |
| POST | `/roles/{id}/workflows` | `create_role_workflow` | Create a new workflow binding |
| PUT | `/roles/{id}/workflows/{binding_name}` | `update_role_workflow` | Update an existing binding |
| POST | `/roles/{id}/workflows/{binding_name}` | `toggle_role_workflow` | Toggle binding active/inactive |
| DELETE | `/roles/{id}/workflows/{binding_name}` | `delete_role_workflow` | Delete a single binding |

### Discovery

| Method | Path | Handler | Purpose |
|---|---|---|---|
| GET | `/roles/event-sources` | `list_event_sources` | List all emit sources across roles |
| GET | `/roles/active` | `list_active_roles` | List enabled roles with workflow bindings |

### POST /roles (Create)

**Request body:**
```json
{
  "roleMd": "---\nname: Chief of Staff\ndescription: ...\n---\n# Content",
  "roleJson": "{...role.json content...}",
  "name": "Chief of Staff",
  "description": "Manages executive rhythm"
}
```

**Handler flow:**
1. Parse ROLE.md frontmatter → extract name, description, skill refs
2. If `roleJson` provided, parse RoleConfig and merge refs into frontmatter
3. Create Role record in DB (with `kind` if provided)
4. Write `ROLE.md` and `role.json` to `user/roles/{name}/`
5. Set `napp_path` to the directory
6. If `roleJson` provided: call `process_role_bindings()` (see [section 9](#9-workflow-binding-processing))
7. Broadcast `"role_installed"` event
8. Collect all dependency refs (from frontmatter + roleJson)
9. Run `resolve_cascade()` to auto-install missing deps

**Response:**
```json
{
  "role": { "id": "...", "name": "...", ... },
  "installReport": [
    { "binding": "morning-briefing", "triggerType": "schedule", "status": "ok" },
    { "binding": "interrupt", "triggerType": "event", "status": "ok" }
  ],
  "cascade": {
    "results": [...],
    "installed_count": 2,
    "pending_count": 0,
    "failed_count": 0
  }
}
```

### POST /roles/{id}/activate

**Handler flow:**
1. Load role from DB
2. Start RoleWorker via `role_workers.start_role()`
3. Register agent in NeboLoop via `register_agent_in_loop()`
4. Return success

### POST /roles/{id}/deactivate

**Handler flow:**
1. Stop RoleWorker via `role_workers.stop_role()`
2. Deregister agent from NeboLoop via `deregister_agent_from_loop()`
3. Return success

### POST /roles/{id}/workflows (Create Binding)

**Handler flow:**
1. Parse request body: trigger, description, inputs, activities, emit
2. Flatten trigger config for DB storage
3. Upsert to `role_workflows` table
4. Register triggers for new binding via `register_binding_triggers()`
5. Write updated role.json to filesystem via `write_role_json_to_fs()`
6. Return updated binding

### DELETE /roles/{id}

**Handler flow:**
1. Unregister all triggers: `workflow::triggers::unregister_role_triggers(&id, &store)`
   - Deletes cron jobs with prefix `role-{id}-`
2. Delete role from DB (cascade deletes `role_workflows`)
3. Broadcast `"role_uninstalled"` event

### Helper Functions

- **`build_trigger_json()`** — Convert flat (type, config) to JSON trigger for frontmatter
- **`flatten_trigger_config()`** — Convert JSON trigger to flat string for DB storage
- **`write_role_json_to_fs()`** — Write updated role.json to filesystem if `napp_path` exists
- **`register_binding_triggers()`** — Register schedule cron + event subscriptions for a single binding
- **`create_blank_role()`** — Create minimal agent and auto-activate

---

## 9. Workflow Binding Processing

**Source:** `crates/server/src/handlers/roles.rs` — `process_role_bindings()`

When a role with `roleJson` is created or updated, this function processes each workflow binding:

### Algorithm

```
FOR EACH (binding_name, binding) in role_config.workflows:

1. Serialize trigger:
   - Schedule → ("schedule", cron_string)
   - Heartbeat → ("heartbeat", "{interval}" or "{interval}|{window}")
   - Event → ("event", "source1,source2,...")
   - Manual → ("manual", "")

2. Serialize activities (if any) to JSON string

3. Upsert to role_workflows table:
   - role_id, binding_name, trigger_type, trigger_config,
     description, inputs, emit, activities

4. Report status:
   - "ok" if upsert succeeded
   - "error" if DB upsert failed

AFTER all bindings processed:

5. Register schedule triggers (cron jobs with task_type "role_workflow")
6. Register event subscriptions (via EventDispatcher)
```

### Report Format

Each binding produces a report entry:

```json
{
  "binding": "morning-briefing",
  "triggerType": "schedule",
  "status": "ok"
}
```

Status values: `"ok"` (upsert succeeded), `"error"` (DB failure).

---

## 10. Trigger Registration

**Source:** `crates/workflow/src/triggers.rs`

### Schedule Triggers (Cron)

```rust
pub fn register_role_triggers(
    role_id: &str,
    bindings: &[db::models::RoleWorkflow],
    store: &Store,
)
```

For each binding with `trigger_type == "schedule"`:
- Creates a cron job named `role-{role_id}-{binding_name}`
- Schedule: the cron expression from `trigger_config`
- Command: `role:{role_id}:{binding_name}` (for scheduler to resolve at execution time)
- Task type: `"role_workflow"`
- Enabled: `true`

When the scheduler fires a cron job with `task_type = "role_workflow"`:
- Parses command `role:{role_id}:{binding_name}`
- Loads role from DB, parses role config
- Gets binding, converts to workflow JSON via `binding.to_workflow_json()`
- Calls `workflow_manager.run_inline(def_json, inputs, "schedule", role_id, emit_source)`

### Single Binding Registration

```rust
pub fn unregister_single_role_trigger(role_id: &str, binding_name: &str, store: &Store)
```

Deletes a single cron job named `role-{role_id}-{binding_name}`. Used when deleting or updating individual bindings.

### Unregistration

```rust
pub fn unregister_role_triggers(role_id: &str, store: &Store)
```

Deletes all cron jobs whose name starts with `role-{role_id}-`. Called on role deletion.

### Standalone Workflow Triggers

```rust
pub fn register_schedule_trigger(workflow_id: &str, cron: &str, store: &Store)
pub fn unregister_triggers(workflow_id: &str, store: &Store)
```

For standalone workflows (not role-bound), creates cron jobs with name `workflow-{id}` and task_type `"workflow"`.

---

## 11. Event Subscriptions

**Source:** `crates/workflow/src/events.rs`, `crates/server/src/handlers/roles.rs`

### EventSubscription

```rust
pub struct EventSubscription {
    pub pattern: String,                    // Event source pattern (exact or wildcard)
    pub default_inputs: serde_json::Value,  // Default inputs merged with event data
    pub role_source: String,                // Role ID that owns this subscription
    pub binding_name: String,               // Binding key from role.json
    pub definition_json: Option<String>,    // Inline workflow JSON from binding.to_workflow_json()
    pub emit_source: Option<String>,        // Namespaced emit source for last activity
}
```

### EventDispatcher

```rust
pub struct EventDispatcher {
    subscriptions: Arc<RwLock<Vec<EventSubscription>>>,
}
```

**Methods:**
- `new()` — Create empty dispatcher
- `subscribe(sub)` — Add a single subscription
- `set_subscriptions(subs)` — Replace all subscriptions
- `unsubscribe_binding(role_id, binding_name)` — Remove subscriptions for a single binding
- `clear()` — Remove all subscriptions
- `match_event(event)` — Find subscriptions matching an event source
- `spawn(rx, manager)` — Spawn the dispatch loop (reads events, matches, triggers runs)

### Registration

During RoleWorker startup or `process_role_bindings()`, for each binding with `trigger_type == "event"`:

1. Split `trigger_config` by comma to get individual event source patterns
2. Build inline workflow JSON via `binding.to_workflow_json()` if `has_activities()`
3. Construct emit_source: `"{role-slug}.{emit-name}"` (if `emit` is set)
4. For each source pattern, create and subscribe:

```rust
EventSubscription {
    pattern: "email.urgent",
    default_inputs: { "priority": "high" },
    role_source: "role-xyz",
    binding_name: "interrupt",
    definition_json: Some("{...workflow JSON...}"),
    emit_source: Some("chief-of-staff.interrupt-done"),
}
```

### Pattern Matching

When an event is emitted:
- **Exact match:** `"email.urgent"` matches `"email.urgent"` only
- **Wildcard:** `"email.*"` matches `"email.urgent"`, `"email.info"`, etc.

### Event-Triggered Execution

When EventDispatcher matches an event to a subscription:
1. Merge event data into default_inputs:
   ```json
   {
     "_event_source": "email.urgent",
     "_event_payload": { ... },
     "_event_origin": "workflow:email-triage:run-550e",
     "priority": "high"
   }
   ```
2. Call `manager.run_inline(definition_json, merged_inputs, "event", role_source, emit_source)`
3. Inline workflow executes in background

---

## 12. Dependency Cascade

**Source:** `crates/server/src/deps.rs`

### Dependency Extraction

```rust
pub fn extract_role_deps(config: &RoleConfig) -> Vec<DepRef>
```

Extracts deps from role.json:
- Each skill in `skills[]` → `DepType::Skill`
- Each skill in inline `activities[].skills[]` → `DepType::Skill`

```rust
pub fn extract_role_deps_from_frontmatter(frontmatter_json: &str) -> Vec<DepRef>
```

Extracts deps from the stored frontmatter JSON. Attempts to parse as `RoleConfig` first, falls back to raw JSON extraction:
- `skills[]` array → `DepType::Skill`

### Cascade Resolution

When a role is created:

```
Role created with:
  - skills: ["@nebo/skills/briefing-writer@^1.0.0"]
  - activity skills: ["@nebo/skills/email-reader@^1.0.0"]

resolve_cascade():
  ├─ Check skill "@nebo/skills/briefing-writer@^1.0.0"
  │  ├─ Already installed? → AlreadyInstalled
  │  └─ Not installed? →
  │     ├─ Autonomous mode → auto-install from NeboLoop
  │     └─ Non-autonomous → mark PendingApproval
  │
  ├─ Check skill "@nebo/skills/email-reader@^1.0.0"
  │  └─ Same logic as above
  │
  └─ Recurse into child deps of newly installed artifacts
```

### Autonomy Modes

- **Autonomous** (`settings.autonomous_mode = 1`): Auto-install missing deps via NeboLoop API
- **Non-autonomous**: Mark deps as `PendingApproval`, broadcast `"dep_pending"` event, user must explicitly approve

### Force Install

`POST /roles/{id}/install-deps` calls `resolve_cascade_force()` which installs regardless of autonomy mode.

---

## 13. Agent Persona Injection

**Source:** `crates/agent/src/prompt.rs`

### PromptContext

```rust
pub struct PromptContext {
    pub agent_name: String,
    pub active_skill: Option<String>,       // Active skill content
    pub skill_hints: Vec<String>,           // Hints about available skills
    pub model_aliases: String,              // Model switching options
    pub channel: String,                    // Channel name
    pub platform: String,                   // Platform (e.g., "macos")
    pub memory_context: String,             // Formatted memory facts
    pub db_context: Option<String>,         // Rich DB context (replaces memory_context when set)
    pub active_role: Option<String>,        // ROLE.md body — replaces default identity
}
```

### How Roles Affect the Agent

The role's ROLE.md content **directly replaces** the default bot identity in the system prompt:

```rust
// In build_static():
if let Some(role_md) = &pctx.active_role {
    if !role_md.is_empty() {
        parts.push(role_md.clone());      // Role markdown IS the identity
    }
} else {
    parts.push(SECTION_IDENTITY.into());  // Default identity fallback
}
```

When a role is active:
1. ROLE.md body is stored in `PromptContext.active_role`
2. The role markdown **replaces** `SECTION_IDENTITY` in the system prompt
3. The agent fully assumes the role's persona, communication style, and behavioral guidelines
4. All other prompt sections (capabilities, tools, behavior, media, etc.) remain unchanged

### DynamicContext

Per-iteration context appended after the static prompt:

```rust
pub struct DynamicContext {
    pub provider_name: String,   // Provider name
    pub model_name: String,      // Model name
    pub active_task: String,     // Background objective
    pub summary: String,         // Conversation summary
}
```

### Separation of Concerns

- **ROLE.md** → Agent's conversational persona (replaces default identity in system prompt)
- **Activity skills** → Pure knowledge for workflow activities (loaded per-activity, no persona bleed)
- **role.json** → Operational scheduling (never in prompts, only drives automation via RoleWorker)

---

## 14. Filesystem & Package Storage

### Directory Structure

```
~/.nebo/
├── nebo/                            # Marketplace (sealed .napp archives)
│   └── roles/
│       └── @org/roles/name/
│           └── 1.0.0.napp           # Sealed tar.gz
│
├── user/                            # User-created (loose files)
│   └── roles/
│       └── my-role/
│           ├── ROLE.md              # Persona (pure prose or frontmatter)
│           └── role.json            # Operational config (optional)
```

### .napp Archive for Roles

```
ROLE-XXXX-XXXX.napp (tar.gz)
├── manifest.json          # Identity + metadata
├── ROLE.md                # Persona content
├── role.json              # Operational config
└── signatures.json        # Code signing (optional)
```

### napp_path Column

The `roles.napp_path` DB column points to:
- **User roles:** `user/roles/{name}/` (directory)
- **Installed roles:** `nebo/roles/@org/roles/name/1.0.0/` (extracted directory)

Set during creation via `store.set_role_napp_path()`.

---

## 15. Code Installation (ROLE-XXXX-XXXX)

**Source:** `crates/server/src/codes.rs`

### Code Detection

```rust
pub enum CodeType { Nebo, Skill, Work, Role, Loop }

pub fn detect_code(prompt: &str) -> Option<(CodeType, &str)>
```

Detects `ROLE-XXXX-XXXX` pattern in user messages (Crockford Base32 charset — no I, L, O, U — case-insensitive).

### Installation Flow

```
User enters: "Install ROLE-ABCD-1234"
  ↓
1. Code detected by detect_code() → (CodeType::Role, "ROLE-ABCD-1234")
2. Broadcast "code_processing" event with status "Installing role..."
3. Call handle_role_code(state, code):
   a. Fetch from NeboLoop API: api.install_role(code)
   b. Check payment_required → return checkout_url if needed
   c. Receive: ROLE.md, role.json, manifest
   d. Create Role record in DB
   e. Write files to user/roles/{name}/
   f. Process workflow bindings
   g. Auto-activate role in RoleWorkerRegistry
   h. Register agent in NeboLoop Loop (async)
   i. Cascade-install dependencies (async)
4. Broadcast result (success/failure)
5. Broadcast "chat_complete"
```

---

## 16. Migration

**Source:** `crates/server/src/migration.rs`

### Phase 1: Directory Migration (v2)

Old layout → new layout (same as skills/workflows):

```
~/.nebo/roles/            →  ~/.nebo/user/roles/
```

- Recursively copy directories containing ROLE.md
- Preserve symlinks
- Idempotent (skip if destination exists)
- Write `.migrated-v2` marker

### Phase 2: .napp Extraction (v3)

For sealed `.napp` archives in `nebo/roles/`:
- Extract alongside: `role.napp` → `role/` directory with ROLE.md, role.json
- Idempotent (skip if sibling dir exists)
- Write `.migrated-v3` marker

### DB Migration 0051

Adds `napp_path TEXT` column to `roles` table.

### DB Migration 0053

Creates `role_workflows` table (initial schema: id, role_id, binding_name, trigger_type, trigger_config, description, inputs, is_active).

### DB Migration 0058 — Role Instances

- Renames `roles.code` → `roles.kind`
- Removes UNIQUE constraint on `kind` (allows multiple instances per marketplace kind)
- Adds `workflow_ref TEXT DEFAULT ''` and `workflow_id TEXT` and `last_fired TEXT` to `role_workflows`
- Adds index `idx_roles_kind`

### DB Migration 0059 — Workflow Emit

Adds `emit TEXT` column to `role_workflows` for event emission on completion.

### DB Migration 0060 — Workflow Activities

Adds `activities TEXT` column to `role_workflows` for serialized inline activities JSON.

### DB Migration 0061 — Workflow Runs FK Drop

Drops FK constraint on `workflow_runs.workflow_id` to allow inline runs using `"role:{role_id}"` format.

### DB Migration 0062 — Workflow Run Output

Adds `output TEXT` column to `workflow_runs` for storing completion output.

---

## 17. RoleWorker Runtime

**Source:** `crates/agent/src/role_worker.rs`

### RoleWorker

Each enabled role gets a `RoleWorker` that manages its trigger execution:

```rust
pub struct RoleWorker {
    pub role_id: String,
    pub name: String,
    pub cancel: CancellationToken,    // Shuts down all spawned trigger tasks
}
```

### RoleWorkerRegistry

Manages all active role workers:

```rust
pub struct RoleWorkerRegistry {
    workers: RwLock<HashMap<String, RoleWorker>>,
    store: Arc<Store>,
    workflow_manager: Arc<dyn WorkflowManager>,
    event_dispatcher: Arc<EventDispatcher>,
}
```

**Methods:**
- `start_role(role_id)` — Create and start a new worker (stops old if exists)
- `stop_role(role_id)` — Stop and remove a single worker
- `stop_all()` — Drain all workers on shutdown

### RoleWorker::start() — Trigger Spawning

When started, a RoleWorker:

1. Loads role workflow bindings from DB
2. Parses role config from frontmatter to access inline activities
3. Calls `register_role_triggers()` to create cron jobs for schedule bindings
4. **For each heartbeat binding:** spawns an async loop:
   ```
   loop {
     sleep(interval)
     if in_time_window(window) {
       workflow_manager.run_inline(def_json, inputs, "heartbeat", role_id, emit_source)
     }
   }
   ```
5. **For each event binding:** subscribes to EventDispatcher:
   ```
   for each source pattern in trigger_config:
     dispatcher.subscribe(EventSubscription {
       pattern, default_inputs, role_source, binding_name,
       definition_json, emit_source
     })
   ```
6. **Schedule bindings:** already handled by cron jobs (step 3)
7. **Manual bindings:** no-op (triggered via chat or API)

### RoleWorker::stop()

1. Cancels the `CancellationToken` — stops all spawned heartbeat loops
2. Calls `unregister_role_triggers()` — removes cron jobs

### Server Startup Sequence

**Source:** `crates/server/src/lib.rs`

```
Server startup:
  → Create Store, providers, tools, hub
  → Create WorkflowManager (with providers, tools, hub)
  → Create EventBus + EventDispatcher
  → Create RoleWorkerRegistry (with store, workflow_manager, event_dispatcher)
  → For each enabled role in DB:
      → role_workers.start_role(role_id)
  → Populate active_role_state (role registry for prompt injection)
  → Spawn EventDispatcher loop (matches events → run_inline)
  → Spawn heartbeat scheduler (per-entity heartbeats via run_chat)
  → Spawn cron scheduler (fires role_workflow and workflow cron jobs)
```

### WorkflowManager — run_inline()

**Source:** `crates/server/src/workflow_manager.rs`

Called by RoleWorker (heartbeat/event triggers) and scheduler (schedule triggers):

```rust
fn run_inline(&self,
    definition_json: &str,   // Inline workflow JSON from binding.to_workflow_json()
    inputs: Value,           // Merged inputs (defaults + event data)
    trigger_type: &str,      // "heartbeat", "event", "schedule"
    role_id: &str,           // Owning role
    emit_source: Option<String>,  // Event to emit on completion
) -> Result<String>  // Returns run_id
```

**Flow:**
1. Parse definition via `workflow::parser::parse_workflow()`
2. Create `workflow_runs` record with `session_key = "role-{role_id}-{run_id}"`
3. Spawn background async task:
   a. Post "started" message to role chat (`role:{role_id}:web`)
   b. Load skill content for activities
   c. Execute via `workflow::engine::execute_workflow()`
   d. On success: post completion message, broadcast `"workflow_run_completed"`
   e. On error: post failure message, broadcast `"workflow_run_failed"`
4. Return `run_id` immediately

### Heartbeat Scheduler (Global)

**Source:** `crates/server/src/heartbeat.rs`

Separate from RoleWorker heartbeats. Polls every 60 seconds for entity-level heartbeats configured via the UI:

```
Every 60 seconds:
  → Load entities with heartbeat configs
  → For each enabled entity:
    → Check interval elapsed since last fire
    → Check time window
    → Call run_chat(heartbeat_content, lane=HEARTBEAT, origin=System)
```

This uses `run_chat()` (conversational agent) rather than `run_inline()` (workflow engine).

---

## 18. Complete Lifecycle

### Creation via API

```
POST /roles {
  roleMd: "---\nname: Chief of Staff\n---\n# Persona...",
  roleJson: '{"workflows": {...}, "skills": [...]}'
}
  ↓
1. Parse ROLE.md frontmatter → name, description, skill refs
2. Parse role.json → RoleConfig with workflow bindings + inline activities
3. Merge refs (frontmatter + roleJson skills deduplicated)
4. Create Role record in DB (with kind if provided)
5. Write ROLE.md + role.json to user/roles/{name}/
6. Set role.napp_path
7. process_role_bindings():
   ├─ For each workflow binding:
   │  ├─ Serialize trigger (type + config)
   │  ├─ Serialize activities to JSON
   │  └─ Upsert to role_workflows table (with emit, activities)
   ├─ Register schedule triggers (cron jobs, task_type="role_workflow")
   └─ Register event subscriptions (via EventDispatcher)
8. Broadcast "role_installed"
9. Cascade-install missing skill deps
10. Return role + installReport + cascade
```

### Execution via Schedule Trigger

```
Cron scheduler tick
  ↓
1. Cron job fires: name="role-{role_id}-{binding_name}"
2. task_type = "role_workflow", command = "role:{role_id}:{binding_name}"
3. Scheduler calls execute_role_workflow_task():
   a. Parse command → role_id + binding_name
   b. Load role from DB, parse role config
   c. Get binding, convert to workflow JSON via binding.to_workflow_json()
   d. Build emit_source: "{role-slug}.{emit-name}"
   e. Call workflow_manager.run_inline(def_json, inputs, "schedule", role_id, emit_source)
4. WorkflowManager (run_inline):
   a. Create WorkflowRun record (workflow_id = "role:{role_id}")
   b. Post "started" message to role chat
   c. Spawn background task → execute_workflow()
   d. On complete: post result, broadcast event
```

### Execution via Heartbeat Trigger

```
RoleWorker heartbeat loop tick
  ↓
1. Sleep for configured interval
2. Check time window (if configured)
3. Build inline workflow JSON from binding activities
4. Call workflow_manager.run_inline(def_json, inputs, "heartbeat", role_id, emit_source)
5. Same WorkflowManager flow as schedule trigger
```

### Execution via Event Trigger

```
Some activity emits: emit({ source: "briefing.ready", payload: {...} })
  ↓
1. EventBus delivers event to EventDispatcher
2. EventDispatcher.match_event() → finds subscription:
   { pattern: "briefing.ready", definition_json: "...", role_source: "xyz", ... }
3. Merge event into default_inputs:
   { "_event_source": "briefing.ready", "_event_payload": {...}, ... }
4. Call manager.run_inline(definition_json, merged_inputs, "event", role_source, emit_source)
5. Same WorkflowManager flow as schedule trigger
```

### Emit Chains

Bindings can trigger other bindings via emit:

```
Binding A (emit: "data.ready")
  → completes → emits "chief-of-staff.data.ready"
    → EventDispatcher matches subscription for Binding B
      → Binding B executes with event data as input
```

The emit source is namespaced: `{role-slug}.{emit-name}`.

### Deletion

```
DELETE /roles/{id}
  ↓
1. Unregister all triggers: delete_cron_jobs_by_prefix("role-{id}-")
2. DB cascade: role_workflows deleted automatically via FK
3. Delete Role record
4. Broadcast "role_uninstalled"
```

### Activation / Deactivation

```
POST /roles/{id}/activate
  ↓
1. Start RoleWorker (spawns heartbeat loops, event subscriptions, cron jobs)
2. Register agent in NeboLoop Loop

POST /roles/{id}/deactivate
  ↓
1. Stop RoleWorker (cancel token → stops loops, unregisters triggers)
2. Deregister agent from NeboLoop Loop
```

---

## 19. Integration Points

### With RoleWorker System

- Each enabled role gets a `RoleWorker` at startup
- Workers spawn heartbeat loops and event subscriptions
- Workers register schedule triggers via cron jobs
- Activation/deactivation via HTTP endpoints or code installation
- `CancellationToken` provides clean shutdown

### With Workflow Engine

- Inline activities in role.json are serialized to workflow JSON via `to_workflow_json()`
- `workflow_manager.run_inline()` executes inline workflows in background
- `workflow_runs` table tracks execution history (with `"role:{role_id}"` as workflow_id)
- Standalone workflows still supported via `workflow_manager.run()`

### With Skill System

- Roles declare skill dependencies in `skills[]`
- Inline activities reference skills in `activities[].skills[]`
- Both are cascade-installed when role is created
- Skill content is loaded per-activity during workflow execution

### With Event System

- Event triggers create `EventSubscription` entries via RoleWorker
- `EventDispatcher` matches emitted events to subscriptions
- Matching triggers `run_inline()` with merged inputs and inline definition
- Emit chains allow binding-to-binding event propagation

### With Agent System

- ROLE.md content replaces default identity in system prompt via `PromptContext.active_role`
- Role chat uses session key `"role:{role_id}:web"`
- Workflow messages are posted to role chat for visibility

### With Cron Scheduler

- Schedule triggers create cron jobs named `role-{id}-{binding}`
- Task type: `"role_workflow"` (distinct from standalone `"workflow"`)
- Command format: `"role:{role_id}:{binding_name}"` (resolved at execution time)
- Scheduler calls `execute_role_workflow_task()` → `run_inline()`
- Unregistered on role deletion or deactivation

### With Heartbeat Scheduler

- Global heartbeat scheduler (60s poll) handles per-entity heartbeats configured via UI
- Uses `run_chat()` (conversational agent, HEARTBEAT lane)
- Separate from RoleWorker heartbeat triggers which use `run_inline()` (workflow engine)

### With NeboLoop

- Roles distributed as `.napp` archives
- Install codes: `ROLE-XXXX-XXXX`
- Cascade-installs all referenced skills
- On activation: agent registered in Loop via `register_agent_in_loop()`
- On deactivation: agent deregistered via `deregister_agent_from_loop()`

### With Database

- `roles` table: Role CRUD + metadata (`kind` for marketplace grouping)
- `role_workflows` table: Bindings + triggers + activities + emit (cascade delete)
- `workflow_runs` table: Execution history (FK-free for inline runs)
- `cron_jobs` table: Schedule triggers (prefix-based cleanup)

---

## 20. Cross-Reference to Go Docs

| Rust (this doc) | Go Equivalent |
|---|---|
| `crates/napp/src/role.rs` | `internal/apps/role/config.go` |
| `crates/napp/src/role_loader.rs` | New in Rust |
| `crates/server/src/handlers/roles.rs` | `internal/server/role_routes.go` |
| `crates/db/src/queries/roles.rs` | `internal/db/roles.go` |
| `crates/workflow/src/triggers.rs` | `internal/workflow/triggers.go` |
| `crates/workflow/src/events.rs` | New in Rust |
| `crates/server/src/deps.rs` | New in Rust |
| `crates/agent/src/role_worker.rs` | New in Rust |
| `crates/server/src/workflow_manager.rs` | New in Rust |
| `crates/server/src/heartbeat.rs` | New in Rust |

**Canonical specification:**
- [platform-taxonomy.md](platform-taxonomy.md) — Authoritative ROLE/WORK/SKILL hierarchy definition

---

## 21. Frontend — Role Agent Pages

### Route Structure

All agent routes live under the `(sidebar)` layout group:

```
(sidebar)/agent/
├── +page.svelte              → redirects to /agents
├── [chatId]/+page.svelte     → redirects to /agent (legacy)
├── assistant/
│   ├── +layout.svelte        → Header "Assistant" + 4-tab bar
│   ├── +page.svelte          → redirects to /agent/assistant/chat
│   ├── chat/+page.svelte     → Chat.svelte (mode='companion')
│   ├── automate/+page.svelte → AutomationsSection (readonly=true)
│   ├── activity/+page.svelte → Activity log
│   └── settings/+page.svelte → Links to /settings/personality + /settings/providers
├── channel/[name]/+page.svelte → NeboLoop channel view
└── role/[name]/
    ├── +layout.svelte        → Header with inline-editable role name + 4-tab bar
    ├── +page.svelte          → redirects to /agent/role/[name]/chat
    ├── chat/+page.svelte     → Chat.svelte (mode='role')
    ├── automate/+page.svelte → AutomationsSection with roleId
    ├── activity/+page.svelte → Activity log for this role
    └── settings/+page.svelte → Role settings (pause, resume, delete)
```

**Layout:** `app/src/routes/(app)/(sidebar)/agent/role/[name]/+layout.svelte`
- Header with inline-editable role name + tab bar (Chat | Automate | Activity | Settings)
- Resolves role via `getActiveRoles()` → sets `channelState` context
- Role name editable in-place (click to edit, Enter to save, Escape to cancel)
- Shows 404 if role not found, loading spinner while fetching

### Chat Tab (role mode)

See [CHAT_SYSTEM.md §14](CHAT_SYSTEM.md) for full Chat.svelte details. Role-specific behavior:
- `chatId` = `role:<roleId>:web`
- `agentName` = role name from context
- Fetches `getRole(roleId)` on mount for description (used in empty state)
- Empty state: role initial avatar + name + description + 2 suggestion buttons
- Full feature parity with companion: streaming indicator, tool output sidebar, ask widgets

### Automate Tab

**File:** `app/src/lib/components/agent/AutomationsSection.svelte` (571 lines)

**Props:** `entityType` ('role' | 'main'), `entityId`, `roleId?`, `readonly?`

Two operating modes, selected via radio buttons:

| Mode | Description |
|------|-------------|
| **Proactive check-ins** (`heartbeat`) | Wake on a schedule, check in using agent's judgment |
| **Automations** (`automations`) | Run defined workflow sequences on triggers |

**Mode persistence:** Initial mode is auto-selected on first load (automations if any exist, else heartbeat). User's manual selection is preserved across data reloads — only reset when navigating to a different role.

**Heartbeat config** (when mode = heartbeat):
- Interval select (1min → 24hr)
- Active window (time range pickers)
- Content textarea ("What should I check?") with RichInput (slash mentions)
- Auto-saves via `updateEntityConfig()` with 800ms debounce on content

**Automations list** (when mode = automations):
- Workflow binding cards with trigger icon, description, trigger summary, step count
- Trigger chain visualization (shows "triggered by" links for event-type triggers via `emit` field)
- Per-workflow: toggle active, edit, duplicate, delete (with confirm)
- "New" button opens `AutomationEditor.svelte`
- Empty state with "New Automation" CTA

**Workflow CRUD operations:**
- `openCreate()` — new workflow via AutomationEditor modal
- `openEdit(wf)` — edit existing binding
- `handleDuplicate(wf)` — copy workflow
- `handleToggle(wf)` — enable/disable via `toggleRoleWorkflow(roleId, bindingName)`
- `handleDelete(bindingName)` — remove via `deleteRoleWorkflow(roleId, bindingName)`

**Data loading:**
- `getEntityConfig(entityType, entityId)` → heartbeat settings
- `getRoleWorkflows(roleId)` → workflow bindings
- Loaded in parallel via `Promise.all`
- Reloads when `entityType` or `entityId` changes (via `$effect`)

### Additional Components

| File | Purpose |
|------|---------|
| `AutomationEditor.svelte` | Modal editor for creating/editing role workflow bindings with trigger config |
| `CommandCenter.svelte` | Role listing dashboard showing active roles and recent sessions |
| `NewBotMenu.svelte` | Menu for creating new bots/roles |
| `SkillsList.svelte` | List of available skills |
| `RoleSetupModal.svelte` | Installation modal for marketplace roles (install, configure inputs, activate) |

---

*Last updated: 2026-03-16*
