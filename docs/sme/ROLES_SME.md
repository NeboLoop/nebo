# Role System — Rust SME Reference

> Definitive reference for the Nebo Rust role system. Covers the role definition
> format (ROLE.md + role.json), trigger types, workflow bindings, dependency cascade,
> database schema, HTTP endpoints, filesystem storage, event subscriptions, agent
> persona injection, and the full installation-to-execution lifecycle.

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
17. [Complete Lifecycle](#17-complete-lifecycle)
18. [Integration Points](#18-integration-points)
19. [Cross-Reference to Go Docs](#19-cross-reference-to-go-docs)

---

## 1. System Overview

A **Role** is a job description with a schedule. It sits at the top of the ROLE → WORK → SKILL hierarchy:

```
ROLE (schedule of intent)
  ├─ ROLE.md      — persona / job description (pure prose)
  ├─ role.json    — operational config (workflow bindings + triggers)
  │
  ├─ WORKFLOW 1   — procedure (what to do)
  │  └─ SKILL A   — domain knowledge
  │  └─ SKILL B
  │
  └─ WORKFLOW 2
     └─ SKILL C
```

> **Key principle:** The workflow does not decide when it runs. The Role does.

**Key properties:**
- Roles **own triggers** — cron schedules, heartbeat intervals, event subscriptions, manual
- Roles **bind workflows** — each binding associates a workflow ref with a trigger and default inputs
- Roles **declare skill dependencies** — skills required by the role itself (beyond those in workflows)
- Roles **define the agent's persona** — ROLE.md is injected into the system prompt
- Installing a role **cascades downward** — auto-installs required workflows and skills

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
    pub body: String,         // Full markdown content
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
    workflows: Vec<String>,    // Workflow refs from frontmatter
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

```rust
pub struct WorkflowBinding {
    #[serde(rename = "ref")]
    pub workflow_ref: String,                     // "@nebo/workflows/daily-briefing@^1.0.0"
    pub trigger: RoleTrigger,
    pub description: String,
    pub inputs: HashMap<String, serde_json::Value>,
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
```

**Validation rules:**
- Workflow refs must be qualified names (`@org/workflows/name`) or install codes (`WORK-XXXX-XXXX`)
- Skill refs must be qualified names (`@org/skills/name`) or install codes (`SKIL-XXXX-XXXX`)
- Event triggers must have at least one source
- Empty config `{}` is valid (no workflows or skills)

### Reference Validation

```rust
fn is_qualified_ref(s: &str, expected_type: &str) -> bool
```

Accepts two formats:
- **Qualified name:** `@org/type/name` or `@org/type/name@version` — type segment must match `expected_type`
- **Install code:** `WORK-XXXX-XXXX` (for workflows) or `SKIL-XXXX-XXXX` (for skills)

### Example role.json

```json
{
  "workflows": {
    "morning-briefing": {
      "ref": "@nebo/workflows/daily-briefing@^1.0.0",
      "trigger": { "type": "schedule", "cron": "0 7 * * *" },
      "description": "Generate daily briefing at 7 AM",
      "inputs": { "department": "engineering" }
    },
    "day-monitor": {
      "ref": "@nebo/workflows/day-monitor@^1.0.0",
      "trigger": { "type": "heartbeat", "interval": "30m", "window": "08:00-18:00" }
    },
    "interrupt": {
      "ref": "@nebo/workflows/urgent-interrupt@^1.0.0",
      "trigger": { "type": "event", "sources": ["calendar.changed", "email.urgent"] }
    },
    "ad-hoc": {
      "ref": "@acme/workflows/ad-hoc@1.0.0",
      "trigger": { "type": "manual" }
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
| `schedule` | `cron` (string) | Fires on a cron schedule. Standard 5-field cron expression. | Cron job via `store.upsert_cron_job()` |
| `heartbeat` | `interval` (string), `window?` (string) | Recurring interval within optional time window. | Stored in DB, future enhancement |
| `event` | `sources` (string array) | Fires when matching event is emitted. Pattern supports wildcards. | `EventSubscription` via `EventDispatcher` |
| `manual` | — | Explicit user trigger only. | Stored in DB, triggered via WorkTool |

### Trigger Config Serialization

When stored in `role_workflows.trigger_config`:
- **Schedule:** cron expression string (e.g., `"0 7 * * *"`)
- **Heartbeat:** `"{interval}"` or `"{interval}|{window}"` (e.g., `"30m|08:00-18:00"`)
- **Event:** comma-separated sources (e.g., `"calendar.changed,email.urgent"`)
- **Manual:** empty string

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
3. Return `LoadedRole` with both

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

### roles

```sql
CREATE TABLE IF NOT EXISTS roles (
    id TEXT PRIMARY KEY,
    code TEXT UNIQUE,                     -- ROLE-XXXX-XXXX
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
```

### role_workflows (migration 0053)

```sql
CREATE TABLE IF NOT EXISTS role_workflows (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    role_id TEXT NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    binding_name TEXT NOT NULL,            -- Key from role.json workflows map
    workflow_ref TEXT NOT NULL,            -- @org/workflows/name or WORK-XXXX-XXXX
    workflow_id TEXT,                      -- Resolved local workflow ID (nullable)
    trigger_type TEXT NOT NULL,            -- schedule, event, heartbeat, manual
    trigger_config TEXT NOT NULL,          -- Cron, interval|window, sources CSV, or empty
    description TEXT,
    inputs TEXT,                          -- Default inputs JSON
    is_active INTEGER NOT NULL DEFAULT 1,
    UNIQUE(role_id, binding_name)
);
```

**Cascade behavior:** Deleting a role automatically deletes all its `role_workflows` entries.

### Rust Models

```rust
pub struct Role {
    pub id: String,
    pub code: Option<String>,
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
    pub workflow_ref: String,
    pub workflow_id: Option<String>,
    pub trigger_type: String,
    pub trigger_config: String,
    pub description: Option<String>,
    pub inputs: Option<String>,
    pub is_active: i64,
}
```

---

## 7. Database Queries

**Source:** `crates/db/src/queries/roles.rs`

```rust
// CRUD
pub fn list_roles(&self, limit: i64, offset: i64) -> Result<Vec<Role>>
pub fn count_roles(&self) -> Result<i64>
pub fn get_role(&self, id: &str) -> Result<Option<Role>>
pub fn create_role(&self, id, code, name, description, role_md, frontmatter, pricing_model, pricing_cost) -> Result<Role>
pub fn update_role(&self, id, name, description, role_md, frontmatter, pricing_model, pricing_cost) -> Result<()>
pub fn delete_role(&self, id: &str) -> Result<()>
pub fn set_role_napp_path(&self, id: &str, napp_path: &str) -> Result<()>
pub fn toggle_role(&self, id: &str) -> Result<()>

// Role-Workflow Bindings
pub fn upsert_role_workflow(&self, role_id, binding_name, workflow_ref, workflow_id, trigger_type, trigger_config, description, inputs) -> Result<()>
pub fn list_role_workflows(&self, role_id: &str) -> Result<Vec<RoleWorkflow>>
pub fn delete_role_workflows(&self, role_id: &str) -> Result<()>
pub fn list_active_event_triggers(&self) -> Result<Vec<RoleWorkflow>>
pub fn delete_cron_jobs_by_prefix(&self, prefix: &str) -> Result<i64>
```

---

## 8. HTTP Endpoints

**Source:** `crates/server/src/handlers/roles.rs`

| Method | Path | Handler | Purpose |
|---|---|---|---|
| GET | `/roles` | `list_roles` | List roles (paginated, max 100) |
| POST | `/roles` | `create_role` | Create role + process bindings + cascade deps |
| GET | `/roles/{id}` | `get_role` | Get single role |
| PUT | `/roles/{id}` | `update_role` | Update ROLE.md + metadata |
| DELETE | `/roles/{id}` | `delete_role` | Delete + unregister all triggers |
| POST | `/roles/{id}/toggle` | `toggle_role` | Enable/disable |
| POST | `/roles/{id}/install-deps` | `install_deps` | Force-install all missing dependencies |

### POST /roles (Create)

**Request body:**
```json
{
  "roleMd": "---\nname: Chief of Staff\ndescription: ...\n---\n# Content",
  "code": "ROLE-XXXX-XXXX",
  "roleJson": "{...role.json content...}",
  "name": "Chief of Staff",
  "description": "Manages executive rhythm"
}
```

**Handler flow:**
1. Parse ROLE.md frontmatter → extract name, description, workflow/skill refs
2. If `roleJson` provided, parse RoleConfig and merge refs into frontmatter
3. Create Role record in DB
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
    { "binding": "morning-briefing", "status": "linked", "workflowId": "abc123" },
    { "binding": "lead-scorer", "status": "pending" }
  ],
  "cascade": {
    "results": [...],
    "installed_count": 2,
    "pending_count": 0,
    "failed_count": 0
  }
}
```

### DELETE /roles/{id}

**Handler flow:**
1. Unregister all triggers: `workflow::triggers::unregister_role_triggers(&id, &store)`
   - Deletes cron jobs with prefix `role-{id}-`
2. Delete role from DB (cascade deletes `role_workflows`)
3. Broadcast `"role_uninstalled"` event

### POST /roles/{id}/install-deps

**Handler flow:**
1. Extract deps from role's `frontmatter` JSON
2. Extract deps from `role_workflows` bindings
3. Call `resolve_cascade_force()` — installs all missing deps regardless of autonomy mode
4. Return cascade result

---

## 9. Workflow Binding Processing

**Source:** `crates/server/src/handlers/roles.rs` — `process_role_bindings()`

When a role with `roleJson` is created or updated, this function processes each workflow binding:

### Algorithm

```
FOR EACH (binding_name, binding) in role_config.workflows:

1. Serialize trigger:
   - Schedule → ("schedule", cron_string)
   - Heartbeat → ("heartbeat", "{interval}|{window}")
   - Event → ("event", "source1,source2,...")
   - Manual → ("manual", "")

2. Resolve workflow_ref → workflow_id:
   - If starts with "WORK-": look up by code
   - Else: search by name (case-insensitive) or ID

3. Upsert to role_workflows table:
   - role_id, binding_name, workflow_ref, workflow_id,
     trigger_type, trigger_config, description, inputs

4. Report status:
   - "linked" if workflow_id resolved
   - "pending" if workflow not yet installed
   - "error" if DB upsert failed

AFTER all bindings processed:

5. Register triggers (schedule → cron jobs)
6. Register event subscriptions (event → EventDispatcher)
```

### Report Format

Each binding produces a report entry:

```json
{
  "binding": "morning-briefing",
  "ref": "@nebo/workflows/daily-briefing@^1.0.0",
  "workflowId": "abc123",
  "triggerType": "schedule",
  "status": "linked"
}
```

Status values: `"linked"` (workflow found), `"pending"` (workflow not installed), `"error"` (DB failure).

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
- Command: the resolved `workflow_id`
- Task type: `"workflow"`
- Enabled: `true`

When the scheduler fires a cron job with `task_type = "workflow"`:
- `command` field contains the `workflow_id`
- Scheduler calls `workflow_manager.run(workflow_id, {}, "schedule")`

### Unregistration

```rust
pub fn unregister_role_triggers(role_id: &str, store: &Store) {
    let prefix = format!("role-{}-", role_id);
    store.delete_cron_jobs_by_prefix(&prefix);
}
```

Deletes all cron jobs whose name starts with `role-{role_id}-`. Called on role deletion.

---

## 11. Event Subscriptions

**Source:** `crates/workflow/src/events.rs`, `crates/server/src/handlers/roles.rs`

### Registration

During `process_role_bindings()`, for each binding with `trigger_type == "event"`:

1. Split `trigger_config` by comma to get individual event source patterns
2. For each source pattern, create an `EventSubscription`:

```rust
EventSubscription {
    pattern: "email.urgent",                // Exact or wildcard (email.*)
    workflow_id: "abc123",                  // Resolved workflow ID
    default_inputs: { "priority": "high" }, // From binding.inputs
    role_source: "role-xyz",                // Role ID for tracing
    binding_name: "interrupt",              // Binding key from role.json
}
```

3. Subscribe to `EventDispatcher`: `state.event_dispatcher.subscribe(sub).await`

### Pattern Matching

When an event is emitted (e.g., from a workflow activity via `emit` tool):
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
2. Call `manager.run(workflow_id, merged_inputs, "event")`
3. Workflow executes in background

---

## 12. Dependency Cascade

**Source:** `crates/server/src/deps.rs`

### Dependency Extraction

```rust
pub fn extract_role_deps(config: &RoleConfig) -> Vec<DepRef>
```

Extracts deps from role.json:
- Each workflow binding's `workflow_ref` → `DepType::Workflow`
- Each skill in `skills[]` → `DepType::Skill`

```rust
pub fn extract_role_deps_from_frontmatter(frontmatter_json: &str) -> Vec<DepRef>
```

Extracts deps from the stored frontmatter JSON:
- `workflows[]` array → `DepType::Workflow`
- `skills[]` array → `DepType::Skill`

### Cascade Resolution

When a role is created:

```
Role created with:
  - workflows: ["@nebo/workflows/daily-briefing@^1.0.0"]
  - skills: ["@nebo/skills/briefing-writer@^1.0.0"]

resolve_cascade():
  ├─ Check workflow "@nebo/workflows/daily-briefing@^1.0.0"
  │  ├─ Already installed? → AlreadyInstalled
  │  └─ Not installed? →
  │     ├─ Autonomous mode → auto-install from NeboLoop
  │     └─ Non-autonomous → mark PendingApproval
  │
  ├─ Check skill "@nebo/skills/briefing-writer@^1.0.0"
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

### How Roles Affect the Agent

The role's ROLE.md content shapes the agent's behavior through the prompt system:

```rust
pub struct PromptContext {
    pub agent_name: String,
    pub tool_names: Vec<String>,
    pub active_skill: Option<String>,
    pub skill_hints: Vec<String>,
    // Role data comes through agent profile
}
```

When a role is active:
1. ROLE.md body is stored in the agent's profile (via `bot(resource: "profile", action: "update")`)
2. The persona content is included in the system prompt construction
3. The agent assumes the role's communication style and behavioral guidelines

### Separation of Concerns

- **ROLE.md** → Agent's conversational persona (used in chat context)
- **Activity skills** → Pure knowledge for workflow activities (no persona bleed)
- **role.json** → Operational scheduling (never in prompts, only drives automation)

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
pub fn detect_code(prompt: &str) -> Option<(CodeType, &str)>
```

Detects `ROLE-XXXX-XXXX` pattern in user messages (Crockford Base32 charset, case-insensitive).

### Installation Flow

```
User enters: "Install ROLE-ABCD-1234"
  ↓
1. Code detected by detect_code() → (CodeType::Role, "ROLE-ABCD-1234")
2. Broadcast "code_processing" event
3. Call handle_role_code(state, code):
   a. Fetch from NeboLoop API: api.install_role(code)
   b. Receive: ROLE.md, role.json, manifest
   c. Create Role record in DB
   d. Write files to user/roles/{name}/
   e. Process workflow bindings
   f. Cascade-install dependencies
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

Adds `napp_path TEXT` column to `roles` table, linking each role to its filesystem location.

### DB Migration 0053

Creates `role_workflows` table for storing role → workflow bindings with trigger configuration.

---

## 17. Complete Lifecycle

### Creation via API

```
POST /roles {
  roleMd: "---\nname: Chief of Staff\n---\n# Persona...",
  roleJson: '{"workflows": {...}, "skills": [...]}'
}
  ↓
1. Parse ROLE.md frontmatter → name, description, refs
2. Parse role.json → RoleConfig with workflow bindings
3. Merge refs (frontmatter + roleJson deduplicated)
4. Create Role record in DB
5. Write ROLE.md + role.json to user/roles/{name}/
6. Set role.napp_path
7. process_role_bindings():
   ├─ For each workflow binding:
   │  ├─ Serialize trigger (type + config)
   │  ├─ Resolve workflow_ref → workflow_id
   │  └─ Upsert to role_workflows table
   ├─ Register schedule triggers (cron jobs)
   └─ Register event subscriptions
8. Broadcast "role_installed"
9. Cascade-install missing deps
10. Return role + installReport + cascade
```

### Execution via Schedule Trigger

```
Cron scheduler tick
  ↓
1. Cron job fires: name="role-{role_id}-morning-briefing"
2. task_type = "workflow", command = workflow_id
3. Scheduler calls workflow_manager.run(workflow_id, {}, "schedule")
4. WorkflowManager:
   a. Load workflow definition
   b. Create WorkflowRun record
   c. Spawn background task
   d. Execute activities sequentially
   e. Complete run record
   f. Broadcast result
```

### Execution via Event Trigger

```
Activity emits: emit({ source: "email.urgent", payload: {...} })
  ↓
1. EventBus.emit() → unbounded channel
2. EventDispatcher receives event
3. match_event() → finds subscription:
   { pattern: "email.urgent", workflow_id: "abc", default_inputs: {...} }
4. Merge event into default_inputs
5. manager.run("abc", merged_inputs, "event")
6. Background workflow execution
```

### Deletion

```
DELETE /roles/{id}
  ↓
1. Unregister all triggers: delete_cron_jobs_by_prefix("role-{id}-")
2. DB cascade: role_workflows deleted automatically via FK
3. Delete Role record
4. Broadcast "role_uninstalled"
```

---

## 18. Integration Points

### With Workflow System

- Roles bind workflows via `role_workflows` table
- Roles own trigger scheduling (workflows are pure procedures)
- `process_role_bindings()` creates cron jobs and event subscriptions
- Workflow refs can be qualified names or install codes

### With Skill System

- Roles declare skill dependencies in `skills[]`
- Skills are cascade-installed when role is created
- Skills referenced by role's workflows are also included in cascade

### With Event System

- Event triggers create `EventSubscription` entries
- `EventDispatcher` matches emitted events to subscriptions
- Matching triggers workflow execution with merged inputs

### With Agent System

- ROLE.md content shapes agent persona via system prompt
- Roles do not affect tool availability (that is workflow/activity-level)
- Agent profile stores active role context

### With Cron Scheduler

- Schedule triggers create cron jobs named `role-{id}-{binding}`
- Scheduler fires → `workflow_manager.run()`
- Unregistered on role deletion

### With NeboLoop Marketplace

- Roles distributed as `.napp` archives
- Install codes: `ROLE-XXXX-XXXX`
- Cascade-installs all referenced workflows and skills
- Version resolution via semver ranges in qualified names

### With Database

- `roles` table: Role CRUD + metadata
- `role_workflows` table: Bindings + triggers (cascade delete)
- `cron_jobs` table: Schedule triggers (prefix-based cleanup)

---

## 19. Cross-Reference to Go Docs

| Rust (this doc) | Go Equivalent |
|---|---|
| `crates/napp/src/role.rs` | `internal/apps/role/config.go` |
| `crates/napp/src/role_loader.rs` | New in Rust |
| `crates/server/src/handlers/roles.rs` | `internal/server/role_routes.go` |
| `crates/db/src/queries/roles.rs` | `internal/db/roles.go` |
| `crates/workflow/src/triggers.rs` | `internal/workflow/triggers.go` |
| `crates/workflow/src/events.rs` | New in Rust |
| `crates/server/src/deps.rs` | New in Rust |

**Canonical specification:**
- [platform-taxonomy.md](platform-taxonomy.md) — Authoritative ROLE/WORK/SKILL hierarchy definition

---

*Last updated: 2026-03-08*
