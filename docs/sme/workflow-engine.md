# Workflow Engine: Implementation SME

**Canonical spec:** [platform-taxonomy.md](platform-taxonomy.md)
**Status:** Infrastructure built (parser, engine, DB, handlers, routes). Execution not yet wired end-to-end.

---

## Current Implementation Status (2026-03-05)

### Built and working

| Component | Where | Status |
|---|---|---|
| **workflow.json parser** | `crates/workflow/src/parser.rs` | Done -- WorkflowDef, Activity, Trigger, ToolRef structs |
| **Workflow engine** | `crates/workflow/src/engine.rs` | Done -- `execute_workflow()` + `execute_activity()` |
| **Trigger registration** | `crates/workflow/src/triggers.rs` | Done -- cron registration via `store.upsert_cron_job()` |
| **DB tables** | `crates/db/migrations/0050_workflows.sql` | Done -- workflows, bindings, runs, activity_results, roles |
| **DB queries** | `crates/db/src/queries/workflows.rs` | Done -- 17 store methods for workflows/runs/bindings/activities |
| **Role queries** | `crates/db/src/queries/roles.rs` | Done -- 7 store methods for role CRUD |
| **Workflow HTTP handlers** | `crates/server/src/handlers/workflows.rs` | Done -- 11 endpoints (CRUD + run + runs + bindings) |
| **Role HTTP handlers** | `crates/server/src/handlers/roles.rs` | Done -- 7 endpoints (CRUD + toggle + install-deps) |
| **ROLE.md parser** | `crates/server/src/handlers/roles.rs` | Done -- YAML frontmatter extraction |
| **Scheduler integration** | `crates/server/src/scheduler.rs` | Done -- `"workflow"` task type handler |
| **Routes** | `crates/server/src/lib.rs` | Done -- 18 routes registered |
| **`implements` manifest field** | `crates/napp/src/manifest.rs` | Done |

### Execution gap: NOT yet wired

The engine code exists (`execute_workflow()` and `execute_activity()`) but the `run_workflow` handler only creates a DB run record and returns it. It does **not** call the engine because three things are missing:

1. **Provider selection** -- `run_workflow` handler needs to pick an `ai::Provider` from the runner's provider list. The runner's providers are private (`Arc<RwLock<Vec<Box<dyn Provider>>>>`). Either expose a provider-lending method on Runner, or pass providers via AppState.

2. **Tool resolution** -- `ToolRef` entries in the workflow definition need to be resolved to actual `DynTool` instances. Code-bound tools need lookup in the napp registry. Interface-bound tools need lookup in the `workflow_tool_bindings` DB table, then resolution to actual tool implementations.

3. **Background execution** -- `execute_workflow()` is async and can run for minutes. The handler should spawn it via `tokio::spawn`, stream progress updates via hub broadcasts, and update the run record on completion.

### Wiring plan (follow-up task)

```rust
// In run_workflow handler:
let provider = state.runner.get_provider_for_model(&activity.model)?;
let tools = resolve_workflow_tools(&wf_def, &state.napp_registry, &state.tools)?;

let store = state.store.clone();
let hub = state.hub.clone();
tokio::spawn(async move {
    let result = workflow::execute_workflow(provider, &tools, &store, &wf_def, inputs).await;
    match result {
        Ok(run) => {
            store.complete_workflow_run(&run_id, "completed", run.tokens_used, None, None);
            hub.broadcast("workflow_run_completed", ...);
        }
        Err(e) => {
            store.complete_workflow_run(&run_id, "failed", 0, Some(&e.to_string()), ...);
            hub.broadcast("workflow_run_failed", ...);
        }
    }
});
```

### Pre-existing infrastructure (leverage these)

