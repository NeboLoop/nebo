# Agent System & Workflows — Rust SME Reference

> Definitive reference for the Nebo Rust agent and workflow system. Covers agent
> definitions (AGENT.md + agent.json), the workflow engine, trigger types (schedule,
> heartbeat, event, watch, manual), the cron scheduler, event dispatcher, AgentWorker
> runtime, database schema, HTTP endpoints, and known issues.

**Last verified against source:** 2026-04-05

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [Agent Definition Format](#2-agent-definition-format)
3. [Agent Configuration (agent.json)](#3-agent-configuration-agentjson)
4. [Agent Loader](#4-agent-loader)
5. [Database Schema](#5-database-schema)
6. [Database Queries](#6-database-queries)
7. [HTTP Endpoints](#7-http-endpoints)
8. [Agent Lifecycle](#8-agent-lifecycle)
9. [Workflow System](#9-workflow-system)
10. [Workflow Engine](#10-workflow-engine)
11. [Trigger System](#11-trigger-system)
12. [Cron Scheduler](#12-cron-scheduler)
13. [Event System](#13-event-system)
14. [AgentWorker Runtime](#14-agentworker-runtime)
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
                    │                   AGENT SYSTEM                       │
                    │                                                     │
  ┌──────────┐     │  ┌───────────┐     ┌──────────────┐                 │
  │ AGENT.md  │────>│  │  agents DB │<───>│ agent.json /  │                 │
  │ (persona)│     │  │  (SQLite) │     │ frontmatter  │                 │
  └──────────┘     │  └─────┬─────┘     └──────┬───────┘                 │
                    │        │                  │                         │
                    │        ▼                  ▼                         │
                    │  ┌─────────────────────────────────┐               │
                    │  │       agent_workflows table       │               │
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
  │  Cron    │   │  AgentWorker  │   │EventDispatcher│   │  Manual  │   │
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

**Core design:** An Agent is a job description with a schedule. It contains:
- **AGENT.md** -- persona/instructions (system prompt identity)
- **agent.json** -- operational config (workflow bindings, triggers, inputs, skills, pricing)

Workflow bindings are owned by agents. Each binding has a trigger type (schedule,
heartbeat, event, manual) and zero or more inline activities. Activities execute
as LLM agent tasks with full tool access.

---

## 2. Agent Definition Format

### AGENT.md

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

**Parsing** (`crates/napp/src/agent.rs:451-470`):

```rust
pub fn parse_agent(content: &str) -> Result<AgentDef, NappError> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        // Pure prose -- no frontmatter
        return Ok(AgentDef { id: String::new(), name: String::new(),
                            description: String::new(), body: content.trim().to_string() });
    }
    // Legacy frontmatter format
    let (yaml_str, body) = split_frontmatter(content)?;
    let mut def: AgentDef = serde_yaml::from_str(&yaml_str)?;
    def.body = body;
    Ok(def)
}
```

**AgentDef struct:**
```rust
pub struct AgentDef {
    pub id: String,           // Empty for modern agents
    pub name: String,         // From frontmatter or empty
    pub description: String,  // From frontmatter or empty
    pub body: String,         // Markdown body after frontmatter
}
```

The handler also parses AGENT.md frontmatter separately in `agents.rs:36-56` using
a `AgentFrontmatter` struct that extracts `name`, `description`, `skills`, and
`pricing` fields for DB storage.

---

## 3. Agent Configuration (agent.json)

**File:** `crates/napp/src/agent.rs`

The `agent.json` file carries the operational structure: inline workflow
definitions, triggers, dependencies, pricing, and input fields.

### AgentConfig

```rust
pub struct AgentConfig {
    pub workflows: HashMap<String, WorkflowBinding>,  // binding_name -> binding
    pub skills: Vec<String>,                           // Qualified skill refs
    pub requires: AgentRequires,                       // Hard deps (plugins, etc.)
    pub pricing: Option<AgentPricing>,
    pub defaults: Option<AgentDefaults>,
    pub inputs: Vec<AgentInputField>,                   // Dynamic form fields
}

pub struct AgentRequires {
    pub plugins: Vec<String>,   // Plugin install codes (e.g., "PLUG-PJ3Z-ECFV")
}
```

### WorkflowBinding

```rust
pub struct WorkflowBinding {
    pub trigger: AgentTrigger,                          // When this runs
    pub description: String,                           // Human-readable
    pub inputs: HashMap<String, serde_json::Value>,    // Default inputs
    pub activities: Vec<AgentActivity>,                 // Inline procedure (empty = chat-only)
    pub budget: AgentBudget,                            // Token constraints
    pub emit: Option<String>,                          // Event to announce on completion
}
```

**Key methods:**

- `has_activities()` -- returns true if `activities` is non-empty
- `to_workflow_json(name)` -- serializes to a `WorkflowDef`-compatible JSON string

### AgentTrigger (tagged enum)

```rust
#[serde(tag = "type")]
pub enum AgentTrigger {
    Schedule { cron: String },                    // Cron expression
    Heartbeat { interval: String, window: Option<String> },  // "30m|08:00-18:00"
    Event { sources: Vec<String> },               // ["email.*", "cal.changed"]
    Watch {                                        // Long-running plugin NDJSON watcher
        plugin: String,                            // Plugin slug (e.g., "gws")
        command: String,                           // CLI args (default empty, resolved from manifest)
        event: Option<String>,                     // Plugin event name → auto-emit + command resolution
        restart_delay_secs: u64,                   // Restart delay on crash (default 5)
    },
    Manual,                                        // User-initiated
}
```

**Watch trigger details:** When `event` is set, the command is resolved from the plugin manifest's `events` array (see `docs/sme/PLUGIN_SYSTEM.md` §5). NDJSON output auto-emits into the EventBus as `{plugin}.{event}`. If activities are defined, the inline workflow also runs (dual mode). Watches without activities are valid — they only auto-emit. Template `{{key}}` placeholders in commands are substituted from agent `input_values` via `substitute_inputs()`. See `crates/agent/src/agent_worker.rs` for implementation.

### AgentActivity

```rust
pub struct AgentActivity {
    pub id: String,
    pub intent: String,           // Task description
    pub skills: Vec<String>,      // Skill references
    pub mcps: Vec<String>,        // MCP server references
    pub cmds: Vec<String>,        // Command references (emit, exit)
    pub model: String,            // Model override
    pub steps: Vec<String>,
    pub token_budget: AgentTokenBudget,  // Default max: 4096
    pub on_error: AgentOnError,          // Default: retry 1, fallback NotifyOwner
}
```

### AgentInputField

```rust
pub struct AgentInputField {
    pub key: String,              // Unique key for workflows
    pub label: String,            // Display label
    pub name: Option<String>,     // NeboLoop alias for key
    pub description: Option<String>,
    pub field_type: String,       // text, textarea, number, select, checkbox, radio
    pub required: bool,
    pub default: Option<serde_json::Value>,
    pub placeholder: Option<String>,
    pub options: Vec<AgentInputOption>,
}
```

### Example agent.json

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

### Validation (`validate_agent_config`)

1. Event triggers must have at least one source
2. Activity IDs must be unique within each binding
3. Skill refs that aren't qualified names (`@org/skills/name` or `SKIL-XXXX-XXXX`) get a warning but don't reject

---

## 4. Agent Loader

**File:** `crates/napp/src/agent_loader.rs`

Filesystem scanner that loads agents from two directories:

| Directory | Source Type | Description |
|-----------|-----------|-------------|
| `<data_dir>/nebo/agents/` | `Installed` | Marketplace .napp archives (sealed) |
| `<data_dir>/user/agents/` | `User` | User-created loose files |

### LoadedAgent

```rust
pub struct LoadedAgent {
    pub agent_def: AgentDef,              // From AGENT.md
    pub config: Option<AgentConfig>,     // From agent.json (if exists)
    pub source: AgentSource,             // Installed or User
    pub napp_path: Option<PathBuf>,
    pub source_path: PathBuf,
    pub version: Option<String>,        // From manifest.json
}
```

### Scanning

- `load_from_dir(dir, source)` -- loads AGENT.md + optional agent.json + manifest.json
- `scan_installed_agents(dir)` -- walks for `AGENT.md` marker files
- `scan_user_agents(dir)` -- shallow read_dir, checks for `AGENT.md` in each subdirectory
- Falls back to directory name if AGENT.md has no frontmatter name

The `list_agents` handler merges DB agents with filesystem agents, deduplicating by name.

---

## 5. Database Schema

### agents

```sql
CREATE TABLE agents (
    id TEXT PRIMARY KEY,
    kind TEXT,                          -- Marketplace code (was 'code', renamed in 0058)
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    agent_md TEXT NOT NULL,              -- Full AGENT.md content
    frontmatter TEXT NOT NULL,          -- agent.json as JSON string
    pricing_model TEXT,
    pricing_cost REAL,
    is_enabled INTEGER NOT NULL DEFAULT 1,
    installed_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    napp_path TEXT,                     -- Filesystem path (added 0051)
    input_values TEXT NOT NULL DEFAULT '{}'  -- User-supplied values (added 0064)
);
CREATE INDEX idx_agents_kind ON agents(kind);
```

**Key points:**
- `kind` allows multiple instances of the same marketplace agent (not UNIQUE)
- `frontmatter` stores the full agent.json content as a JSON string
- `input_values` stores user-supplied form values, separate from the schema in frontmatter

### agent_workflows

```sql
CREATE TABLE agent_workflows (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
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
    UNIQUE(agent_id, binding_name)
);
CREATE INDEX idx_agent_workflows_agent ON agent_workflows(agent_id);
```

**Key points:**
- `is_active` controls whether a binding fires -- toggled via REST endpoint
- `activities` stores the inline activity definitions as JSON (denormalized from frontmatter)
- FK cascade: deleting an agent auto-deletes its agent_workflows

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
    workflow_id TEXT NOT NULL,               -- "persona:{agent_id}" for inline runs, workflow ID for standalone
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
inline agent runs where `workflow_id = "persona:{agent_id}"`.

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
    command TEXT DEFAULT '',                 -- Shell cmd / workflow ID / "persona:id:binding"
    task_type TEXT DEFAULT 'bash',           -- bash, shell, agent, workflow, agent_workflow
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
    entity_type TEXT NOT NULL CHECK(entity_type IN ('main', 'agent', 'channel')),
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
agents (1) ──→ (many) agent_workflows ──→ (via trigger registration) cron_jobs
agents ──→ (via "persona:{id}") workflow_runs ──→ (many) workflow_activity_results
cron_jobs (1) ──→ (many) cron_history
entity_config ── standalone per (entity_type, entity_id) pair
```

---

## 6. Database Queries

### Agent Queries (`crates/db/src/queries/agents.rs`)

| Method | SQL | Purpose |
|--------|-----|---------|
| `list_agents(limit, offset)` | `SELECT ... FROM agents ORDER BY installed_at DESC` | List all agents |
| `count_agents()` | `SELECT COUNT(*) FROM agents` | Total agent count |
| `get_agent(id)` | `SELECT ... FROM agents WHERE id = ?1` | Get single agent |
| `create_agent(...)` | `INSERT INTO agents ... RETURNING ...` | Create agent |
| `update_agent(...)` | `UPDATE agents SET ...` | Update agent fields |
| `delete_agent(id)` | `DELETE FROM agents WHERE id = ?1` | Delete (cascades to agent_workflows) |
| `toggle_agent(id)` | `UPDATE agents SET is_enabled = NOT is_enabled` | Toggle enabled state |
| `set_agent_enabled(id, bool)` | `UPDATE agents SET is_enabled = ?1` | Set explicit state |
| `set_agent_napp_path(id, path)` | `UPDATE agents SET napp_path = ?1` | Set filesystem path |
| `update_agent_input_values(id, json)` | `UPDATE agents SET input_values = ?1` | Store user inputs |
| `agent_installed_by_name(name)` | `SELECT COUNT(*) ... WHERE LOWER(name) = LOWER(?1)` | Check if installed |

### Agent Workflow Queries

| Method | SQL | Purpose |
|--------|-----|---------|
| `upsert_agent_workflow(...)` | `INSERT ... ON CONFLICT DO UPDATE` | Create or update binding |
| `list_agent_workflows(agent_id)` | `SELECT ... WHERE agent_id = ?1` | List bindings for agent |
| `delete_single_agent_workflow(agent_id, binding)` | `DELETE ... WHERE agent_id AND binding_name` | Delete one binding |
| `toggle_agent_workflow(agent_id, binding)` | `UPDATE SET is_active = NOT is_active` | Toggle + return new state |
| `delete_agent_workflows(agent_id)` | `DELETE ... WHERE agent_id = ?1` | Delete all for agent |
| `list_active_event_triggers()` | `WHERE trigger_type='event' AND is_active=1 AND r.is_enabled=1` | Active event bindings |
| `update_agent_workflow_last_fired(...)` | `UPDATE SET last_fired = ?1` | Record last execution |
| `list_emit_sources()` | `WHERE emit IS NOT NULL AND is_active=1` | Available event sources |
| `delete_cron_jobs_by_prefix(prefix)` | `DELETE ... WHERE name LIKE ?1%` | Cleanup cron jobs for agent |

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
| `agent_workflow_stats(agent_id)` | Aggregate stats (total/completed/failed/cancelled/running/tokens) |
| `agent_recent_errors(agent_id, limit)` | Recent failure messages |

---

## 7. HTTP Endpoints

### Agents -- `/api/v1/agents`

| Method | Path | Handler | Description |
|--------|------|---------|-------------|
| GET | `/agents` | `list_agents` | List all agents (DB + filesystem merged) |
| POST | `/agents` | `create_agent` | Create agent from AGENT.md + optional agent.json |
| GET | `/agents/active` | `list_active_agents` | Currently active agents from AgentRegistry |
| GET | `/agents/event-sources` | `list_event_sources` | Available emit names from active bindings |
| GET | `/agents/{id}` | `get_agent` | Get agent with version and normalized inputFields |
| PUT | `/agents/{id}` | `update_agent` | Update agent fields |
| DELETE | `/agents/{id}` | `delete_agent` | Delete agent + cleanup triggers + filesystem |
| POST | `/agents/{id}/toggle` | `toggle_agent` | Toggle is_enabled, start/stop worker |
| POST | `/agents/{id}/activate` | `activate_agent` | Activate + add to registry + start worker |
| POST | `/agents/{id}/deactivate` | `deactivate_agent` | Deactivate + remove from registry + stop worker |
| POST | `/agents/{id}/duplicate` | `duplicate_agent` | Deep copy with "(Copy)" suffix, auto-activate |
| POST | `/agents/{id}/install-deps` | `install_deps` | Force-resolve skill dependencies |
| POST | `/agents/{id}/check-update` | `check_agent_update` | Check NeboLoop for newer version |
| POST | `/agents/{id}/apply-update` | `apply_agent_update` | Download and apply latest from NeboLoop |
| POST | `/agents/{id}/reload` | `reload_agent` | Re-read AGENT.md + agent.json from filesystem |
| POST | `/agents/{id}/setup` | `trigger_agent_setup` | Broadcast setup event for frontend wizard |
| PUT | `/agents/{id}/inputs` | `update_agent_inputs` | Store user-supplied input values |
| GET | `/agents/{id}/stats` | `agent_stats` | Aggregate workflow run statistics |
| GET | `/agents/{id}/runs` | `list_agent_runs` | List workflow runs for agent |
| POST | `/agents/{id}/chat` | `chat_with_agent` | Send message to agent's session |

### Agent Workflows -- `/api/v1/agents/{id}/workflows`

| Method | Path | Handler | Description |
|--------|------|---------|-------------|
| GET | `/agents/{id}/workflows` | `list_agent_workflows` | List bindings for an agent |
| POST | `/agents/{id}/workflows` | `create_agent_workflow` | Create new binding |
| PUT | `/agents/{id}/workflows/{binding}` | `update_agent_workflow` | Update existing binding |
| DELETE | `/agents/{id}/workflows/{binding}` | `delete_agent_workflow` | Delete binding + triggers |
| POST | `/agents/{id}/workflows/{binding}/toggle` | `toggle_agent_workflow` | Toggle is_active + register/unregister triggers |

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

## 8. Agent Lifecycle

### Create (`create_agent`)

1. Parse AGENT.md frontmatter (name, description, skills, pricing)
2. Generate UUID v4 as agent ID
3. Merge skills from AGENT.md frontmatter and agent.json
4. Build frontmatter JSON (full agent.json with merged skills)
5. Insert into `agents` table
6. Write AGENT.md, agent.json, and manifest.json to `user/agents/{name}/`
7. Set `napp_path` on the agent record
8. Process workflow bindings (upsert to agent_workflows, register triggers)
9. Resolve dependency cascade (plugins from `requires.plugins`, then skills)
10. Broadcast `agent_installed` WebSocket event

### Activate (`activate_agent`)

1. Set `is_enabled = true` in DB
2. Parse AgentConfig from frontmatter
3. Build `ActiveAgent` struct and insert into `AgentRegistry` (in-memory `RwLock<HashMap>`)
4. Start `AgentWorker` (registers all triggers)
5. Register agent in NeboLoop personal loop (async, best-effort)
6. Broadcast `agent_activated` WebSocket event

### Deactivate (`deactivate_agent`)

1. Set `is_enabled = false` in DB
2. Stop `AgentWorker` (cancels all triggers, running workflows)
3. Remove from `AgentRegistry`
4. Deregister agent from NeboLoop (async, best-effort)
5. Broadcast `agent_deactivated` WebSocket event

### Delete (`delete_agent`)

1. Stop `AgentWorker`
2. Remove from `AgentRegistry`
3. Unregister all cron triggers
4. Unsubscribe all event triggers from EventDispatcher
5. Delete from `agents` table (cascades to `agent_workflows`)
6. Broadcast `agent_uninstalled` (immediately — before cleanup, so frontend updates fast)
7. Clean up agent-scoped data (best-effort, `let _ =`):
   - `delete_agent_chats(id)` — `DELETE FROM chats WHERE session_name LIKE 'agent:{id}:%'`
   - `delete_agent_sessions(id)` — `DELETE FROM sessions WHERE scope = 'agent' AND scope_id = ?`
   - `delete_agent_memories(id)` — `DELETE FROM memories WHERE user_id LIKE '%:agent:{id}'`
   - `delete_agent_workflow_runs(id)` — `DELETE FROM workflow_runs WHERE workflow_id = 'agent:{id}'`
   - Messages, memory_chunks, and activity_results cascade-delete via FK
   - Order matters: chats before sessions (chats reference session names)
8. Clean up filesystem (napp_path, nebo/agents/, user/agents/)
9. Deregister from NeboLoop (async)

### Update (`update_agent`)

1. Load existing agent from DB
2. Parse AGENT.md frontmatter
3. Body fields take priority over frontmatter (allows renaming without editing AGENT.md)
4. Preserve existing frontmatter workflows
5. Persist to DB
6. Sync in-memory agent_registry if agent is active
7. Broadcast `agent_updated`

### Duplicate (`duplicate_agent`)

1. Load source agent
2. Generate new UUID, append " (Copy)" to name
3. Update frontmatter name in AGENT.md
4. Insert new agent record
5. Copy all agent_workflow bindings from source
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

**Triggers are no longer part of workflow.json** -- they are owned by Agents via
agent.json. Legacy `triggers` fields are silently ignored on parse (via
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
    fn run_inline(def_json, inputs, trigger, agent_id, emit) -> Result<String>;
    fn run_status(run_id) -> Result<WorkflowRunInfo>;
    fn list_runs(workflow_id, limit) -> Vec<WorkflowRunInfo>;
    fn toggle(id) -> Result<bool>;
    fn create(name, definition) -> Result<WorkflowInfo>;
    fn cancel(run_id) -> Result<()>;
    fn cancel_runs_for_agent(agent_id) -> ();
}
```

### WorkflowManagerImpl (`crates/server/src/workflow_manager.rs`)

Concrete implementation. Key internals:

- `active_runs: Mutex<HashMap<String, CancellationToken>>` -- token per running workflow
- `agent_runs: Mutex<HashMap<String, Vec<String>>>` -- agent_id to list of active run_ids
- `event_bus: Option<EventBus>` -- for injecting emit tool
- `skill_loader: Option<Arc<Loader>>` -- for resolving skill content

**run_inline() flow:**
1. Parse definition from JSON string
2. Create `workflow_runs` record with `workflow_id = "persona:{agent_id}"`
3. Store CancellationToken + track in agent_runs
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
| `schedule` | Cron expression (`"0 7 * * 1-5"`) | Creates `cron_jobs` row | Cron scheduler -> `execute_agent_workflow_task()` |
| `heartbeat` | `"30m"` or `"30m\|08:00-18:00"` | AgentWorker spawns `tokio::interval` task | AgentWorker -> `manager.run_inline()` |
| `event` | Comma-separated patterns (`"email.*,cal.changed"`) | EventDispatcher subscription | EventDispatcher -> `manager.run_inline()` |
| `watch` | JSON: `{"plugin","command","event","multiplexed","restart_delay_secs"}` | AgentWorker spawns `watch_loop()` task | AgentWorker -> `watch_loop()` -> auto-emit to EventBus + optional `manager.run_inline()` |
| `manual` | Empty string | No registration | User triggers via REST API |

### Schedule Trigger Registration

```rust
pub fn register_agent_triggers(agent_id: &str, bindings: &[AgentWorkflow], store: &Store) {
    for binding in bindings {
        if binding.trigger_type == "schedule" {
            let name = format!("agent-{}-{}", agent_id, binding.binding_name);
            let command = format!("persona:{}:{}", agent_id, binding.binding_name);
            store.upsert_cron_job(&name, &binding.trigger_config, &command, "agent_workflow", ...);
        }
    }
}
```

### Unregistration

| Function | Scope |
|----------|-------|
| `unregister_single_agent_trigger(agent_id, binding)` | One cron job: `"agent-{id}-{binding}"` |
| `unregister_agent_triggers(agent_id)` | All cron jobs with prefix `"agent-{id}-"` |
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

`last_run` parsing: tries `i64` Unix timestamp, then `NaiveDateTime` format `"%Y-%m-%d %H:%M:%S"`, defaults to epoch 0. Schedule normalization uses `tools::PersonaTool::normalize_cron()` to handle stale 5-field expressions.

### Task Type Dispatch

| task_type | Command format | Execution |
|-----------|----------------|-----------|
| `"bash"` / `"shell"` / `""` | Shell command | `sh -c {command}` subprocess |
| `"agent"` | Prompt text | `runner.run()` with `Origin::System`, session `"cron-{name}"` |
| `"workflow"` | Workflow ID | `manager.run(id, null, "cron")` |
| `"agent_workflow"` | `"persona:{agent_id}:{binding}"` | Parse -> load agent -> `manager.run_inline()` |

### execute_agent_workflow_task() -- **CRITICAL PATH**

```rust
async fn execute_agent_workflow_task(
    manager: &dyn WorkflowManager,
    store: &Store,
    command: &str,  // "persona:{agent_id}:{binding_name}"
) -> (bool, String, Option<String>) {
    let parts = command.splitn(3, ':');  // ["persona", agent_id, binding_name]
    let agent = store.get_agent(agent_id)?;
    let config = napp::agent::parse_agent_config(&agent.frontmatter)?;
    let binding = config.workflows.get(binding_name)?;
    if !binding.has_activities() { return error; }
    let def_json = binding.to_workflow_json(binding_name);
    let emit_source = binding.emit.map(|e| format!("{}.{}", slug, e));
    manager.run_inline(def_json, inputs, "schedule", agent_id, emit_source).await
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
    pub agent_source: String,                // Agent ID
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
        manager.run_inline(def_json, inputs, "event", &sub.agent_source, ...).await;
    }
}
```

**Pattern matching:**
- Exact: `"email.urgent"` matches `"email.urgent"`
- Wildcard suffix: `"email.*"` matches `"email.urgent"`, `"email.info"`, etc.
- No deeper wildcard support (e.g., no `"**"` or `"email.*.done"`)

**Methods:**
- `subscribe(sub)` -- add a subscription
- `unsubscribe_binding(agent_id, binding_name)` -- remove subscriptions for one binding
- `unsubscribe_agent(agent_id)` -- remove all subscriptions for an agent
- `set_subscriptions(subs)` -- replace all subscriptions
- `clear()` -- remove all

---

## 14. AgentWorker Runtime

**File:** `crates/agent/src/agent_worker.rs`

### AgentWorker

```rust
pub struct AgentWorker {
    pub agent_id: String,
    pub name: String,
    cancel: CancellationToken,
    event_dispatcher: Arc<EventDispatcher>,
    workflow_manager: Arc<dyn WorkflowManager>,
}
```

### start()

1. Load agent workflow bindings from DB (`list_agent_workflows`)
2. Load agent config from DB frontmatter (`parse_agent_config`)
3. Register schedule triggers via `register_agent_triggers()` (creates cron_jobs)
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
                mgr.run_inline(def_json, inputs, "heartbeat", &agent_id, emit_source).await;
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
    agent_source: agent_id.clone(),
    binding_name: binding.binding_name.clone(),
    definition_json: def_json.clone(),
    emit_source: event_emit_source.clone(),
}).await;
```

### stop()

```rust
fn stop(&self, store: &Store) {
    self.cancel.cancel();                           // Cancel all spawned tasks
    workflow::triggers::unregister_agent_triggers();  // Remove cron jobs
    mgr.cancel_runs_for_agent(&agent_id);             // Cancel running workflows
    dispatcher.unsubscribe_agent(&agent_id);           // Remove event subscriptions
}
```

### AgentWorkerRegistry

```rust
pub struct AgentWorkerRegistry {
    workers: RwLock<HashMap<String, AgentWorker>>,
    store: Arc<Store>,
    workflow_manager: Arc<dyn WorkflowManager>,
    event_dispatcher: Arc<EventDispatcher>,
}
```

- `start_agent(id, name)` -- stops existing worker first (clean re-registration), then starts new
- `stop_agent(id)` -- stops and removes worker
- `stop_all()` -- stops all workers (shutdown)

---

## 15. Workflow Binding Processing

**File:** `crates/server/src/handlers/agents.rs:594-713`

### process_agent_bindings()

Called during agent creation and reload. For each binding in `AgentConfig.workflows`:

1. Determine `trigger_type` and `trigger_config` from `AgentTrigger` enum
2. Serialize `inputs` and `activities` to JSON
3. Upsert to `agent_workflows` table
4. After all bindings: `register_agent_triggers()` for schedule triggers
5. Build and register `EventSubscription`s for event triggers

### Dual Storage

Workflow bindings are stored in two places:
1. **`agents.frontmatter`** -- the full agent.json as a JSON string (source of truth for the definition)
2. **`agent_workflows` table** -- tracking rows for trigger registration, is_active state, last_fired

Both are updated on create/update/delete. The `frontmatter` is also written to
the filesystem as `agent.json` via `write_agent_json_to_fs()`.

### CRUD Handlers

**create_agent_workflow:** Checks for conflict (binding exists), builds trigger
JSON, inserts into frontmatter, upserts tracking row, registers triggers,
writes to filesystem.

**update_agent_workflow:** Merges provided fields over existing binding, handles
trigger type changes (unregister old, register new), updates both frontmatter
and tracking row.

**delete_agent_workflow:** Removes from frontmatter, deletes tracking row,
unregisters triggers (cron + event), writes to filesystem.

---

## 16. Toggle / Enable / Disable

### Agent Toggle (`toggle_agent`)

```rust
store.toggle_agent(&id);  // Flip is_enabled
if agent.is_enabled != 0 {
    state.agent_workers.start_agent(&id, &agent.name).await;
} else {
    state.agent_workers.stop_agent(&id).await;
}
```

### Workflow Binding Toggle (`toggle_agent_workflow`)

```rust
let is_active = store.toggle_agent_workflow(&id, &binding_name);
if is_active {
    register_binding_triggers(&id, &binding_name, ...).await;
} else {
    workflow::triggers::unregister_single_agent_trigger(&id, &binding_name, &store);
    state.event_dispatcher.unsubscribe_binding(&id, &binding_name).await;
}
```

**Toggle -> OFF:**
- Schedule: deletes cron_job named `"agent-{id}-{binding}"`
- Event: removes subscription from EventDispatcher
- Heartbeat: **NOT directly stopped** -- the AgentWorker's heartbeat tokio task
  is not individually cancellable. It continues running until the entire
  AgentWorker is stopped.

**Toggle -> ON:**
- Re-registers triggers (cron job and/or event subscription)

---

## 17. End-to-End Flows

### Full Agent Install Flow (Code → Running Agent)

The complete install flow when a user submits an `AGNT-XXXX-XXXX` code.

**Phase 1 — Code Detection & Entry**

1. User pastes `AGNT-XXXX-XXXX` into chat
2. `codes.rs` `detect_code()` matches `AGNT-` prefix → `CodeType::Agent`
3. `handle_code()` broadcasts `"code_processing"` WS event ("Installing agent...")
4. Dispatches to `handle_agent_code()`

**Phase 2 — Redeem & Download**

5. Builds NeboLoop API client with auth tokens
6. Redeems install code: `api.install_agent(code)` → gets `artifact_id`, `artifact_name`
   - If already redeemed: falls back to `api.list_products()` lookup by code
   - If `payment_required`: returns checkout URL to user, stop
7. **Reinstall check:** If agent already exists in DB:
   - Stops agent worker, removes from registry
   - Unregisters triggers, unsubscribes events
   - Deletes DB rows (`agent_workflows`, `agents`) + filesystem
   - Deregisters from NeboLoop
8. `persist_agent_from_api()`:
   a. Fetches metadata from NeboLoop (`content_md`, `type_config`, `download_url`)
   b. Creates/updates `agents` DB row (AGENT.md, frontmatter, description)
   c. Downloads `.napp` → saves to `<nebo_dir>/agents/<slug>/<version>/<version>.napp`
   d. `extract_napp_alongside()` removes existing dir if present, extracts tar.gz
   e. Validates: must contain `AGENT.md` + `agent.json` (fails fast if missing)

**Phase 3 — Dependency Cascade (BEFORE activation)**

9. Extracts frontmatter from `type_config` (or DB fallback)
10. `extract_agent_deps_from_frontmatter()` parses:
    - `requires.plugins[]` → `DepType::Plugin` (install codes like `PLUG-XXXX-XXXX`)
    - `skills[]` → `DepType::Skill`
    - Inline activity skill references → `DepType::Skill`
11. `resolve_cascade()` for each dep (with visited-set dedup):
    a. Check if already installed (filesystem for skills/plugins, DB for workflows)
    b. **Plugins:** `plugin_store.ensure()` → download from NeboLoop → SHA256 + ED25519 verify → store at `<data_dir>/nebo/plugins/<slug>/<version>/`
    c. **Skills:** redeem code → `persist_skill_from_api()` → reload skill loader → extract child deps (including skill's own `plugins:` frontmatter) and recurse
    d. Broadcasts `"dep_installed"` per dep
12. Broadcasts `"dep_cascade_complete"` with install/skip/fail counts

**Phase 4 — Workflow Binding Processing**

13. Parses `agent.json` → `AgentConfig` (via `parse_agent_config()`)
14. `process_agent_bindings()` for each `WorkflowBinding`:
    a. Extracts trigger type: `Schedule`/`Heartbeat`/`Event`/`Watch`/`Manual`
    b. Upserts to `agent_workflows` table (binding_name, trigger_type, trigger_config, activities, inputs, emit)
    c. Registers triggers: cron jobs, interval timers, event subscriptions

**Phase 5 — Agent Activation**

15. Fetches fresh agent from DB
16. Creates `ActiveAgent` struct, inserts into `agent_registry` (`RwLock<HashMap>`)
17. Broadcasts `"agent_activated"` WS event
18. `AgentWorkerRegistry::start_agent()`:
    a. Loads workflow bindings from DB
    b. Spawns trigger tasks per binding:
       - **Schedule:** cron job registered with scheduler
       - **Heartbeat:** `tokio::spawn` interval loop (with optional time window)
       - **Event:** `EventSubscription` registered with `EventDispatcher`
       - **Watch:** spawns plugin process, parses NDJSON output, auto-emits to EventBus
    c. Inserts worker into registry

**Phase 6 — NeboLoop Registration (async, non-blocking)**

19. `tokio::spawn` registers agent in user's personal loop
    - Deregisters first (prevents 409 on reinstall)
    - Then registers: `api.register_agent(&loop_id, name, slug, None)`
    - Failure logged but doesn't block install

**Phase 7 — Completion**

20. Returns `CodeHandlerResult { message: "Installed agent: {name}", artifact_id, artifact_name }`
21. Broadcasts `"code_result"` (success, artifact_name, artifact_id) + `"chat_complete"`

**WebSocket Event Sequence:**
```
code_processing → [dep_installed × N] → dep_cascade_complete → code_result → agent_activated → chat_complete
```

**Filesystem Created:**
```
<nebo_dir>/agents/<slug>/
  <version>.napp                # sealed archive
  <version>/
    AGENT.md                    # persona description
    agent.json                  # config, triggers, deps
    manifest.json               # package metadata

<data_dir>/nebo/plugins/<slug>/<version>/   # (if requires.plugins present)
  plugin.json                   # manifest
  <binary>                      # platform executable
```

**Key Source References:**
- `crates/server/src/codes.rs:318-494` — `handle_agent_code()`
- `crates/tools/src/lib.rs:233-342` — `persist_agent_from_api()`
- `crates/server/src/deps.rs:64-196` — `resolve_cascade()`
- `crates/server/src/handlers/agents.rs:639-769` — `process_agent_bindings()`
- `crates/agent/src/agent_worker.rs:34-80` — `AgentWorker::start()`

---

### User Creates Schedule Automation

1. **Frontend:** Automate tab -> "New Automation"
2. AutomationEditor: name "Morning Briefing", steps, schedule 7:30 AM weekdays
3. **API:** `POST /agents/{id}/workflows` with bindingName, triggerType, triggerConfig, activities
4. **Handler:** Builds trigger JSON, inserts into frontmatter, upserts agent_workflows row
5. **Trigger registration:** Creates cron_job named `"agent-{agent_id}-morning-briefing"`
6. **Scheduler tick (60s):** Finds cron job due -> `execute_agent_workflow_task()`
7. Parses `"persona:{agent_id}:morning-briefing"` -> loads agent config -> `run_inline()`
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
2. **API:** `POST /agents/{id}/workflows/{binding}/toggle`
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

When a user toggles an agent_workflow binding to `is_active = 0`:
- The toggle handler **correctly** deletes the cron_job (unregisters the schedule trigger)
- However, if the cron_job somehow still exists (e.g., re-registered on agent worker restart),
  `execute_agent_workflow_task()` in `scheduler.rs` **does NOT check `agent_workflows.is_active`**

The execution path in `scheduler.rs:209-253`:
```rust
async fn execute_agent_workflow_task(manager, store, command) {
    let parts = command.splitn(3, ':');
    let agent = store.get_agent(agent_id)?;
    let config = napp::agent::parse_agent_config(&agent.frontmatter)?;
    let binding = config.workflows.get(binding_name)?;
    // ** NO CHECK: does not query agent_workflows.is_active **
    manager.run_inline(def_json, inputs, "schedule", agent_id, ...).await
}
```

The agent config comes from `agents.frontmatter` (which is the static agent.json
definition) -- it has no concept of `is_active`. The `is_active` flag only
exists in the `agent_workflows` tracking table.

**Event triggers DO check correctly** via `list_active_event_triggers()` which
has `WHERE rw.is_active = 1 AND r.is_enabled = 1`.

**Heartbeat triggers** have a partial mitigation: they run in the AgentWorker's
tokio task, which is started on agent activation and stopped on deactivation.
However, individual heartbeat bindings cannot be toggled without restarting the
entire worker.

**Fix:** Add an `is_active` check in `execute_agent_workflow_task()`:
```rust
// After loading agent and config, check tracking table:
let bindings = store.list_agent_workflows(agent_id)?;
let is_active = bindings.iter()
    .find(|b| b.binding_name == binding_name)
    .map(|b| b.is_active != 0)
    .unwrap_or(false);
if !is_active { return (true, "skipped: binding inactive".into(), None); }
```

### Heartbeat Toggle Granularity

Individual heartbeat bindings cannot be toggled without stopping the entire
AgentWorker. The toggle handler deletes the cron_job (which is irrelevant for
heartbeats) and unsubscribes events (also irrelevant). The tokio::interval task
spawned by the worker continues until the worker is stopped.

**Impact:** Toggling a heartbeat binding to OFF has no effect until the agent
itself is deactivated and reactivated.

### Dual Storage Drift

Workflow binding definitions exist in two places:
1. `agents.frontmatter` (JSON string of agent.json)
2. `agent_workflows` table rows

The CRUD handlers update both, but there is no reconciliation mechanism. If one
is modified outside the handler (e.g., direct DB edit, filesystem edit without
reload), they can drift out of sync. The `reload_agent` endpoint addresses
filesystem drift but not direct DB edits.

### Event Channel Unbounded

`EventBus` uses `mpsc::unbounded_channel()` -- no backpressure. In a scenario
where workflow chains produce events faster than they are consumed, memory could
grow unboundedly.

### Scheduler Single-Tick Grid

All cron jobs share a 60s tick. Jobs due within the same minute all fire in one
batch. No per-job scheduling granularity below 1 minute.

### AgentWorker Heartbeat Overlaps with Server Heartbeat

Two independent heartbeat mechanisms exist:
1. **`heartbeat.rs`** (server-level) -- entity_config-driven, uses `run_chat()` on HEARTBEAT lane
2. **`agent_worker.rs`** (agent-level) -- from agent_workflows with trigger_type "heartbeat", uses `run_inline()`

These are different systems serving different purposes. The server heartbeat is
a general "wake up and check" mechanism. The agent worker heartbeat runs specific
inline workflow activities. They can overlap if both are configured for the same agent.

### NotifyOwner Fallback is Stubbed

`Fallback::NotifyOwner` behaves identically to `Fallback::Abort` -- the
notification mechanism is not implemented. Both abort the workflow on activity failure.

---

## 20. Key Files Reference

### Core Agent System

| File | Lines | Description |
|------|-------|-------------|
| `crates/server/src/handlers/agents.rs` | ~1754 | All agent HTTP handlers + workflow binding CRUD |
| `crates/napp/src/agent.rs` | ~665 | AgentConfig, WorkflowBinding, AgentTrigger, parse_agent_config() |
| `crates/napp/src/agent_loader.rs` | ~141 | Filesystem scanner (LoadedAgent, scan_installed/user_agents) |
| `crates/db/src/queries/agents.rs` | ~353 | All agent + agent_workflow DB queries |
| `crates/db/src/models.rs:570-647` | ~78 | Agent, AgentWorkflow, EmitSource, AgentWorkflowStats structs |
| `crates/tools/src/persona_tool.rs:14-29` | ~16 | ActiveAgent struct, AgentRegistry type alias |

### Workflow Engine

| File | Lines | Description |
|------|-------|-------------|
| `crates/workflow/src/lib.rs` | ~52 | WorkflowError enum, module exports |
| `crates/workflow/src/parser.rs` | ~226 | WorkflowDef, Activity, parse_workflow(), validation |
| `crates/workflow/src/engine.rs` | ~603 | execute_workflow(), execute_activity(), build_activity_prompt() |
| `crates/workflow/src/loader.rs` | ~110 | Filesystem loader (LoadedWorkflow, scan directories) |
| `crates/workflow/src/triggers.rs` | ~125 | register/unregister schedule/agent triggers |
| `crates/workflow/src/events.rs` | ~174 | EventDispatcher, EventSubscription, source_matches() |
| `crates/server/src/workflow_manager.rs` | ~400+ | WorkflowManagerImpl (trait implementation) |
| `crates/tools/src/workflows/manager.rs` | ~93 | WorkflowManager trait definition |

### Scheduling & Runtime

| File | Lines | Description |
|------|-------|-------------|
| `crates/server/src/scheduler.rs` | ~254 | Cron scheduler (tick loop, task dispatch, agent_workflow execution) |
| `crates/agent/src/agent_worker.rs` | ~415 | AgentWorker, AgentWorkerRegistry, heartbeat/event trigger tasks |
| `crates/db/src/queries/cron_jobs.rs` | ~296 | All cron_job + cron_history DB queries |
| `crates/db/src/queries/workflows.rs` | ~508 | Workflow, run, activity_result queries + stats |

### Database Migrations

| Migration | Description |
|-----------|-------------|
| `0050_workflows.sql` | Initial: workflows, workflow_runs, workflow_activity_results, agents |
| `0051_workflows_to_fs.sql` | Add napp_path to workflows + agents |
| `0053_agent_workflows.sql` | Create agent_workflows table (binding_name, trigger, is_active) |
| `0056_agent_workflow_last_fired.sql` | Add last_fired column |
| `0058_agent_instances.sql` | Rename code->kind (not UNIQUE), add workflow_ref/workflow_id |
| `0059_agent_workflow_emit.sql` | Add emit column |
| `0060_agent_workflow_activities.sql` | Add activities column |
| `0061_workflow_runs_drop_fk.sql` | Drop FK on workflow_runs.workflow_id (support inline runs) |
| `0062_workflow_run_output.sql` | Add output column to workflow_runs |
| `0064_agent_input_values.sql` | Add input_values column to agents |
| `0015_cron_jobs.sql` | Initial: cron_jobs + cron_history |
| `0042_cron_skill.sql` | Add instructions column to cron_jobs |
| `0057_entity_config.sql` | Create entity_config table |

---

*Last updated: 2026-04-05*
