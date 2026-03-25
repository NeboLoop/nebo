# Automation System — SME Reference

Comprehensive Subject Matter Expert document covering the full Nebo automation
pipeline: proactive heartbeats, cron scheduling, workflow execution, event-driven
triggers, role workers, and frontend UI.

**Status:** Current (Rust implementation) | **Last updated:** 2026-03-25

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [Three Scheduling Patterns](#2-three-scheduling-patterns)
3. [Cron Scheduler](#3-cron-scheduler)
4. [Heartbeat System](#4-heartbeat-system)
5. [Event System](#5-event-system)
6. [Workflow Manager](#6-workflow-manager)
7. [Workflow Engine](#7-workflow-engine)
8. [Role Workers](#8-role-workers)
9. [Trigger Registration](#9-trigger-registration)
10. [DB Schema](#10-db-schema)
11. [REST Endpoints](#11-rest-endpoints)
12. [Frontend — Automate Tab](#12-frontend--automate-tab)
13. [Frontend — Automation Editor](#13-frontend--automation-editor)
14. [Frontend — Entity Config Panel](#14-frontend--entity-config-panel)
15. [Frontend — Types & API](#15-frontend--types--api)
16. [End-to-End Flows](#16-end-to-end-flows)
17. [Error Handling & Recovery](#17-error-handling--recovery)
18. [Constants & Timings](#18-constants--timings)
19. [Known Issues](#19-known-issues)

---

## 1. Architecture Overview

```
                         ┌─────────────────────────────────────────┐
                         │           TRIGGER LAYER                 │
                         │                                         │
  ┌──────────┐   ┌──────────────┐   ┌───────────────┐   ┌───────┐│
  │  Cron    │   │  Heartbeat   │   │ EventDispatcher│   │Manual ││
  │Scheduler │   │  Scheduler   │   │  (real-time)   │   │(REST) ││
  │(60s tick)│   │  (60s tick)  │   │                │   │       ││
  └────┬─────┘   └──────┬───────┘   └───────┬────────┘   └───┬───┘│
       │                │                    │                │    │
       └────────────────┴────────────────────┴────────────────┘    │
                         │                                         │
                         ▼                                         │
                ┌─────────────────┐                                │
                │ WorkflowManager │  (trait: run / run_inline)     │
                │   Impl          │                                │
                └────────┬────────┘                                │
                         │                                         │
              ┌──────────┴──────────┐                              │
              │                     │                              │
              ▼                     ▼                              │
     ┌────────────────┐   ┌─────────────┐                         │
     │ Workflow Engine │   │  run_chat() │  (heartbeat → agent)    │
     │  (activities)   │   │  (HEARTBEAT │                         │
     │                 │   │    lane)    │                         │
     └────────┬────────┘   └─────────────┘                         │
              │                                                    │
              ▼                                                    │
     ┌─────────────┐       ┌─────────────┐                        │
     │ emit_tool   │──────>│  EventBus   │─── feeds back ──────>──┘
     │ (in activity│       │ (mpsc chan) │
     │  context)   │       └─────────────┘
     └─────────────┘
```

**Design principle:** Three concurrent scheduling patterns (cron, heartbeat, event)
converge on the same execution backend. Workflow activities run as LLM agent tasks
with full tool access. Events emitted by one workflow can trigger another, creating
chains.

---

## 2. Three Scheduling Patterns

| Pattern | Source | Tick | Execution | Lane |
|---------|--------|------|-----------|------|
| **Cron** | `scheduler.rs` | 60s | Shell, agent, workflow, or role_workflow | Varies |
| **Heartbeat** | `heartbeat.rs` | 60s | `run_chat()` with heartbeat content | `HEARTBEAT` |
| **Event** | `EventDispatcher` | Real-time | `WorkflowManager.run_inline()` | Spawned |

All three coexist as independent `tokio::spawn` loops started during server boot.

**Boot sequence:**
1. Server starts, builds `AppState`
2. `scheduler::spawn()` — 10s delay, then 60s tick
3. `heartbeat::spawn()` — 15s delay, then 60s tick
4. `EventDispatcher::spawn()` — immediate, consumes from EventBus channel
5. `RoleWorkerRegistry` — starts workers for each active role

---

## 3. Cron Scheduler

**File:** `crates/server/src/scheduler.rs`

### Spawn

```rust
pub fn spawn(
    store: Arc<Store>,
    runner: Arc<Runner>,
    hub: Arc<ClientHub>,
    snapshot_store: Arc<browser::SnapshotStore>,
    workflow_manager: Arc<dyn tools::workflows::WorkflowManager>,
)
```

10s boot delay → 60s tick loop. Each tick:
1. Query all enabled `cron_jobs`
2. For each: parse schedule, check if due, dispatch by task_type
3. Clean up completed tasks older than 7 days

### Task Type Dispatch

| task_type | Command format | Execution |
|-----------|----------------|-----------|
| `"bash"` / `"shell"` / `""` | Shell command | `sh -c {command}` subprocess |
| `"agent"` | Prompt text | `runner.run()` with `Origin::System`, session `"cron-{name}"` |
| `"workflow"` | Workflow ID | `manager.run(id, null, "cron")` |
| `"role_workflow"` | `"role:{role_id}:{binding}"` | Parse → load role → `manager.run_inline()` |

### Cron Resolution

Uses the `cron` crate for schedule parsing:
```rust
let schedule: Schedule = job.schedule.parse()?;
let next = schedule.after(&last_run).next()?;
if next > now { continue; }  // Not due
```

Timestamp parsing: tries `i64` Unix → `NaiveDateTime` format → defaults to epoch 0.

### Role Workflow Execution

When `task_type == "role_workflow"`:
1. Parse command: `"role:{role_id}:{binding_name}"`
2. Load role from DB → parse `RoleConfig` from frontmatter
3. Lookup binding → check `has_activities()` (skip chat-only bindings)
4. Convert to inline definition via `binding.to_workflow_json()`
5. Build emit_source: `"{role-slug}.{emit-name}"`
6. Call `manager.run_inline(def_json, inputs, "schedule", role_id, emit_source)`

### History Tracking

```rust
store.create_cron_history(job.id);         // Before execution
store.update_cron_job_last_run(job.id, err_msg);  // After execution
store.update_cron_history(h.id, success, output, error);
```

---

## 4. Heartbeat System

**File:** `crates/server/src/heartbeat.rs`

### Spawn

```rust
pub fn spawn(state: AppState)
```

15s boot delay → 60s tick loop.

### Entity Resolution

Each tick loads heartbeat-eligible entities:
```rust
let mut entities = state.store.list_heartbeat_entities()?;
```

Entity types: `"main"` (global), `"role"`, `"channel"`. The main entity is
auto-included if global `heartbeat_interval_minutes > 0` and not explicitly disabled.

### Configuration Resolution

Uses `entity_config::resolve()` — layers entity-specific overrides on top of
global Settings defaults:

```rust
pub struct ResolvedEntityConfig {
    pub entity_type: String,
    pub entity_id: String,
    pub heartbeat_enabled: bool,
    pub heartbeat_interval_minutes: i64,
    pub heartbeat_content: String,
    pub heartbeat_window: Option<(String, String)>,  // (HH:MM, HH:MM)
    pub permissions: HashMap<String, bool>,
    pub resource_grants: HashMap<String, String>,
    pub model_preference: Option<String>,
    pub personality_snippet: Option<String>,
    pub overrides: HashMap<String, bool>,            // Which fields are overridden (not inherited)
    pub allowed_paths: Vec<String>,                  // Restricts file writes and shell to these dirs
}
```

Resolution cascade: entity-level → global Settings → hardcoded default.

### Throttling

```rust
type LastFired = Arc<Mutex<HashMap<String, Instant>>>;
// Key: "{entity_type}-{entity_id}"
// Checks: now.duration_since(last) >= interval_duration
```

Uses `Instant` (monotonic clock) to avoid timestamp manipulation.

### Time Window

```rust
fn in_time_window(start: &str, end: &str) -> bool {
    let now = Local::now().format("%H:%M").to_string();
    if start <= end { now >= start && now <= end }    // Normal
    else { now >= start || now <= end }               // Midnight wrap
}
```

### Chat Dispatch

Builds `ChatConfig` and calls `run_chat()`:
```rust
ChatConfig {
    session_key: format!("heartbeat-{}-{}", entity.entity_type, entity.entity_id),
    prompt: resolved.heartbeat_content.clone(),
    origin: Origin::System,
    channel: "heartbeat",
    lane: lanes::HEARTBEAT,
    role_id,  // Set if entity_type == "role"
    entity_config: Some(resolved),
    ..
}
```

The HEARTBEAT lane is isolated from MAIN (user input) — heartbeats never block chat.

---

## 5. Event System

### EventBus (`crates/tools/src/events.rs`)

```rust
pub struct Event {
    pub source: String,              // "email.urgent", "workflow.triage.completed"
    pub payload: serde_json::Value,
    pub origin: String,              // Trace: "workflow:email-triage:run-550e"
    pub timestamp: u64,              // Unix epoch seconds
}

pub struct EventBus {
    tx: mpsc::UnboundedSender<Event>,
}
```

Best-effort delivery via unbounded channel. Events dropped if receiver gone.

### Emit Tool

Auto-injected into every workflow activity. Not in the normal tool registry — the
engine adds it directly.

```rust
// Input: { "source": "email.urgent", "payload": {...} }
// Creates Event and sends to EventBus
```

### EventDispatcher (`crates/workflow/src/events.rs`)

```rust
pub struct EventSubscription {
    pub pattern: String,                   // "email.*" or "email.urgent"
    pub default_inputs: serde_json::Value,
    pub role_source: String,               // Role ID
    pub binding_name: String,
    pub definition_json: Option<String>,   // Inline workflow JSON
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
- Wildcard: `"email.*"` matches anything starting with `"email."`

---

## 6. Workflow Manager

**File:** `crates/server/src/workflow_manager.rs`

### Trait

```rust
pub trait WorkflowManager: Send + Sync {
    fn run(&self, id, inputs, trigger_type) -> Result<String>;      // Standalone
    fn run_inline(&self, def_json, inputs, trigger, role_id, emit) -> Result<String>;  // Role inline
    fn cancel(&self, run_id) -> Result<()>;
    fn list(&self) -> Vec<WorkflowInfo>;
    fn run_status(&self, run_id) -> Result<WorkflowRunInfo>;
    fn list_runs(&self, workflow_id, limit) -> Vec<WorkflowRunInfo>;
    fn toggle(&self, id) -> Result<bool>;
    fn install(&self, code) -> Result<WorkflowInfo>;
    fn uninstall(&self, id) -> Result<()>;
    fn create(&self, name, definition) -> Result<WorkflowInfo>;
    fn resolve(&self, name_or_id) -> Result<WorkflowInfo>;
}
```

### run() — Standalone Workflow

1. Load workflow from DB → check enabled
2. Parse definition from `napp_path` or DB column
3. Create `workflow_runs` record (status: "running")
4. Store `CancellationToken` in active_runs map
5. **tokio::spawn** background task:
   - Get provider, build tool wrappers, load skill content
   - Call `workflow::engine::execute_workflow()`
   - Update run status on completion/failure
   - Broadcast WebSocket events
6. Return `run_id` immediately

### run_inline() — Role Workflow

Same as `run()` but:
- Definition parsed directly from JSON string (not from DB)
- `workflow_id` = `"role:{role_id}"`
- Session key = `"role-{role_id}-{run_id}"`
- Passes `emit_source` to engine

### cancel_run()

Looks up `CancellationToken` from active_runs → `token.cancel()` → update DB → broadcast.

### Definition Resolution Priority

1. Filesystem: `wf.napp_path` → read `workflow.json`
2. DB column: parse `wf.definition`

---

## 7. Workflow Engine

**File:** `crates/workflow/src/engine.rs`

### Entry Point

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
    progress_tx: Option<tokio::sync::mpsc::UnboundedSender<WorkflowProgress>>,
) -> Result<(String, String), WorkflowError>
```

### Activity Loop

For each activity in `def.activities`:

1. **Cancellation check** — bail if token cancelled
2. **Update run** — set `current_activity` in DB
3. **Execute with retry** — up to `activity.on_error.retry` attempts
4. **Record result** — `workflow_activity_results` row
5. **Accumulate context** — prior results passed to next activity
6. **Budget check** — per-activity and global token limits
7. **Error handling** — `Fallback::Skip` continues, `Abort` stops

### Activity Execution (Single)

```rust
pub async fn execute_activity(
    activity: &Activity, prior_context: &str, inputs: &Value,
    provider: &dyn Provider, tools: &[&Box<dyn DynTool>],
    skill_content: Option<&HashMap<String, String>>,
    emit_source: Option<&str>,
) -> Result<(String, u32), WorkflowError>
```

**System prompt construction:**
1. Skills (injected from SKILL.md content)
2. Task (activity.intent)
3. Steps (activity.steps array)
4. Inputs (user-provided, excluding `_` prefixed keys)
5. Prior Results (accumulated from prior activities)
6. Workflow Controls (exit, emit hints)
7. Output instruction (if emit_source set on last activity)

**Agentic loop** (max 20 iterations):
1. Build ChatRequest → `provider.stream()`
2. Accumulate text + tool_calls
3. If no tool_calls → return (response, tokens)
4. Execute each tool call
5. Check for `EXIT_SENTINEL` in tool results → early exit
6. Continue loop

**Auto-injected tools:** `emit_tool` and `exit_tool` are always available, even if
not declared in the activity definition.

### WorkflowDef Structure

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
    pub intent: String,          // Task description
    pub skills: Vec<String>,     // Skill references
    pub mcps: Vec<String>,       // MCP server references
    pub cmds: Vec<String>,       // Command references
    pub model: String,           // Model override
    pub steps: Vec<String>,
    pub token_budget: TokenBudget,
    pub on_error: OnError,
}

pub struct Budget { pub total_per_run: u32, pub cost_estimate: String }
pub struct TokenBudget { pub max: u32 }
pub struct OnError { pub retry: u32, pub fallback: Fallback }
pub enum Fallback { NotifyOwner, Skip, Abort }
```

### EXIT_SENTINEL

```rust
pub const EXIT_SENTINEL: &str = "__WORKFLOW_EXIT__:";
```

When a tool result starts with this prefix, the workflow exits early with the
remainder as the reason. Status = `"exited"` (distinct from "completed" or "failed").

---

## 8. Role Workers

**File:** `crates/agent/src/role_worker.rs`

### RoleWorker

```rust
pub struct RoleWorker {
    pub role_id: String,
    pub name: String,
    cancel: CancellationToken,
}
```

**start():**
1. Load role workflow bindings from DB
2. Parse `RoleConfig` from role frontmatter
3. Register cron triggers via `register_role_triggers()`
4. For each binding, spawn trigger task by type:

| trigger_type | Worker behavior |
|-------------|-----------------|
| `schedule` | No-op (handled by cron scheduler via registered jobs) |
| `heartbeat` | Spawns `tokio::interval` task → `run_inline()` on tick, with time window check |
| `event` | Subscribes patterns to `EventDispatcher` |
| `manual` | No-op (user-triggered via REST) |

**stop():**
- Cancel token → stops all spawned tasks
- `unregister_role_triggers()` → removes cron jobs

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
                    if now < start || now > end { continue; }
                }
                mgr.run_inline(def_json, inputs, "heartbeat", &role, emit_source).await;
            }
            _ = token.cancelled() => break,
        }
    }
});
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

Methods: `start_role()`, `stop_role()`, `stop_all()`.
`start_role()` stops existing worker first (clean re-registration).

---

## 9. Trigger Registration

**File:** `crates/workflow/src/triggers.rs`

### Standalone Workflows

```rust
pub fn register_schedule_trigger(workflow_id: &str, cron: &str, store: &Store)
// Creates cron_job: name="workflow-{id}", task_type="workflow", command=workflow_id
```

### Role Workflows

```rust
pub fn register_role_triggers(role_id: &str, bindings: &[RoleWorkflow], store: &Store)
// For schedule bindings:
//   Creates cron_job: name="role-{role_id}-{binding}", task_type="role_workflow",
//   command="role:{role_id}:{binding}"
```

### Unregistration

```rust
pub fn unregister_triggers(workflow_id: &str, store: &Store)        // By workflow
pub fn unregister_role_triggers(role_id: &str, store: &Store)       // All role triggers
pub fn unregister_single_role_trigger(role_id, binding, store)      // Single binding
```

All use DB deletion by cron_job name pattern.

---

## 10. DB Schema

### cron_jobs

```sql
CREATE TABLE cron_jobs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT UNIQUE NOT NULL,
    schedule TEXT NOT NULL,              -- Cron expression
    command TEXT DEFAULT '',             -- Shell cmd / workflow ID / "role:id:binding"
    task_type TEXT DEFAULT 'bash',       -- bash, shell, agent, workflow, role_workflow
    message TEXT DEFAULT '',             -- Agent prompt
    deliver TEXT DEFAULT '',
    enabled INTEGER DEFAULT 1,
    last_run DATETIME,
    run_count INTEGER DEFAULT 0,
    last_error TEXT,
    instructions TEXT,                   -- System prompt for agent tasks (added in migration 0042)
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

### workflow_runs

```sql
CREATE TABLE workflow_runs (
    id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL,           -- or "role:{role_id}" for inline
    trigger_type TEXT NOT NULL,          -- schedule, event, manual, cron, heartbeat
    trigger_detail TEXT,
    status TEXT NOT NULL DEFAULT 'running',  -- running, completed, failed, cancelled, exited
    inputs TEXT,                         -- JSON
    current_activity TEXT,
    total_tokens_used INTEGER DEFAULT 0,
    error TEXT,
    error_activity TEXT,
    session_key TEXT,
    started_at INTEGER NOT NULL DEFAULT (unixepoch()),
    completed_at INTEGER
);
```

### workflow_activity_results

```sql
CREATE TABLE workflow_activity_results (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
    activity_id TEXT NOT NULL,
    status TEXT NOT NULL,                -- completed, failed, exited
    tokens_used INTEGER DEFAULT 0,
    attempts INTEGER DEFAULT 1,
    error TEXT,
    started_at INTEGER NOT NULL,
    completed_at INTEGER
);
```

### role_workflows

```sql
CREATE TABLE role_workflows (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    role_id TEXT NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    binding_name TEXT NOT NULL,
    trigger_type TEXT NOT NULL,          -- schedule, heartbeat, event, manual
    trigger_config TEXT NOT NULL,        -- Cron / "30m|08:00-18:00" / "email.*,cal.changed"
    description TEXT,
    inputs TEXT,                         -- JSON
    is_active INTEGER NOT NULL DEFAULT 1,
    emit TEXT,                           -- Event to announce on completion
    activities TEXT,                     -- JSON of activity definitions
    UNIQUE(role_id, binding_name)
);
```

### entity_config

```sql
CREATE TABLE entity_config (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_type TEXT NOT NULL CHECK(entity_type IN ('main', 'role', 'channel')),
    entity_id   TEXT NOT NULL,
    heartbeat_enabled INTEGER,           -- NULL=inherit, 0/1
    heartbeat_interval_minutes INTEGER,
    heartbeat_content TEXT,
    heartbeat_window_start TEXT,         -- HH:MM
    heartbeat_window_end TEXT,
    permissions TEXT,                    -- JSON: {"web": true, ...}
    resource_grants TEXT,                -- JSON: {"screen": "allow", ...}
    model_preference TEXT,
    personality_snippet TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(entity_type, entity_id)
);
```

### Key Relationships

```
roles (1) ──→ (many) role_workflows
workflows (1) ──→ (many) workflow_runs ──→ (many) workflow_activity_results
cron_jobs (1) ──→ (many) cron_history
entity_config — standalone per (type, id) pair
```

---

## 11. REST Endpoints

### Tasks (Cron Jobs) — `/api/v1/tasks`

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/tasks` | List all cron jobs |
| POST | `/api/v1/tasks` | Create cron job |
| GET | `/api/v1/tasks/:name` | Get job |
| PUT | `/api/v1/tasks/:name` | Update job |
| DELETE | `/api/v1/tasks/:name` | Delete job |
| POST | `/api/v1/tasks/:name/toggle` | Enable/disable |
| POST | `/api/v1/tasks/:name/run` | Execute immediately |
| GET | `/api/v1/tasks/:name/history` | Execution history |

### Workflows — `/api/v1/workflows`

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/workflows` | List workflows |
| POST | `/api/v1/workflows` | Create workflow |
| GET | `/api/v1/workflows/{id}` | Get workflow |
| PUT | `/api/v1/workflows/{id}` | Update workflow |
| DELETE | `/api/v1/workflows/{id}` | Delete workflow |
| POST | `/api/v1/workflows/{id}/toggle` | Enable/disable |
| POST | `/api/v1/workflows/{id}/run` | Execute (body: `{inputs}`) |
| GET | `/api/v1/workflows/{id}/runs` | List runs |
| GET | `/api/v1/workflows/{id}/runs/{runId}` | Run status |
| POST | `/api/v1/workflows/{id}/runs/{runId}/cancel` | Cancel run |

### Role Workflows — `/api/v1/roles/{id}/workflows`

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/roles/{id}/workflows` | List bindings |
| POST | `/api/v1/roles/{id}/workflows` | Create binding |
| PUT | `/api/v1/roles/{id}/workflows/{binding}` | Update binding |
| DELETE | `/api/v1/roles/{id}/workflows/{binding}` | Delete binding |
| POST | `/api/v1/roles/{id}/workflows/{binding}/toggle` | Toggle active |

### Entity Config — `/api/v1/entity-config`

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/entity-config/{type}/{id}` | Get resolved config |
| PUT | `/api/v1/entity-config/{type}/{id}` | Patch-update overrides |
| DELETE | `/api/v1/entity-config/{type}/{id}` | Reset to inherited defaults |

### Event Sources

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/roles/event-sources` | Available emit sources from active bindings |

---

## 12. Frontend — Automate Tab

**File:** `app/src/lib/components/agent/AutomationsSection.svelte`

### Props

```typescript
{
    entityType: string;    // 'role' | 'main'
    entityId: string;      // Role ID or 'main'
    roleId?: string;       // Required for workflow CRUD
    readonly?: boolean;    // true for assistant (disables editing)
}
```

### Two Operating Modes

Radio button selector:

| Mode | Label | Description |
|------|-------|-------------|
| `heartbeat` | Proactive check-ins | Wake on a schedule, check in using agent's judgment |
| `automations` | Automations | Run defined workflow sequences on triggers |

**Mode persistence:** Initial mode auto-selected on first load (automations if any
exist, else heartbeat). User's manual selection preserved across data reloads — only
reset when navigating to a different role (via `modeInitialized` flag).

### Heartbeat Config (mode = heartbeat)

- **Interval dropdown** — 1min to 24hr (10 options)
- **Active window** — two `<input type="time">` fields (HH:MM)
- **Content** — `RichInput` component (supports `/` slash mentions for MCPs, skills, agents)
- Auto-saves via `updateEntityConfig()` with 800ms debounce on content

### Automations List (mode = automations)

Per-workflow card displays:
- **Trigger icon** — 📅 schedule, ⏱ heartbeat, ⚡ event, ▶ manual
- **Description** — binding name or custom description
- **Trigger summary** — human-readable (e.g., "Daily at 7:30 AM", "Every 30 minutes")
- **Step count** — "2 steps" (from activities array)
- **Emit** — "announces: morning-briefing.done" (if configured)
- **Triggered by** — chain annotation if event trigger matches another workflow's emit
- **Toggle** — enable/disable checkbox
- **Overflow menu** — Edit, Duplicate, Delete (with confirm)

Empty state: clock icon + "No automations yet" + "New Automation" CTA.

### Trigger Summary Helpers

```typescript
summarizeTrigger(wf) → string
// schedule: cronToHuman("0 7 * * 1-5") → "Weekdays at 7:00 AM"
// heartbeat: "30m|09:00-18:00" → "Every 30 minutes, 9am–6pm"
// event: "email.urgent" → "When email.urgent fires"
// manual: "Run manually"
```

### Data Loading

```typescript
async loadAll() {
    const [configRes, wfRes] = await Promise.all([
        getEntityConfig(entityType, entityId),
        getRoleWorkflows(roleId),
    ]);
    // Initialize heartbeat state from configRes
    // Initialize workflows from wfRes
    // Auto-select mode on first load only
}
```

Reloads when `entityType` or `entityId` changes (via `$effect`).

### Route Integration

**Role automate:** `app/src/routes/(app)/(sidebar)/agent/role/[name]/automate/+page.svelte`
```svelte
<AutomationsSection entityType="role" entityId={activeRoleId} roleId={activeRoleId} />
```

**Assistant automate:** `app/src/routes/(app)/(sidebar)/agent/assistant/automate/+page.svelte`
```svelte
<AutomationsSection entityType="main" entityId="main" readonly={true} />
```

---

## 13. Frontend — Automation Editor

**File:** `app/src/lib/components/agent/AutomationEditor.svelte`

Full-page modal for creating and editing workflow bindings.

### Props

```typescript
{
    roleId: string;
    existing: RoleWorkflowEntry | null;  // null = create, object = edit
    onclose: () => void;
    onsave: () => void;
}
```

### Form Sections

**1. Name** — text input, auto-generates `bindingName` via slug conversion

**2. Steps (Activities)** — ordered list with:
- Drag handle for reordering
- Step number circle
- `RichInput` with `/` mention support (MCPs, skills, agents)
- Remove button (hover-reveal)
- Connector lines ("passes result to step N+1")
- Add step button

**3. Trigger Type** — 4-button grid:
- **Schedule** — time pickers (hour, minute, AM/PM) + day selector (every/weekdays/weekends/custom)
- **Interval** — duration dropdown (5m–24h) + optional time window
- **Event** — TagInput for sources + available events from other workflows (lazy-loaded)
- **Manual** — static text

**4. Inputs** — key-value pair editor for default parameters

**5. Emit** — checkbox to enable, event name input, payload toggle (output vs nothing)

**6. ID (Advanced)** — editable binding name (create mode only)

### Save Flow

```typescript
async handleSave() {
    // 1. Validate: bindingName required, >= 1 non-empty activity
    // 2. Build inputs object from rows (skip _emit, _payload reserved keys)
    // 3. Process activities: extract refs, clean intent text
    // 4. Build triggerConfig from form state
    // 5. Create or update via API
    // 6. Call onsave() on success
}
```

### Cron Builder

```typescript
buildCron(): string
// AM/PM → 24hr conversion
// Days: "every" → "*", "weekdays" → "1-5", "weekends" → "0,6", custom → comma list
// Returns: "{min} {hr24} * * {dow}"
```

### Reference Extraction

Activities support `{{type:id:name}}` markup from RichInput:
```typescript
extractRefs(intent): { type: 'mcp'|'skill'|'agent'|'cmd', id: string, name: string }[]
```

Stripped from the clean `intent` text, stored separately as `skills`, `mcps`, `cmds` arrays.

---

## 14. Frontend — Entity Config Panel

**File:** `app/src/lib/components/chat/EntityConfigPanel.svelte`

Accessible from the chat header gear icon. 5 sections:

### 1. Heartbeat

- Toggle enabled
- Interval dropdown (5min–24hr)
- Active window (time range)
- Content textarea (shows "Inherited from HEARTBEAT.md" if not overridden)
- Per-field reset buttons (sets to `null` → inherit)

### 2. Permissions (7 categories)

Web Search, Desktop Control, File System, Shell Commands, Memory Access, Calendar, Email.
Each: 3-way select (Inherit / Allow / Deny).

### 3. Resource Access

Screen Access, Browser Access. Each: 3-way select.

### 4. Model Preference

Text input for fuzzy model name (e.g., "sonnet", "gpt-4").

### 5. Personality Snippet

Textarea for additional personality instructions. Prepended to system prompt.

### Save Pattern

Each field auto-saves on blur/change via `updateEntityConfig(entityType, entityId, patch)`.
NULL values clear overrides (inherit from defaults).

---

## 15. Frontend — Types & API

### RoleWorkflowEntry

```typescript
interface RoleWorkflowEntry {
    id: number;
    roleId: string;
    bindingName: string;
    workflowRef: string;
    workflowId?: string;
    triggerType: string;       // 'schedule' | 'heartbeat' | 'event' | 'manual'
    triggerConfig: string;     // Cron / "interval|window" / "source1,source2"
    description?: string;
    inputs?: string;           // JSON
    isActive: boolean;
    emit?: string;             // Event name
    activities?: Array<Record<string, unknown>>;
}
```

### ResolvedEntityConfig

```typescript
interface ResolvedEntityConfig {
    entityType: string;
    entityId: string;
    heartbeatEnabled: boolean;
    heartbeatIntervalMinutes: number;
    heartbeatContent: string;
    heartbeatWindow: [string, string] | null;
    permissions: Record<string, boolean>;
    resourceGrants: Record<string, string>;
    modelPreference: string | null;
    personalitySnippet: string | null;
    overrides: Record<string, boolean>;  // Which fields are customized
}
```

### API Functions (`app/src/lib/api/nebo.ts`)

| Function | Method | Endpoint |
|----------|--------|----------|
| `getRoleWorkflows(roleId)` | GET | `/api/v1/roles/{id}/workflows` |
| `createRoleWorkflow(roleId, data)` | POST | `/api/v1/roles/{id}/workflows` |
| `updateRoleWorkflow(roleId, binding, data)` | PUT | `/api/v1/roles/{id}/workflows/{binding}` |
| `deleteRoleWorkflow(roleId, binding)` | DELETE | `/api/v1/roles/{id}/workflows/{binding}` |
| `toggleRoleWorkflow(roleId, binding)` | POST | `/api/v1/roles/{id}/workflows/{binding}/toggle` |
| `listEventSources()` | GET | `/api/v1/roles/event-sources` |
| `getEntityConfig(type, id)` | GET | `/api/v1/entity-config/{type}/{id}` |
| `updateEntityConfig(type, id, patch)` | PUT | `/api/v1/entity-config/{type}/{id}` |
| `deleteEntityConfig(type, id)` | DELETE | `/api/v1/entity-config/{type}/{id}` |

---

## 16. End-to-End Flows

### User Creates Heartbeat for a Role

1. **Frontend:** User opens role → Automate tab → selects "Proactive check-ins"
2. Sets interval to 30 min, window 9am–6pm, content "Check email for urgent items"
3. **API:** `PUT /entity-config/role/{id}` → saves to `entity_config` table
4. **Heartbeat tick (60s):** Loads entity, resolves config, checks LastFired + window
5. **When due:** Builds `ChatConfig` → `run_chat()` on HEARTBEAT lane
6. **Runner:** Creates session `"heartbeat-role-{id}"`, runs agentic loop with content as prompt
7. Agent checks email using tools, responds
8. Response broadcast via WebSocket to frontend

### User Creates Schedule Automation

1. **Frontend:** Automate tab → "Automations" → "+ New"
2. AutomationEditor: name "Morning Briefing", 2 steps, schedule 7:30 AM weekdays
3. **API:** `POST /roles/{id}/workflows` → creates `role_workflows` row
4. **Server:** Restarts role worker → `register_role_triggers()` creates cron_job
5. **Scheduler tick (60s):** Finds job due → `execute_role_workflow_task()`
6. Parses `"role:{role_id}:{binding}"` → loads role config → `run_inline()`
7. **Engine:** Executes activity 1 → passes result → executes activity 2
8. If emit configured: `emit_tool` fires → EventBus → EventDispatcher

### Event Chain: Workflow A → Workflow B

1. Workflow A completes, last activity calls `emit_tool` with source `"triage.done"`
2. **EventBus:** Event published to unbounded channel
3. **EventDispatcher:** Matches `"triage.done"` against subscription pattern `"triage.*"`
4. Found: Workflow B subscribed with `pattern: "triage.*"`
5. Injects `_event_source`, `_event_payload`, `_event_origin` into inputs
6. Calls `manager.run_inline()` for Workflow B
7. Workflow B executes with event data as input context

### User Cancels Running Workflow

1. **Frontend:** Activity tab shows running workflow → Cancel button
2. **API:** `POST /workflows/{id}/runs/{runId}/cancel`
3. **WorkflowManager:** Looks up CancellationToken → `token.cancel()`
4. **Engine:** Next activity check sees `token.is_cancelled()` → returns `WorkflowError::Cancelled`
5. Run status updated to `"cancelled"` in DB
6. WebSocket broadcast: `"workflow_run_cancelled"`

---

## 17. Error Handling & Recovery

### Scheduler Errors

| Error | Handling |
|-------|----------|
| Invalid cron expression | Logged, job skipped |
| DB errors on history | Best-effort, non-critical |
| Agent run failure | Captured in (success, output, error) tuple |
| Subprocess failure | Exit code + stderr captured |

### Heartbeat Errors

| Error | Handling |
|-------|----------|
| Entity config resolution failure | Uses safe defaults |
| HEARTBEAT.md read failure | Empty string fallback |
| Chat dispatch error | Logged, next tick retries |

### Workflow Engine Errors

| Error Type | Fallback | Behavior |
|-----------|----------|----------|
| Provider error | Immediate | Return error, mark run failed |
| Token budget exceeded | Per-activity or global | Mark run failed |
| Tool not found | Continue | `ToolResult::error()`, activity continues |
| Cancellation | Before each activity | Return `WorkflowError::Cancelled` |
| Exit tool | Propagate | Mark run "exited" (distinct from "failed") |
| Retry exhausted | `on_error.fallback` | Skip / Abort / NotifyOwner |

### Fallback Strategies

```rust
pub enum Fallback {
    Skip,          // Continue to next activity
    Abort,         // Fail entire workflow
    NotifyOwner,   // Same as Abort (notification stubbed)
}
```

---

## 18. Constants & Timings

### Scheduling

| Constant | Value | Location |
|----------|-------|----------|
| Scheduler boot delay | 10s | `scheduler.rs` |
| Heartbeat boot delay | 15s | `heartbeat.rs` |
| Scheduler tick interval | 60s | `scheduler.rs` |
| Heartbeat tick interval | 60s | `heartbeat.rs` |
| Cleanup TTL | 7 days | `scheduler.rs` |

### Workflow Engine

| Constant | Value | Location |
|----------|-------|----------|
| Max iterations per activity | 20 | `engine.rs` |
| EXIT_SENTINEL | `"__EXIT__:"` | `tools` crate |

### Lanes

| Lane | Max Concurrent | Purpose |
|------|---------------|---------|
| `HEARTBEAT` | 1 | Sequential proactive ticks |
| `MAIN` | unlimited | User input |
| `COMM` | unlimited | NeboLoop messages |
| `EVENTS` | unlimited | Event-triggered |

### Origins

| Origin | Used By |
|--------|---------|
| `System` | Cron scheduler, heartbeat |
| `User` | Manual triggers, REST |
| `Comm` | NeboLoop messages |

---

## 19. Known Issues

### Heartbeat vs Role Worker Heartbeat

Two independent heartbeat mechanisms exist:
1. **`heartbeat.rs`** — server-level, entity_config-driven, uses `run_chat()`
2. **`role_worker.rs`** — role-level, `tokio::interval`-driven, uses `run_inline()`

These can potentially overlap if both are configured for the same role. The
entity_config heartbeat uses HEARTBEAT lane (max_concurrent=1), providing natural
dedup for server-level heartbeats, but role worker heartbeats are not gated by the
same mechanism.

### Event Channel Unbounded

`EventBus` uses `mpsc::unbounded_channel()` — no backpressure. In a scenario where
workflow chains produce events faster than they're consumed, memory could grow.

### Scheduler Single-Tick Grid

All cron jobs share a 60s tick — jobs due within the same minute all fire in one
batch. No per-job scheduling granularity below 1 minute.

---

*Last updated: 2026-03-16*