| Component | Where | Maps to |
|---|---|---|
| `Runner.Run()` | `crates/agent/src/runner.rs` | Activity worker -- one `Run()` = one activity |
| Cron scheduler | `crates/server/src/scheduler.rs` | `schedule` trigger type |
| Session system | `crates/agent/src/session.rs` | Shared context between activities |
| Skill loader | `crates/tools/src/skill_tool.rs` | Per-activity skill injection source |
| Model selector | `crates/agent/src/selector.rs` | Per-activity model override |
| MCP bridge | `crates/mcp/src/bridge.rs` | Tool interface resolution |

### Still needs building

| Component | Description |
|---|---|
| **Provider access from handlers** | Expose provider selection for workflow execution |
| **Runtime tool resolution** | Resolve ToolRef to DynTool at execution time |
| **Background execution + streaming** | Spawn workflow in background, stream progress via hub |
| **Install code interception** | `WORK-XXXX-XXXX` and `ROLE-XXXX-XXXX` prompt-level detection + NeboLoop SDK calls |
| **NeboLoop REST client** | Download workflows/roles from marketplace (in progress -- comm crate) |
| **Interface binding resolution** | Resolve `{ "interface": "crm-lookup" }` to installed tool at runtime |
| **Per-activity skill injection** | Load specific SKILL.md content into activity prompt context |

---

## Data Structures

### workflow.json (parsed into Rust structs)

```rust
#[derive(Debug, Deserialize, Serialize)]
pub struct WorkflowDef {
    pub version: String,                    // "1.0"
    pub id: String,                         // "lead-qualification"
    pub name: String,                       // "Lead Qualification"
    pub triggers: Vec<Trigger>,
    pub inputs: HashMap<String, InputParam>,
    pub activities: Vec<Activity>,
    pub dependencies: Dependencies,
    pub budget: Budget,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum Trigger {
    #[serde(rename = "event")]
    Event { event: String },
    #[serde(rename = "schedule")]
    Schedule { cron: String },
    #[serde(rename = "manual")]
    Manual,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct InputParam {
    #[serde(rename = "type")]
    pub param_type: String,                 // "string", "integer", etc.
    pub required: bool,
    pub default: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Activity {
    pub id: String,                         // "lookup"
    pub intent: String,                     // "Find existing contact record"
    #[serde(default)]
    pub skills: Vec<String>,                // ["SKILL-sales-qualification"]
    pub tools: Vec<ToolRef>,                // code or interface binding
    pub model: String,                      // "haiku", "sonnet", "opus"
    pub steps: Vec<String>,                 // natural-language instructions
    pub token_budget: TokenBudget,
    #[serde(default)]
    pub on_error: OnError,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ToolRef {
    Code(String),                           // "TOOL-A1B2-C3D4-E5F6"
    Interface { interface: String },        // { "interface": "crm-lookup" }
    Pinned { code: String },                // { "code": "TOOL-A1B2-C3D4-E5F6" }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TokenBudget {
    pub max: u32,                           // max tokens for this activity
}

#[derive(Debug, Deserialize, Serialize)]
pub struct OnError {
    #[serde(default = "default_retry")]
    pub retry: u32,                         // default: 1
    #[serde(default = "default_fallback")]
    pub fallback: Fallback,                 // default: notify_owner
}

fn default_retry() -> u32 { 1 }
fn default_fallback() -> Fallback { Fallback::NotifyOwner }

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Fallback {
    NotifyOwner,
    Skip,
    Abort,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Dependencies {
    pub skills: Vec<String>,
    pub tools: Vec<ToolDep>,
    #[serde(default)]
    pub workflows: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ToolDep {
    pub code: String,
    pub name: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Budget {
    pub total_per_run: u32,                 // sum of all activity token_budget.max
    pub cost_estimate: String,              // "$0.0043"
}
```

### Workflow Run State

