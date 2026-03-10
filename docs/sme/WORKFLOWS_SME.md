# Workflow System — Rust SME Reference

> Definitive reference for the Nebo Rust workflow system. Covers the workflow definition
> format, execution engine, activity loop, trigger registration, event dispatch, role
> system, database schema, HTTP endpoints, tool resolution, token budgets, cancellation,
> filesystem storage, and the full request-to-result data flow.

**Canonical spec:** [platform-taxonomy.md](platform-taxonomy.md)

---

## Table of Contents

1. [System Overview](#1-system-overview)
2. [Architecture Layers](#2-architecture-layers)
3. [Workflow Definition (workflow.json)](#3-workflow-definition-workflowjson)
4. [Role Configuration (role.json)](#4-role-configuration-rolejson)
5. [Execution Engine](#5-execution-engine)
6. [Activity Execution Loop](#6-activity-execution-loop)
7. [Trigger Registration](#7-trigger-registration)
8. [Event Dispatch System](#8-event-dispatch-system)
9. [Database Schema](#9-database-schema)
10. [Database Queries](#10-database-queries)
11. [HTTP Endpoints](#11-http-endpoints)
12. [WorkflowManager Implementation](#12-workflowmanager-implementation)
13. [Tool Resolution](#13-tool-resolution)
14. [Error Handling & Budgets](#14-error-handling--budgets)
15. [Filesystem & Package Storage](#15-filesystem--package-storage)
16. [Workflow Loader](#16-workflow-loader)
17. [Role Loader](#17-role-loader)
18. [Integration Points](#18-integration-points)
19. [Constants & Defaults](#19-constants--defaults)
20. [Cross-Reference to Go Docs](#20-cross-reference-to-go-docs)

---

## 1. System Overview

The workflow system is a three-layer procedure execution engine:

```
ROLE (schedule of intent)
  └─ defines WHEN to run
    └─ WORKFLOW (procedure)
      └─ defines WHAT to do
        └─ ACTIVITY (LLM-guided task)
          └─ executes with tools + skills
```

**Key properties:**
- Workflows are **pure procedure definitions** (stored in `workflow.json`)
- Roles **own triggers** (cron, heartbeat, event, manual) via `role.json` + `role_workflows` DB table
- Activities are **LLM-guided tool-use loops** with retry + fallback policies
- Skill content is **injected into activity prompts** for domain knowledge
- Execution is **background async** via `tokio::spawn` with cancellation tokens
- Events can **trigger workflows** via pattern-matching subscriptions

> **Key principle:** The workflow does not decide when it runs. The Role does.

---

## 2. Architecture Layers

### Parsing Layer

**Source:** `crates/workflow/src/parser.rs`, `crates/workflow/src/loader.rs`

Deserialization, validation, and in-memory representation.

```rust
pub fn parse_workflow(json_str: &str) -> Result<WorkflowDef, WorkflowError>
pub fn validate_workflow(def: &WorkflowDef) -> Result<(), WorkflowError>
```

### Engine Layer

**Source:** `crates/workflow/src/engine.rs`

Core execution logic with activity streaming, tool calling, and token tracking.

```rust
pub async fn execute_workflow(
    def: &WorkflowDef,
    inputs: serde_json::Value,
    trigger_type: &str,
    trigger_detail: Option<&str>,
    store: &Arc<Store>,
    provider: &dyn ai::Provider,
    resolved_tools: &[Box<dyn DynTool>],
    existing_run_id: Option<&str>,
    cancel_token: Option<&CancellationToken>,
    skill_content: Option<&HashMap<String, String>>,
    event_bus: Option<&tools::EventBus>,
) -> Result<String, WorkflowError>
```

### Trigger Layer

**Source:** `crates/workflow/src/triggers.rs`

Registration of cron jobs and event subscriptions.

### Event Dispatch Layer

**Source:** `crates/workflow/src/events.rs`

Pattern matching and workflow triggering on events.

### Manager Layer

**Source:** `crates/server/src/workflow_manager.rs`

WorkflowManager trait implementation — lifecycle + execution orchestration.

### Handler Layer

**Source:** `crates/server/src/handlers/workflows.rs`, `crates/server/src/handlers/roles.rs`

REST API endpoints for workflows and roles.

---

## 3. Workflow Definition (workflow.json)

**Source:** `crates/workflow/src/parser.rs`

### WorkflowDef

```rust
pub struct WorkflowDef {
    pub version: String,                          // "1.0"
    pub id: String,                               // "lead-qualification"
    pub name: String,                             // "Lead Qualification Workflow"
    pub inputs: HashMap<String, InputParam>,      // Declared inputs
    pub activities: Vec<Activity>,                // Sequential activity chain
    pub dependencies: Dependencies,               // Required skills/workflows
    pub budget: Budget,                           // Token budget for entire run
}
```

### InputParam

```rust
pub struct InputParam {
    pub param_type: String,                       // "string", "number", "object"
    pub required: bool,
    pub default: Option<serde_json::Value>,
}
```

### Activity

```rust
pub struct Activity {
    pub id: String,                               // "qualify-lead"
    pub intent: String,                           // "Assess if lead is qualified"
    pub skills: Vec<String>,                      // Qualified skill names for this activity
    pub model: String,                            // Model override (e.g., "sonnet")
    pub steps: Vec<String>,                       // Procedural hints
    pub token_budget: TokenBudget,                // Max tokens for this activity
    pub on_error: OnError,                        // Retry + fallback policy
}
```

### TokenBudget

```rust
pub struct TokenBudget {
    pub max: u32,                                 // Default: 4096
}
```

### OnError

```rust
pub struct OnError {
    pub retry: u32,                               // Default: 1
    pub fallback: Fallback,                       // Default: NotifyOwner
}

pub enum Fallback {
    NotifyOwner,                                  // Log + continue (default)
    Skip,                                         // Skip activity, continue workflow
    Abort,                                        // Stop entire workflow
}
```

### Dependencies

```rust
pub struct Dependencies {
    pub skills: Vec<String>,                      // Required skill codes
    pub workflows: Vec<String>,                   // Required workflow codes
}
```

### Budget

```rust
pub struct Budget {
    pub total_per_run: u32,                       // Total tokens for entire workflow (0 = unlimited)
    pub cost_estimate: String,                    // Human-readable estimate
}
```

### Example workflow.json

```json
{
  "version": "1.0",
  "id": "lead-qualification",
  "name": "Lead Qualification Workflow",
  "inputs": {
    "lead_id": { "type": "string", "required": true },
    "threshold": { "type": "number", "default": 50 }
  },
  "activities": [
    {
      "id": "research",
      "intent": "Research the lead's company and recent activity",
      "skills": ["@acme/skills/crm-lookup@^1.0.0"],
      "model": "sonnet",
      "steps": [
        "Look up the lead in the CRM",
        "Search for recent company news",
        "Check LinkedIn for decision-maker info"
      ],
      "token_budget": { "max": 4096 },
      "on_error": { "retry": 2, "fallback": "skip" }
    },
    {
      "id": "score",
      "intent": "Score the lead based on research findings",
      "skills": ["@acme/skills/lead-scoring@^1.0.0"],
      "steps": ["Apply scoring criteria", "Generate qualification report"],
      "token_budget": { "max": 2048 },
      "on_error": { "retry": 1, "fallback": "abort" }
    }
  ],
  "dependencies": {
    "skills": ["@acme/skills/crm-lookup@^1.0.0", "@acme/skills/lead-scoring@^1.0.0"]
  },
  "budget": {
    "total_per_run": 10000,
    "cost_estimate": "$0.03 per run"
  }
}
```

---

## 4. Role Configuration (role.json)

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
    pub workflow_ref: String,                     // "@nebo/workflows/daily-briefing@^1.0.0"
    pub trigger: RoleTrigger,
    pub description: String,
    pub inputs: HashMap<String, serde_json::Value>,
}
```

### RoleTrigger

```rust
pub enum RoleTrigger {
    Schedule { cron: String },                    // "0 7 * * *"
    Heartbeat {
        interval: String,                         // "30m"
        window: Option<String>,                   // "08:00-18:00"
    },
    Event { sources: Vec<String> },               // ["email.urgent", "calendar.changed"]
    Manual,
}
```

### RolePricing / RoleDefaults

```rust
pub struct RolePricing {
    pub model: String,
    pub cost: f64,
}

pub struct RoleDefaults {
    pub timezone: String,
    pub configurable: Vec<String>,                // JSON paths user can override
}
```

### ROLE.md

Pure prose — the agent's conversational persona when in this role. Parsed by `parse_role()` which handles both frontmatter (legacy) and pure-markdown formats.

```rust
pub struct RoleDef {
    pub id: String,
    pub name: String,
    pub description: String,
    pub body: String,                             // Markdown body
}
```

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
    "lead-scorer": {
      "ref": "WORK-XXXX-XXXX",
      "trigger": { "type": "event", "sources": ["crm.lead.created"] },
      "inputs": { "threshold": 50 }
    }
  },
  "skills": ["@nebo/skills/crm-lookup@^1.0.0"],
  "pricing": { "model": "monthly_fixed", "cost": 47.0 }
}
```

---

## 5. Execution Engine

**Source:** `crates/workflow/src/engine.rs`

### execute_workflow()

The top-level function that orchestrates a full workflow run.

```rust
pub async fn execute_workflow(
    def: &WorkflowDef,
    inputs: serde_json::Value,
    trigger_type: &str,
    trigger_detail: Option<&str>,
    store: &Arc<Store>,
    provider: &dyn ai::Provider,
    resolved_tools: &[Box<dyn DynTool>],
    existing_run_id: Option<&str>,
    cancel_token: Option<&CancellationToken>,
    skill_content: Option<&HashMap<String, String>>,
    event_bus: Option<&tools::EventBus>,
) -> Result<String, WorkflowError>
```

### Algorithm

```
1. Create WorkflowRun record (or use existing_run_id)
   - session_key = "workflow-{workflow_id}-{run_id}"

2. FOR EACH activity in def.activities (sequential):
   a. Check cancellation token → return Cancelled if cancelled
   b. Update run: current_activity = activity.id
   c. Record activity start time
   d. RETRY LOOP (attempts = 1..=max_attempts):
      i.  Execute activity (see §6)
      ii. On success: break retry loop
      iii. On failure: if more retries, continue; else apply fallback
   e. Record activity_result in DB (status, tokens, attempts, error)
   f. Add tokens to running total
   g. Check workflow budget (budget.total_per_run)
      - If exceeded: return BudgetExceeded error

3. Complete workflow run
   - status = "completed"
   - total_tokens_used = sum of all activities
   - Emit system event if event_bus available

4. Return run_id
```

### Activity Prompt Building

```rust
fn build_activity_prompt(
    activity: &Activity,
    prior_context: String,
    inputs: String,
    skill_content: Option<&HashMap<String, String>>,
) -> String
```

The prompt is assembled in this order:

1. **Skills section** — SKILL.md bodies for skills listed in `activity.skills`
2. **Task** — `activity.intent`
3. **Steps** — numbered list from `activity.steps`
4. **Inputs** — serialized JSON
5. **Prior results** — output from previous activities

---

## 6. Activity Execution Loop

**Source:** `crates/workflow/src/engine.rs`

### execute_activity()

```rust
pub async fn execute_activity(
    activity: &Activity,
    prior_context: &str,
    inputs: &serde_json::Value,
    provider: &dyn ai::Provider,
    tools: &[&Box<dyn DynTool>],
    skill_content: Option<&HashMap<String, String>>,
) -> Result<(String, u32), WorkflowError>
```

Returns `(response_text, tokens_used)`.

### Loop (max 20 iterations)

```
1. Check token budget (activity.token_budget.max)
   - If tokens_used >= max → return BudgetExceeded

2. Build ChatRequest:
   - messages: accumulated history
   - tools: activity tool definitions
   - max_tokens: remaining budget
   - system: full activity prompt
   - model: activity.model (or inherited)

3. Stream from provider:
   - StreamEventType::Text → accumulate response_text
   - StreamEventType::ToolCall → collect tool_calls
   - StreamEventType::Done → update tokens_used
   - StreamEventType::Error → return Provider error

4. If NO tool_calls:
   - Return (response_text, tokens_used)

5. If tool_calls present:
   - Append assistant message with tool_calls
   - FOR EACH tool_call:
     * Find matching tool from tools array
     * Execute: tool.execute_dyn(&ctx, tool_call.input)
     * Collect ToolResult
   - Append tool results message
   - Continue loop

6. After MAX_ITERATIONS (20):
   - Return MaxIterations error
```

### Key Differences from Agent Loop

| Aspect | Agent Loop | Activity Loop |
|---|---|---|
| **Context** | Full (identity, steering, memory, advisors) | Lean (intent + steps + skills only) |
| **Tools** | All registered (filtered by context) | Only declared activity tools |
| **Token limit** | Soft (context window) | Hard (token_budget.max) |
| **Memory** | Injected, extracted, refreshed | None — append-only within run |
| **Temperature** | Configurable | 0.0 (deterministic) |
| **Max iterations** | 100 | 20 |
| **Cost predictability** | Unpredictable | Deterministic per-run |

---

## 7. Trigger Registration

**Source:** `crates/workflow/src/triggers.rs`

### Schedule Triggers (Cron)

```rust
pub fn register_schedule_trigger(workflow_id: &str, cron: &str, store: &Store) {
    let name = format!("workflow-{}", workflow_id);
    store.upsert_cron_job(&name, cron, workflow_id, "workflow", ...);
}
```

When the scheduler fires a cron job with `task_type = "workflow"`:
- `command` field contains the `workflow_id`
- Calls `workflow_manager.run(workflow_id, {}, "schedule")`

### Role-Based Triggers

```rust
pub fn register_role_triggers(
    role_id: &str,
    bindings: &[db::models::RoleWorkflow],
    store: &Store,
)
```

For each binding:
- **Schedule:** Create cron job named `role-{role_id}-{binding_name}`
- **Event:** Stored in `role_workflows` table, consumed by EventDispatcher
- **Heartbeat/Manual:** Stored but not yet executed (future enhancement)

### Unregistration

```rust
pub fn unregister_role_triggers(role_id: &str, store: &Store) {
    let prefix = format!("role-{}-", role_id);
    store.delete_cron_jobs_by_prefix(&prefix);
}

pub fn unregister_triggers(workflow_id: &str, store: &Store) {
    let name = format!("workflow-{}", workflow_id);
    store.delete_cron_job_by_name(&name);
}
```

---

## 8. Event Dispatch System

**Source:** `crates/workflow/src/events.rs`, `crates/tools/src/events.rs`

### EventSubscription

```rust
pub struct EventSubscription {
    pub pattern: String,              // "email.urgent" or "email.*"
    pub workflow_id: String,
    pub default_inputs: serde_json::Value,
    pub role_source: String,
    pub binding_name: String,
}
```

### EventDispatcher

```rust
pub struct EventDispatcher {
    subscriptions: Arc<RwLock<Vec<EventSubscription>>>,
}
```

### Pattern Matching

```rust
fn source_matches(pattern: &str, source: &str) -> bool {
    if pattern == source { return true; }
    if let Some(prefix) = pattern.strip_suffix(".*") {
        return source.starts_with(prefix) && source[prefix.len()..].starts_with('.');
    }
    false
}
```

- **Exact match:** `"email.urgent"` matches `"email.urgent"` only
- **Wildcard suffix:** `"email.*"` matches `"email.urgent"`, `"email.info"`, etc.

### Event Flow

```
1. Activity calls emit tool:
   emit({ source: "email.customer-service", payload: {...} })

2. EventBus.emit() → unbounded_channel.send()

3. EventDispatcher loop receives event:
   ├─ match_event(event) → find matching subscriptions
   └─ For each match:
      ├─ Merge event into default_inputs:
      │  {
      │    "_event_source": "email.customer-service",
      │    "_event_payload": { ... },
      │    "_event_origin": "...",
      │    ...default_inputs
      │  }
      └─ manager.run(workflow_id, merged_inputs, "event")
```

### Dispatcher Lifecycle

```rust
pub fn spawn(
    self: Arc<Self>,
    rx: tokio::sync::mpsc::UnboundedReceiver<Event>,
    manager: Arc<dyn WorkflowManager>,
) -> tokio::task::JoinHandle<()>
```

Spawns a background task that listens on the receiver and triggers workflows for each matching event.

---

## 9. Database Schema

### workflows

```sql
CREATE TABLE IF NOT EXISTS workflows (
    id TEXT PRIMARY KEY,
    code TEXT UNIQUE,                     -- WORK-XXXX-XXXX
    name TEXT NOT NULL,
    version TEXT NOT NULL DEFAULT '1.0',
    definition TEXT NOT NULL,             -- workflow.json content
    skill_md TEXT,                        -- WORKFLOW.md content
    manifest TEXT,                        -- manifest.json for marketplace
    is_enabled INTEGER NOT NULL DEFAULT 1,
    installed_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    napp_path TEXT                        -- Path to .napp or directory (migration 0051)
);
```

### workflow_tool_bindings

```sql
CREATE TABLE IF NOT EXISTS workflow_tool_bindings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    interface_name TEXT NOT NULL,
    tool_code TEXT NOT NULL,
    UNIQUE(workflow_id, interface_name)
);
```

### workflow_runs

```sql
CREATE TABLE IF NOT EXISTS workflow_runs (
    id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    trigger_type TEXT NOT NULL,            -- "manual", "schedule", "event"
    trigger_detail TEXT,
    status TEXT NOT NULL DEFAULT 'running',
    inputs TEXT,
    current_activity TEXT,
    total_tokens_used INTEGER DEFAULT 0,
    error TEXT,
    error_activity TEXT,
    session_key TEXT,                      -- "workflow-{id}-{run_id}"
    started_at INTEGER NOT NULL DEFAULT (unixepoch()),
    completed_at INTEGER
);
```

### workflow_activity_results

```sql
CREATE TABLE IF NOT EXISTS workflow_activity_results (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
    activity_id TEXT NOT NULL,
    status TEXT NOT NULL,                  -- "completed", "failed"
    tokens_used INTEGER DEFAULT 0,
    attempts INTEGER DEFAULT 1,
    error TEXT,
    started_at INTEGER NOT NULL,
    completed_at INTEGER
);
```

### roles

```sql
CREATE TABLE IF NOT EXISTS roles (
    id TEXT PRIMARY KEY,
    code TEXT UNIQUE,                     -- ROLE-XXXX-XXXX
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    role_md TEXT NOT NULL,                -- ROLE.md content
    frontmatter TEXT NOT NULL,            -- Metadata JSON
    pricing_model TEXT,
    pricing_cost REAL,
    is_enabled INTEGER NOT NULL DEFAULT 1,
    installed_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    napp_path TEXT                        -- Path to .napp or directory (migration 0051)
);
```

### role_workflows (migration 0053)

```sql
CREATE TABLE IF NOT EXISTS role_workflows (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    role_id TEXT NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    binding_name TEXT NOT NULL,
    workflow_ref TEXT NOT NULL,            -- WORK-XXXX-XXXX or @org/workflows/name
    workflow_id TEXT,                      -- Resolved local ID
    trigger_type TEXT NOT NULL,            -- "schedule", "event", "heartbeat", "manual"
    trigger_config TEXT NOT NULL,          -- Cron expression, interval, or sources CSV
    description TEXT,
    inputs TEXT,                          -- Default inputs JSON
    is_active INTEGER NOT NULL DEFAULT 1,
    UNIQUE(role_id, binding_name)
);
```

### Rust Models

**Source:** `crates/db/src/models.rs`

```rust
pub struct Workflow {
    pub id: String,
    pub code: Option<String>,
    pub name: String,
    pub version: String,
    pub definition: String,
    pub skill_md: Option<String>,
    pub manifest: Option<String>,
    pub is_enabled: i64,
    pub installed_at: i64,
    pub updated_at: i64,
    pub napp_path: Option<String>,
}

pub struct WorkflowRun {
    pub id: String,
    pub workflow_id: String,
    pub trigger_type: String,
    pub trigger_detail: Option<String>,
    pub status: String,
    pub inputs: Option<String>,
    pub current_activity: Option<String>,
    pub total_tokens_used: Option<i64>,
    pub error: Option<String>,
    pub error_activity: Option<String>,
    pub session_key: Option<String>,
    pub started_at: i64,
    pub completed_at: Option<i64>,
}

pub struct WorkflowActivityResult {
    pub id: i64,
    pub run_id: String,
    pub activity_id: String,
    pub status: String,
    pub tokens_used: Option<i64>,
    pub attempts: Option<i64>,
    pub error: Option<String>,
    pub started_at: i64,
    pub completed_at: Option<i64>,
}

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

pub struct WorkflowToolBinding {
    pub id: i64,
    pub workflow_id: String,
    pub interface_name: String,
    pub tool_code: String,
}
```

---

## 10. Database Queries

**Source:** `crates/db/src/queries/workflows.rs`, `crates/db/src/queries/roles.rs`

### Workflow Queries

```rust
// CRUD
pub fn list_workflows(&self, limit: i64, offset: i64) -> Result<Vec<Workflow>>
pub fn count_workflows(&self) -> Result<i64>
pub fn get_workflow(&self, id: &str) -> Result<Option<Workflow>>
pub fn get_workflow_by_code(&self, code: &str) -> Result<Option<Workflow>>
pub fn create_workflow(&self, id, code, name, version, definition, skill_md, manifest) -> Result<Workflow>
pub fn update_workflow(&self, id, name, version, definition, skill_md, manifest) -> Result<()>
pub fn delete_workflow(&self, id: &str) -> Result<()>
pub fn set_workflow_napp_path(&self, id: &str, napp_path: &str) -> Result<()>
pub fn toggle_workflow(&self, id: &str) -> Result<()>

// Bindings
pub fn list_workflow_bindings(&self, workflow_id: &str) -> Result<Vec<WorkflowToolBinding>>
pub fn upsert_workflow_binding(&self, workflow_id, interface_name, tool_code) -> Result<()>
pub fn delete_workflow_bindings(&self, workflow_id: &str) -> Result<()>

// Runs
pub fn create_workflow_run(&self, id, workflow_id, trigger_type, trigger_detail, inputs, session_key) -> Result<WorkflowRun>
pub fn update_workflow_run(&self, id, status, current_activity, total_tokens_used, error, error_activity) -> Result<()>
pub fn complete_workflow_run(&self, id, status, total_tokens_used, error, error_activity) -> Result<()>
pub fn list_workflow_runs(&self, workflow_id, limit, offset) -> Result<Vec<WorkflowRun>>
pub fn count_workflow_runs(&self, workflow_id: &str) -> Result<i64>
pub fn get_workflow_run(&self, id: &str) -> Result<Option<WorkflowRun>>
pub fn delete_workflow_runs(&self, workflow_id: &str) -> Result<()>

// Activity Results
pub fn create_activity_result(&self, run_id, activity_id, status, tokens_used, attempts, error, started_at, completed_at) -> Result<()>
pub fn list_activity_results(&self, run_id: &str) -> Result<Vec<WorkflowActivityResult>>
```

### Role Queries

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

## 11. HTTP Endpoints

**Source:** `crates/server/src/handlers/workflows.rs`, `crates/server/src/handlers/roles.rs`

### Workflow Endpoints

| Method | Path | Handler | Purpose |
|---|---|---|---|
| GET | `/workflows` | `list_workflows` | List workflows (paginated) |
| POST | `/workflows` | `create_workflow` | Create from JSON definition |
| GET | `/workflows/{id}` | `get_workflow` | Get single workflow |
| PUT | `/workflows/{id}` | `update_workflow` | Update definition |
| DELETE | `/workflows/{id}` | `delete_workflow` | Delete + unregister triggers |
| POST | `/workflows/{id}/toggle` | `toggle_workflow` | Enable/disable |
| POST | `/workflows/{id}/run` | `run_workflow` | Start execution (returns immediately) |
| GET | `/workflows/{id}/runs` | `list_runs` | List runs (paginated) |
| GET | `/workflows/{id}/runs/{runId}` | `get_run` | Get run + activity results |
| POST | `/workflows/{id}/runs/{runId}/cancel` | `cancel_run` | Cancel via CancellationToken |
| GET | `/workflows/{id}/bindings` | `list_bindings` | List tool bindings |
| PUT | `/workflows/{id}/bindings` | `update_bindings` | Replace tool bindings |

### Role Endpoints

| Method | Path | Handler | Purpose |
|---|---|---|---|
| GET | `/roles` | `list_roles` | List roles (paginated) |
| POST | `/roles` | `create_role` | Create + process bindings + cascade deps |
| GET | `/roles/{id}` | `get_role` | Get single role |
| PUT | `/roles/{id}` | `update_role` | Update ROLE.md + metadata |
| DELETE | `/roles/{id}` | `delete_role` | Delete + unregister all triggers |
| POST | `/roles/{id}/toggle` | `toggle_role` | Enable/disable |
| POST | `/roles/{id}/install-deps` | `install_deps` | Force-install all missing deps |

### Create Workflow Response

```json
{
  "workflow": { "id": "...", "name": "...", "version": "...", ... },
  "cascade": {
    "results": [...],
    "installed_count": 2,
    "pending_count": 0,
    "failed_count": 0
  }
}
```

### Create Role Response

```json
{
  "role": { "id": "...", "name": "...", ... },
  "installReport": [
    { "binding": "morning-briefing", "status": "linked", "workflowId": "abc123" },
    { "binding": "lead-scorer", "status": "pending", "reason": "workflow not installed" }
  ],
  "cascade": { ... }
}
```

---

## 12. WorkflowManager Implementation

**Source:** `crates/server/src/workflow_manager.rs`

### WorkflowManagerImpl

```rust
pub struct WorkflowManagerImpl {
    store: Arc<Store>,
    tools: Arc<tools::Registry>,
    hub: Arc<ClientHub>,
    runner_providers: Arc<RwLock<Vec<Box<dyn ai::Provider>>>>,
    skill_loader: Option<Arc<tools::skills::Loader>>,
    event_bus: Option<tools::EventBus>,
    active_runs: Arc<RwLock<HashMap<String, CancellationToken>>>,
}
```

### run() Implementation

The `run()` method is the most complex — it spawns background execution:

```rust
pub async fn run(&self, id: &str, inputs: Value, trigger_type: &str) -> Result<String, String> {
    // 1. Load workflow from DB or filesystem
    let wf = self.resolve_workflow(id)?;
    let def = parse_workflow(&wf.definition)?;

    // 2. Create run record in DB
    let run_id = uuid::Uuid::new_v4().to_string();
    let session_key = format!("workflow-{}-{}", wf.id, run_id);
    store.create_workflow_run(&run_id, &wf.id, trigger_type, None, ...)?;

    // 3. Create cancellation token
    let cancel_token = CancellationToken::new();
    active_runs.write().await.insert(run_id.clone(), cancel_token.clone());

    // 4. Spawn background execution
    tokio::spawn(async move {
        // Get first available AI provider
        let provider = providers.read().await.first()?;

        // Build tool wrappers (exclude MCP proxies)
        let resolved_tools = build_registry_tools(&tools, &tool_defs);

        // Load skill content for referenced skills
        let skill_content = load_skill_content(&skill_loader, &def);

        // Execute workflow
        let result = workflow::engine::execute_workflow(
            &def, inputs, trigger_type, None,
            &store, provider, &resolved_tools,
            Some(&run_id), Some(&cancel_token),
            skill_content.as_ref(), event_bus.as_ref(),
        ).await;

        // Update run record
        match result {
            Ok(_) => store.complete_workflow_run(&run_id, "completed", ...),
            Err(WorkflowError::Cancelled) => store.complete_workflow_run(&run_id, "cancelled", ...),
            Err(e) => store.complete_workflow_run(&run_id, "failed", ...),
        }

        // Broadcast result via hub
        hub.broadcast("workflow_run_completed" or "workflow_run_failed", ...);

        // Clean up
        active_runs.write().await.remove(&run_id);
    });

    Ok(run_id)
}
```

### cancel() Implementation

```rust
pub async fn cancel(&self, run_id: &str) -> Result<(), String> {
    if let Some(token) = self.active_runs.read().await.get(run_id) {
        token.cancel();
        self.store.update_workflow_run(run_id, Some("cancelled"), ...)?;
        self.hub.broadcast("workflow_run_cancelled", ...);
        Ok(())
    } else {
        Err("Run not found or already completed".into())
    }
}
```

---

## 13. Tool Resolution

**Source:** `crates/server/src/workflow_manager.rs`

### RegistryTool Wrapper

All built-in tools are wrapped for workflow execution:

```rust
struct RegistryTool {
    tool_name: String,
    tool_desc: String,
    tool_schema: serde_json::Value,
    registry: Arc<tools::Registry>,
}

impl DynTool for RegistryTool {
    fn execute_dyn(&self, ctx, input) -> Pin<Box<...>> {
        Box::pin(async move {
            self.registry.execute(ctx, &self.tool_name, input).await
        })
    }
}
```

### Tool Filtering for Workflows

```rust
let resolved_tools: Vec<Box<dyn DynTool>> = tool_defs
    .iter()
    .filter(|td| !td.name.starts_with("mcp__"))  // EXCLUDE MCP proxies
    .map(|td| Box::new(RegistryTool { ... }) as Box<dyn DynTool>)
    .collect();
```

**MCP proxy tools are never available to workflow activities.** Only built-in tools and .napp tools are resolved.

### EmitTool Injection

If an `event_bus` is available, the `EmitTool` is automatically added to the resolved tools array. Activities do not need to declare it.

---

## 14. Error Handling & Budgets

### WorkflowError

```rust
pub enum WorkflowError {
    Parse(String),
    Validation(String),
    MissingDependency(String),
    UnresolvedInterface(String),
    MaxIterations(String),               // Activity exceeded 20 iterations
    BudgetExceeded {
        activity_id: String,
        used: u32,
        limit: u32,
    },
    ActivityFailed(String, String),      // activity_id, error_msg
    NotFound(String),
    Database(String),
    Provider(String),
    Cancelled,
    Other(String),
}
```

### Token Budget Enforcement

**Per-activity:**
```rust
if tokens_used >= activity.token_budget.max {
    return Err(WorkflowError::BudgetExceeded { ... });
}
```

**Per-workflow:**
```rust
if def.budget.total_per_run > 0 && total_tokens > def.budget.total_per_run {
    return Err(WorkflowError::BudgetExceeded { activity_id: "workflow", ... });
}
```

### Fallback State Machine

```
Activity fails after all retries:

on_error.fallback:
├─ Skip       → Log warning, continue to next activity
├─ Abort      → Fail entire workflow, return error
└─ NotifyOwner → Log + fail workflow (default)
```

---

## 15. Filesystem & Package Storage

### Directory Structure

```
~/.nebo/
├── nebo/                            # Marketplace (sealed .napp archives)
│   ├── workflows/
│   │   └── @org/workflows/name/
│   │       └── 1.0.0.napp
│   └── roles/
│       └── @org/roles/name/
│           └── 1.0.0.napp
├── user/                            # User-created (loose files)
│   ├── workflows/
│   │   └── my-workflow/
│   │       ├── workflow.json
│   │       └── WORKFLOW.md
│   └── roles/
│       └── my-role/
│           ├── ROLE.md
│           └── role.json
```

### napp_path (DB field, migration 0051)

After migration, each workflow/role record has a `napp_path` column:
- **Installed:** path to extracted directory or `.napp` file
- **User-created:** path to `user/workflows/{name}` or `user/roles/{name}`

Loading priority: read from `napp_path` first, fall back to `definition` column in DB.

---

## 16. Workflow Loader

**Source:** `crates/workflow/src/loader.rs`

### LoadedWorkflow

```rust
pub struct LoadedWorkflow {
    pub id: String,
    pub name: String,
    pub version: String,
    pub definition: String,           // Raw workflow.json content
    pub skill_md: Option<String>,     // WORKFLOW.md content
    pub manifest: Option<String>,     // manifest.json content
    pub source: WorkflowSource,
    pub path: PathBuf,
}

pub enum WorkflowSource {
    Installed,                        // nebo/workflows/
    User,                             // user/workflows/
}
```

### Scanning Functions

```rust
pub fn scan_installed_workflows(dir: &Path) -> Vec<LoadedWorkflow>
pub fn scan_user_workflows(dir: &Path) -> Vec<LoadedWorkflow>
pub fn load_from_dir(dir: &Path, source: WorkflowSource) -> Result<LoadedWorkflow, String>
```

**scan_installed_workflows:** Walks `nebo/workflows/` recursively, looks for directories containing `workflow.json`. For `.napp` files, reads `workflow.json` directly from the sealed archive.

**scan_user_workflows:** Walks `user/workflows/` looking for `workflow.json` in immediate subdirectories.

---

## 17. Role Loader

**Source:** `crates/napp/src/role_loader.rs`

### LoadedRole

```rust
pub struct LoadedRole {
    pub id: String,
    pub name: String,
    pub description: String,
    pub role_md: String,
    pub role_config: Option<RoleConfig>,
    pub source: RoleSource,
    pub path: PathBuf,
}

pub enum RoleSource {
    Installed,                        // nebo/roles/
    User,                             // user/roles/
}
```

### Scanning Functions

```rust
pub fn scan_installed_roles(dir: &Path) -> Vec<LoadedRole>
pub fn scan_user_roles(dir: &Path) -> Vec<LoadedRole>
```

Scans for directories containing `ROLE.md`. Optionally loads `role.json` for operational config.

---

## 18. Integration Points

### With Agent System

- Agent uses `WorkTool` to trigger workflows: `work(resource: "my-workflow", action: "run")`
- Agent can emit events that trigger event-subscribed workflows
- Workflows run independently in background (do not block agent conversation)

### With Skill System

- Activities reference skills by qualified name in `activity.skills`
- Skill loader provides SKILL.md content
- Content injected into activity system prompt as `## Skills` section
- No global skill context bleed between activities

### With Provider System

- Activities specify model override (e.g., `"sonnet"`)
- WorkflowManager picks first available provider from `runner_providers`
- All streaming handled via `ai::Provider` trait

### With Tools Registry

- All registered built-in tools available to activities
- MCP proxy tools (`mcp__*` prefix) filtered out
- Tool execution via `DynTool` trait through `RegistryTool` wrapper

### With Event Bus

- `EmitTool` injected into every activity automatically
- Events flow via unbounded channel to `EventDispatcher`
- `EventDispatcher` matches patterns and triggers workflows
- Enables workflow-to-workflow communication

### With Database

- All workflow state persisted via `Store`
- Cascade deletes via FK constraints (runs, activity results, bindings)
- Activity results tracked for observability

### With Cron Scheduler

- Cron jobs created for schedule triggers
- `task_type = "workflow"` entries fire `workflow_manager.run()`
- Cron job names: `workflow-{id}` (direct) or `role-{role_id}-{binding}` (via role)

---

## 19. Constants & Defaults

```rust
const MAX_ITERATIONS: u32 = 20;              // Max tool-call loops per activity

// Token budgets
TokenBudget::default().max = 4096
Budget::default().total_per_run = 0           // 0 = unlimited

// Retry
OnError::default().retry = 1
OnError::default().fallback = Fallback::NotifyOwner

// Workflow list pagination
default_limit = 50
max_limit = 100

// Run status values
"running", "completed", "failed", "cancelled"

// Trigger types
"manual", "schedule", "event", "heartbeat"

// Cancellation
CancellationToken from tokio_util::sync
```

---

## 20. Cross-Reference to Go Docs

| Rust (this doc) | Go Equivalent |
|---|---|
| `crates/workflow/src/parser.rs` | `internal/workflow/parser.go` |
| `crates/workflow/src/engine.rs` | `internal/workflow/engine.go` |
| `crates/workflow/src/triggers.rs` | `internal/workflow/triggers.go` |
| `crates/workflow/src/events.rs` | New in Rust |
| `crates/workflow/src/loader.rs` | New in Rust |
| `crates/server/src/workflow_manager.rs` | `internal/server/workflow_handler.go` |
| `crates/server/src/handlers/workflows.rs` | `internal/server/workflow_routes.go` |
| `crates/server/src/handlers/roles.rs` | `internal/server/role_routes.go` |
| `crates/napp/src/role.rs` | `internal/apps/role/config.go` |
| `crates/napp/src/role_loader.rs` | New in Rust |
| `crates/tools/src/workflows/work_tool.rs` | `internal/agent/tools/workflow_domain.go` |
| `crates/tools/src/emit_tool.rs` | New in Rust |
| `crates/tools/src/events.rs` | New in Rust |

**Go-only docs for reference:**
- [workflow-engine.md](workflow-engine.md) — Original implementation status + wiring plan

**Canonical specification:**
- [platform-taxonomy.md](platform-taxonomy.md) — Authoritative ROLE/WORK/SKILL hierarchy definition

---

*Last updated: 2026-03-08*