```rust
pub struct WorkflowRun {
    pub id: String,                         // UUID
    pub workflow_id: String,                // "lead-qualification"
    pub session_key: String,                // shared session for context passing
    pub trigger: Trigger,                   // what triggered this run
    pub inputs: HashMap<String, serde_json::Value>,
    pub status: WorkflowStatus,
    pub current_activity: Option<String>,   // activity.id
    pub activity_results: Vec<ActivityResult>,
    pub total_tokens_used: u32,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub enum WorkflowStatus {
    Running,
    Completed,
    Failed { error: String, activity_id: String },
    Aborted,
}

pub struct ActivityResult {
    pub activity_id: String,
    pub status: ActivityStatus,
    pub tokens_used: u32,
    pub attempts: u32,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub completed_at: chrono::DateTime<chrono::Utc>,
}

pub enum ActivityStatus {
    Completed,
    Skipped,
    Failed { error: String },
}
```

### Manifest Extension for `implements`

```rust
// Add to existing AppManifest in crates/apps/src/manifest.rs
pub struct AppManifest {
    // ... existing fields ...
    #[serde(default)]
    pub implements: Vec<String>,            // ["crm-lookup", "contact-search"]
}
```

### Tool Interface Registry

```rust
/// Maps abstract interfaces to installed tools that satisfy them
pub struct InterfaceRegistry {
    /// interface_name -> Vec<tool_code> (multiple tools may implement same interface)
    bindings: HashMap<String, Vec<String>>,
    /// workflow_id -> (interface_name -> chosen_tool_code) (user's binding choices)
    user_bindings: HashMap<String, HashMap<String, String>>,
}
```

---

## Execution Algorithm

### Design Principle: Hyper-Controlled Token Usage

Workflow execution is **NOT** the full agentic loop. The agent does NOT get steering messages, memory injection, personality context, or freestyle tool discovery. The workflow engine hands the LLM exactly:

1. **The prompt** -- constructed from the activity's intent + steps + skill context
2. **The tools** -- only the tools declared for this activity
3. **Nothing else**

This is a lean execution path. No steering pipeline, no memory refresh, no fuzzy model matching, no proactive behavior. The workflow designer controls exactly what the LLM sees and what it can do. Token usage is deterministic and predictable.

### `execute_workflow()`

```
fn execute_workflow(def: WorkflowDef, inputs: HashMap, trigger: Trigger):
    1. Create WorkflowRun with UUID
    2. Create shared session (key: "workflow-{workflow_id}-{run_id}")
    3. Resolve all tool references:
       - Code bindings → verify tool is installed
       - Interface bindings → lookup InterfaceRegistry.user_bindings
       - If unresolved interface → error (prompt user to configure binding)

    4. FOR EACH activity in def.activities (sequential):
       a. Build the EXACT prompt (no extras):
          - System message constructed from:
            * Activity skill content (raw SKILL.md text, loaded per-activity)
            * Activity intent: "{activity.intent}"
            * Activity steps: "1. {step1}\n2. {step2}\n..."
            * Prior activity outputs (from shared session, if any)
            * Workflow inputs (injected as context)
          - NO steering messages
          - NO memory context
          - NO personality/soul documents
          - NO identity guard, tool nudges, or any generator output
          - NO advisor consultation

       b. Build the EXACT tool set (no extras):
          - ONLY tools declared in activity.tools
          - No MCP proxy tools unless explicitly listed
          - No platform tools unless explicitly listed
          - No skill/bot/event/memory tools -- only declared tools

       c. Select the EXACT model:
          - Map activity.model ("haiku"/"sonnet"/"opus") directly
          - No fallback, no routing heuristic, no selector logic
          - If the specified model is unavailable → fail the activity

       d. Execute with hard token ceiling:
          - Call provider.chat() directly with:
            * messages: [system_prompt, ...prior_activity_context]
            * tools: resolved_activity_tools
            * max_tokens: activity.token_budget.max
          - Agent loops: tool call → tool result → next LLM call
          - Hard stop at token_budget.max (input + output combined)
          - Hard stop at max_iterations (prevent runaway loops)

       e. Track token usage:
          - Count input tokens per LLM call
          - Count output tokens per LLM call
          - Sum for activity total
          - Compare against token_budget.max
          - If budget exceeded mid-activity → terminate, record partial

       f. On success → record ActivityResult (tokens_used, duration)
       g. On error → apply on_error policy:
          - If attempts < retry → retry from step (d)
          - If retry exhausted:
            - fallback == "notify_owner" → send notification, mark failed
            - fallback == "skip" → record skip, continue to next
            - fallback == "abort" → mark workflow failed, stop

    5. Sum all activity token usage → workflow total
    6. Compare against budget.total_per_run
    7. Record run history with exact cost breakdown
```

### Two execution paths in nebo-rs

The Rust codebase will have TWO distinct execution paths:

| | Agentic (existing Runner.Run()) | Workflow Activity |
|---|---|---|
| **Prompt** | Full: identity, personality, steering, memory, advisors | Lean: intent + steps + skills only |
| **Tools** | All registered tools + MCP + platform | Only declared activity tools |
| **Model** | Selector with fallback + routing | Exact model, no fallback |
| **Token control** | Context window limit (soft) | Hard budget ceiling (strict) |
| **Memory** | Injected, extracted, refreshed | None -- no extraction, no injection |
| **Steering** | 12 generators fire per iteration | None |
| **Advisors** | Consulted based on config | None |
| **Session** | Full compaction, pruning, recovery | Minimal -- append-only within run |
| **Cost** | Unpredictable | Deterministic per-run |

### Why two paths

The agentic loop is designed for open-ended conversation where the agent needs autonomy, personality, and context. Workflows are designed for repeatable jobs where cost predictability and tool control matter more than agent creativity. These are fundamentally different use cases and should not share the same execution path.

### Implementation: `execute_activity()` function

This is NOT `Runner.Run()`. It is a new, lean function:

```rust
async fn execute_activity(
    activity: &Activity,
    session: &mut WorkflowSession,
    tools: &[Box<dyn DynTool>],
    provider: &dyn Provider,
    inputs: &HashMap<String, serde_json::Value>,
) -> Result<ActivityResult, WorkflowError> {
    let mut tokens_used: u32 = 0;
    let mut iterations: u32 = 0;
    const MAX_ITERATIONS: u32 = 20; // much lower than Runner's 100

    // Build system prompt from skills + intent + steps
    let system = build_activity_prompt(activity, inputs);

    // Initialize messages with system prompt + prior context from session
    let mut messages = session.messages_for_activity(&activity.id);
    messages.insert(0, Message::system(system));

    loop {
        if iterations >= MAX_ITERATIONS {
            return Err(WorkflowError::MaxIterations(activity.id.clone()));
        }
        if tokens_used >= activity.token_budget.max {
            return Err(WorkflowError::BudgetExceeded {
                activity_id: activity.id.clone(),
                used: tokens_used,
                limit: activity.token_budget.max,
            });
        }

        // Call LLM with exact tools and remaining budget
        let remaining = activity.token_budget.max - tokens_used;
        let response = provider.chat(ChatRequest {
            messages: messages.clone(),
            tools: tools.iter().map(|t| t.schema()).collect(),
            max_tokens: remaining.min(4096), // cap single response
            model: activity.model.clone(),
        }).await?;

        tokens_used += response.input_tokens + response.output_tokens;
        iterations += 1;

        // Append assistant response to session
        session.append(activity.id.clone(), response.message.clone());

        // If no tool calls, activity is complete
        if response.tool_calls.is_empty() {
            break;
        }

        // Execute tool calls, append results
        for call in &response.tool_calls {
            let tool = tools.iter().find(|t| t.name() == call.name)
                .ok_or(WorkflowError::ToolNotFound(call.name.clone()))?;
            let result = tool.execute(call.arguments.clone()).await?;
            session.append_tool_result(activity.id.clone(), call.id.clone(), result);
        }

        // Rebuild messages with tool results for next iteration
        messages = session.messages_for_activity(&activity.id);
    }

    Ok(ActivityResult {
        activity_id: activity.id.clone(),
        status: ActivityStatus::Completed,
        tokens_used,
        attempts: 1,
        started_at: todo!(),
        completed_at: todo!(),
    })
}
```

### Key design decisions:

1. **Lean prompt, no extras** -- The LLM gets intent + steps + skills. No personality, no steering, no memory. The workflow designer controls every token of context.

2. **Exact tools, no discovery** -- Only declared tools are available. The agent cannot discover or use tools not in the activity definition.

3. **Exact model, no fallback** -- The workflow designer picked the model. If it's unavailable, the activity fails. No silent downgrade to a cheaper model.

4. **Hard token ceiling** -- `token_budget.max` is enforced strictly. If exceeded, the activity terminates. This makes per-run cost deterministic.

5. **MAX_ITERATIONS = 20** -- Much lower than the agentic loop's 100. Workflows are bounded procedures, not open-ended exploration.

6. **Shared session for context flow** -- Activities share a session so the `assess` activity can see what `lookup` found. But no compaction, no pruning, no memory extraction.

7. **No memory extraction** -- Workflows do not learn. They execute and report. Memory is for the agentic loop.

8. **Separate function, not Runner.Run()** -- This is a new `execute_activity()` function, not a mode flag on the existing runner. Clean separation.

---

## Trigger System

### Mapping to existing infrastructure

```
Trigger::Event { event } → Events system (internal/events/)
    - Register event listener: on("new_contact_form_submission", run_workflow)
    - When event fires, enqueue workflow on events lane

Trigger::Schedule { cron } → Cron scheduler (crates/server/src/scheduler.rs)
    - Register cron job with task_type = "workflow"
    - Scheduler polls, fires workflow on cron lane

Trigger::Manual → Direct invocation
    - User sends message or clicks "Run" in UI
    - Enqueue workflow on main lane
```

### New: Workflow-aware trigger registration

When a workflow is installed, its triggers must be registered:
- Schedule triggers → create cron_jobs row
- Event triggers → register with events system
- Manual triggers → no registration needed (on-demand)

When a workflow is uninstalled, triggers must be cleaned up.

---

## Database Schema

### New tables needed

```sql
-- Installed workflows
CREATE TABLE workflows (
    id TEXT PRIMARY KEY,               -- "lead-qualification"
    code TEXT UNIQUE,                  -- "WORK-A1B2-C3D4-E5F6"
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    definition TEXT NOT NULL,          -- workflow.json content
    skill_md TEXT,                     -- SKILL.md content
    manifest TEXT,                     -- manifest.json content
    installed_at TEXT DEFAULT (datetime('now')),
    enabled INTEGER DEFAULT 1
);

-- Tool interface bindings (user's choices)
CREATE TABLE workflow_tool_bindings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    workflow_id TEXT NOT NULL REFERENCES workflows(id),
    interface_name TEXT NOT NULL,      -- "crm-lookup"
    tool_code TEXT NOT NULL,           -- "TOOL-A1B2-C3D4-E5F6"
    UNIQUE(workflow_id, interface_name)
);

-- Workflow run history
CREATE TABLE workflow_runs (
    id TEXT PRIMARY KEY,               -- UUID
    workflow_id TEXT NOT NULL REFERENCES workflows(id),
    trigger_type TEXT NOT NULL,        -- "event", "schedule", "manual"
    trigger_detail TEXT,               -- event name or cron expression
    status TEXT NOT NULL,              -- "running", "completed", "failed", "aborted"
    inputs TEXT,                       -- JSON
    current_activity TEXT,
    total_tokens_used INTEGER DEFAULT 0,
    total_cost_estimate TEXT,
    error TEXT,
    error_activity TEXT,
    session_key TEXT,
    started_at TEXT DEFAULT (datetime('now')),
    completed_at TEXT
);

-- Per-activity results within a run
CREATE TABLE workflow_activity_results (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL REFERENCES workflow_runs(id),
    activity_id TEXT NOT NULL,
    status TEXT NOT NULL,              -- "completed", "skipped", "failed"
    tokens_used INTEGER DEFAULT 0,
    attempts INTEGER DEFAULT 1,
    error TEXT,
    started_at TEXT NOT NULL,
    completed_at TEXT NOT NULL
);

-- Installed roles (marketplace bundles)
CREATE TABLE roles (
    id TEXT PRIMARY KEY,               -- "client-intake-specialist"
    code TEXT UNIQUE,                  -- "ROLE-7K3M-Q9P2-X4W1"
    name TEXT NOT NULL,
    role_md TEXT NOT NULL,             -- ROLE.md content
    installed_at TEXT DEFAULT (datetime('now'))
);
```

---

## API Routes

### New endpoints needed

```
-- Workflow CRUD
GET    /api/v1/workflows                    -- List installed workflows
POST   /api/v1/workflows                    -- Install workflow (from code or upload)
GET    /api/v1/workflows/:id                -- Get workflow details
PUT    /api/v1/workflows/:id                -- Update workflow
DELETE /api/v1/workflows/:id                -- Uninstall workflow
POST   /api/v1/workflows/:id/toggle         -- Enable/disable
POST   /api/v1/workflows/:id/run            -- Manual trigger
GET    /api/v1/workflows/:id/runs           -- Run history
GET    /api/v1/workflows/:id/runs/:runId    -- Specific run details

-- Tool interface binding
GET    /api/v1/workflows/:id/bindings       -- List interface bindings
PUT    /api/v1/workflows/:id/bindings       -- Set interface binding choices

-- Role resolution
POST   /api/v1/roles/install                -- Install from ROLE code
GET    /api/v1/roles                        -- List installed roles

-- Store (extend existing)
GET    /api/v1/store/workflows              -- Browse marketplace workflows
GET    /api/v1/store/roles                  -- Browse marketplace roles
```

---

## Install Code Handling

### New code prefixes to add

```rust
fn detect_install_code(input: &str) -> Option<InstallCode> {
    let trimmed = input.trim().to_uppercase();

    // Existing codes (from Go)
    if trimmed.starts_with("NEBO-") && trimmed.len() == 19 { return Some(NeboLoop(trimmed)); }
    if trimmed.starts_with("LOOP-") && trimmed.len() == 19 { return Some(Loop(trimmed)); }
    if trimmed.starts_with("SKILL-") && trimmed.len() == 20 { return Some(Skill(trimmed)); }

    // Renamed: APP- → TOOL- (support both for backwards compat)
    if trimmed.starts_with("APP-") && trimmed.len() == 18 { return Some(Tool(trimmed)); }
    if trimmed.starts_with("TOOL-") && trimmed.len() == 19 { return Some(Tool(trimmed)); }

    // New codes
    if trimmed.starts_with("WORK-") && trimmed.len() == 19 { return Some(Workflow(trimmed)); }
    if trimmed.starts_with("ROLE-") && trimmed.len() == 19 { return Some(Role(trimmed)); }

    None
}

enum InstallCode {
    NeboLoop(String),
    Loop(String),
    Skill(String),
    Tool(String),
    Workflow(String),
    Role(String),
}
```

### Install flow per code type

**WORK-XXXX-XXXX-XXXX:**
1. Fetch workflow from NeboLoop API: `GET /api/v1/workflows/{code}`
2. Parse workflow.json, SKILL.md, manifest.json
3. Resolve dependencies:
   - For each skill in `dependencies.skills` → `InstallSkill()` if not installed
   - For each tool in `dependencies.tools` → `InstallApp()` if not installed
   - For each workflow in `dependencies.workflows` → recursive install
4. Check interface bindings:
   - For each activity, for each tool ref that is interface-bound:
     - Check if user has a binding for this interface
     - If not → prompt user to choose from compatible installed tools
5. Store workflow definition in `workflows` table
6. Register triggers (cron jobs, event listeners)
7. Confirm to user

**ROLE-XXXX-XXXX-XXXX:**
1. Fetch ROLE.md from NeboLoop API: `GET /api/v1/roles/{code}`
2. Parse YAML frontmatter to extract `workflows`, `tools`, `skills`
3. For each workflow code → install via WORK- flow above
4. Store ROLE.md in `roles` table
5. Confirm to user

---

## Crate Organization

### Option A: New `workflow` crate (recommended)

```
crates/
├── workflow/
│   ├── src/
│   │   ├── lib.rs          -- pub exports
│   │   ├── parser.rs       -- workflow.json + ROLE.md parsing
│   │   ├── engine.rs       -- execute_workflow() orchestration
│   │   ├── resolver.rs     -- dependency resolution + interface binding
│   │   ├── triggers.rs     -- trigger registration/dispatch
│   │   ├── budget.rs       -- per-activity token budget enforcement
│   │   └── history.rs      -- run history recording
│   └── Cargo.toml
```

**Dependencies:**
- `agent` -- for `Runner.Run()`
- `tools` -- for tool registry and filtering
- `db` -- for workflow/run storage
- `config` -- for model mapping
- `apps` -- for manifest with `implements`

### Integration points

1. **Server** (`crates/server/src/lib.rs`):
   - Add workflow routes
   - Initialize workflow engine with dependencies

2. **Agent runner** (`crates/agent/src/runner.rs`):
   - Add `RunRequest.token_limit: Option<u32>` for per-activity budget
   - Add `RunRequest.tool_filter: Option<Vec<String>>` for per-activity tools
   - Add `RunRequest.model_override: Option<String>` for per-activity model

3. **Apps manifest** (`crates/apps/src/manifest.rs`):
   - Add `implements: Vec<String>` field

4. **Scheduler** (`crates/server/src/scheduler.rs`):
   - Add `task_type = "workflow"` support

5. **Install code detection** (new, in CLI or agent):
   - WORK- and ROLE- prefix handling

---

## What NOT to Build in v1

Per the taxonomy doc:
- **No `InstallRole()` runtime method** -- Roles are resolved to their constituent workflows at install time
- **No mid-workflow async wait** -- Decompose into separate trigger-linked workflows
- **No cross-provider-family workflows** -- v1 constrains to single provider family per workflow
- **No workflow editor UI** -- Workflow Specialists author workflow.json by hand or with tools
- **No dynamic tool interface resolution at runtime** -- Interface bindings are set at install time, not per-run

---

## Migration Notes

### From Go

The Go codebase has **zero workflow code**. This is entirely new functionality. However, the Go codebase has infrastructure that the workflow engine depends on:

| Go Infrastructure | Status in Rust | Needed for Workflows |
|---|---|---|
| Lane system | Not yet ported | Yes -- activity queuing |
| Orchestrator | Not yet ported | Yes -- sub-workflow invocation |
| Events system | Not yet ported | Yes -- event triggers |
| Cron scheduler | Ported | Yes -- schedule triggers |
| Runner.Run() | Ported | Yes -- activity execution |
| Session system | Ported | Yes -- shared context |
| Skill loader | Partially ported | Yes -- per-activity skills |
| Model selector | Ported | Yes -- per-activity model |
| NeboLoop store | Not yet ported | Yes -- marketplace install |
| Install codes | Not yet ported | Yes -- WORK/ROLE codes |

**Recommendation:** Port the lane system and orchestrator FIRST, then build the workflow engine on top.

---

*Generated: 2026-03-04*
