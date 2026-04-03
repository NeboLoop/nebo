# Event System — SME Reference

Complete Subject Matter Expert document covering the Nebo event system: EventBus,
EventDispatcher, emit tool, event-triggered workflows, system-emitted events, pattern
matching, event chaining, and integration with the broader automation pipeline.

**Status:** Current (Rust implementation) | **Last updated:** 2026-03-24

**Related docs:** [AUTOMATION_SME.md](AUTOMATION_SME.md) (covers cron scheduler,
heartbeat system, workflow engine, and frontend). This document goes deeper on the
event subsystem specifically.

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [Event Schema](#2-event-schema)
3. [EventBus](#3-eventbus)
4. [EmitTool](#4-emittool)
5. [EventDispatcher](#5-eventdispatcher)
6. [Pattern Matching](#6-pattern-matching)
7. [Event Subscriptions](#7-event-subscriptions)
8. [Subscription Registration](#8-subscription-registration)
9. [Event-Triggered Workflow Execution](#9-event-triggered-workflow-execution)
10. [Event Chaining](#10-event-chaining)
11. [System-Emitted Events](#11-system-emitted-events)
12. [MCP Server Event Emission](#12-mcp-server-event-emission)
13. [EventTool (Cron/Task Management)](#13-eventtool-crontask-management)
14. [Event Sources Discovery API](#14-event-sources-discovery-api)
15. [Frontend Integration](#15-frontend-integration)
16. [Database Schema](#16-database-schema)
17. [Boot Sequence](#17-boot-sequence)
18. [Code Paths That Register Event Triggers](#18-code-paths-that-register-event-triggers)
19. [Code Paths That Emit Events](#19-code-paths-that-emit-events)
20. [Naming Conventions](#20-naming-conventions)
21. [Gotchas & Known Issues](#21-gotchas--known-issues)
22. [Relationship to Other Systems](#22-relationship-to-other-systems)
23. [File Reference](#23-file-reference)

---

## 1. Architecture Overview

```
  EMITTERS                              CONSUMERS
  ────────                              ─────────

  ┌──────────────┐
  │  EmitTool    │  (auto-injected into every workflow activity)
  │  emit()      │──┐
  └──────────────┘  │
                    │
  ┌──────────────┐  │    ┌──────────┐        ┌─────────────────┐
  │  NeboLoop    │  ├───>│ EventBus │───────> │ EventDispatcher │
  │  comm msgs   │──┤    │ (mpsc)   │        │ (pattern match) │
  └──────────────┘  │    └──────────┘        └────────┬────────┘
                    │                                 │
  ┌──────────────┐  │                        For each matching subscription:
  │  MCP Server  │──┘                                 │
  │  emit_event  │                                    ▼
  └──────────────┘                      ┌──────────────────────────┐
                                        │  WorkflowManager         │
                                        │  .run_inline()           │
                                        │                          │
                                        │  → Workflow Engine        │
                                        │  → Activities execute     │
                                        │  → May emit more events  │──── event chain
                                        └──────────────────────────┘
```

**Key principle:** The event system is fire-and-forget, in-memory, with no persistence.
Events flow through an unbounded MPSC channel. The EventDispatcher pattern-matches
against in-memory subscriptions and triggers inline workflow execution.

---

## 2. Event Schema

**File:** `crates/tools/src/events.rs`

```rust
pub struct Event {
    pub source: String,              // e.g. "email.urgent", "neboloop.chat"
    pub payload: serde_json::Value,  // Arbitrary JSON
    pub origin: String,              // Trace: "workflow:email-triage:run-550e" or "neboloop"
    pub timestamp: u64,              // Unix epoch seconds
}
```

| Field | Purpose | Set By |
|-------|---------|--------|
| `source` | Event type identifier, used for pattern matching | Emitter |
| `payload` | Arbitrary data passed to triggered workflows | Emitter |
| `origin` | Trace provenance — session key for emit tool, "neboloop" for comm, "mcp" for MCP | System |
| `timestamp` | Unix epoch seconds when event was created | System |

The `source` field is the primary routing key. The EventDispatcher matches it against
subscription patterns.

---

## 3. EventBus

**File:** `crates/tools/src/events.rs`

```rust
pub struct EventBus {
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
}
```

- Created at server startup (`crates/server/src/lib.rs:733`)
- Returns `(EventBus, UnboundedReceiver<Event>)` — bus is cloneable, receiver is single
- **Best-effort delivery**: `emit()` silently drops events if receiver is gone
- **No backpressure**: Unbounded channel — producers never block
- **No persistence**: Events exist only in memory, lost on restart
- **No deduplication**: Same event emitted twice will be dispatched twice

The `EventBus` instance is:
1. Stored in `AppState` (for system emitters)
2. Cloned into `EmitTool` instances (for workflow activity emitters)
3. The receiver half is consumed by `EventDispatcher::spawn()`

---

## 4. EmitTool

**File:** `crates/tools/src/emit_tool.rs`

```rust
pub struct EmitTool {
    bus: EventBus,
}
```

**Tool schema:**
```json
{
    "source": "string (required) — Event source identifier",
    "payload": "object (optional) — Arbitrary event data"
}
```

**Behavior:**
1. Validates source is non-empty
2. Captures current UNIX timestamp
3. Uses `ctx.session_key` as the `origin` trace
4. Sends to EventBus
5. Returns `"Event emitted: {source}"`

**Injection:** Auto-injected into every workflow activity by the engine
(`crates/workflow/src/engine.rs:124`). Activities do NOT need to declare it in their
`tools` array. It is always available alongside `exit_tool`.

**Approval:** `requires_approval() = false` — emit is a side-effect-free action from
the LLM's perspective (the downstream trigger is asynchronous).

---

## 5. EventDispatcher

**File:** `crates/workflow/src/events.rs`

```rust
pub struct EventDispatcher {
    subscriptions: Arc<RwLock<Vec<EventSubscription>>>,
}
```

### Spawn Loop

```rust
pub fn spawn(
    self: Arc<Self>,
    rx: UnboundedReceiver<Event>,
    manager: Arc<dyn WorkflowManager>,
) -> JoinHandle<()>
```

Started at `crates/server/src/lib.rs:806`. Consumes from the EventBus receiver:

```
while event = rx.recv():
    matches = self.match_event(event)
    for sub in matches:
        merge _event_source, _event_payload, _event_origin into sub.default_inputs
        if sub.definition_json exists:
            manager.run_inline(def, inputs, "event", sub.agent_source, sub.emit_source)
        else:
            warn "no inline definition, skipping"
```

### Subscription Management

| Method | Description |
|--------|-------------|
| `set_subscriptions(subs)` | Replace all (used during bulk reload) |
| `subscribe(sub)` | Add one subscription |
| `unsubscribe_binding(agent_id, binding_name)` | Remove by agent + binding |
| `unsubscribe_agent(agent_id)` | Remove all for an agent |
| `clear()` | Remove everything |
| `match_event(event)` | Find matching subscriptions (read lock) |

All mutations take a write lock on `subscriptions`. Matching takes a read lock.

---

## 6. Pattern Matching

**Function:** `source_matches(pattern, source)` in `crates/workflow/src/events.rs:146`

Two matching modes:

| Mode | Pattern | Source | Match? |
|------|---------|--------|--------|
| Exact | `"email.urgent"` | `"email.urgent"` | Yes |
| Exact | `"email.urgent"` | `"email.info"` | No |
| Wildcard | `"email.*"` | `"email.urgent"` | Yes |
| Wildcard | `"email.*"` | `"email.info"` | Yes |
| Wildcard | `"email.*"` | `"calendar.changed"` | No |
| Wildcard | `"email.*"` | `"emailurgent"` | No (requires `.` separator) |

**Implementation:**
```rust
fn source_matches(pattern: &str, source: &str) -> bool {
    if pattern == source { return true; }
    if let Some(prefix) = pattern.strip_suffix(".*") {
        return source.starts_with(prefix) && source[prefix.len()..].starts_with('.');
    }
    false
}
```

**Limitations:**
- Only suffix wildcards (`"email.*"`) — no mid-pattern wildcards (`"*.urgent"`)
- No multi-level wildcards (`"email.**"` or `"email.*.important"`)
- No regex patterns
- Case-sensitive matching

---

## 7. Event Subscriptions

```rust
pub struct EventSubscription {
    pub pattern: String,                   // "email.*" or "email.urgent"
    pub default_inputs: serde_json::Value, // Merged with event data
    pub agent_source: String,              // Agent ID that owns this
    pub binding_name: String,              // Workflow binding name
    pub definition_json: Option<String>,   // Inline workflow JSON
    pub emit_source: Option<String>,       // "{agent-slug}.{emit-name}" for chaining
}
```

| Field | Source |
|-------|--------|
| `pattern` | From `trigger_config` (comma-split, one subscription per source) |
| `default_inputs` | From `agent_workflows.inputs` JSON column |
| `agent_source` | Agent ID |
| `binding_name` | Workflow binding name |
| `definition_json` | Generated via `WorkflowBinding::to_workflow_json()` from agent.json |
| `emit_source` | Built as `"{agent-slug}.{emit-name}"` if binding has `emit` field |

Subscriptions are **in-memory only**. They are rebuilt from the database on:
- Server boot (via `process_agent_bindings`)
- Agent install/update
- Agent worker start
- Binding create/update/toggle

---

## 8. Subscription Registration

### Three Code Paths

There are three independent paths that register event subscriptions. All converge on
`EventDispatcher::subscribe()`.

#### Path 1: Agent Install/Update via `process_agent_bindings`

**File:** `crates/server/src/handlers/agents.rs:572`

Called when an agent is installed or its config is re-processed. Iterates all bindings
from the parsed `AgentConfig`, upserts to `agent_workflows` table, then:

```
for each binding where trigger_type == "event":
    parse WorkflowBinding from config
    build definition_json via to_workflow_json()
    build emit_source from "{agent-slug}.{emit-name}"
    split trigger_config by comma → one EventSubscription per source pattern
    dispatcher.subscribe(sub)
```

#### Path 2: Single Binding CRUD via `register_binding_triggers`

**File:** `crates/server/src/handlers/agents.rs:1434`

Called from `create_agent_workflow`, `update_agent_workflow`, and `toggle_agent_workflow`
HTTP handlers. Registers a single binding's triggers:

```
if trigger_type == "schedule":
    upsert_cron_job(...)
elif trigger_type == "event":
    parse WorkflowBinding from frontmatter
    build definition_json and emit_source
    for each source in trigger_config.split(','):
        dispatcher.subscribe(EventSubscription { ... })
```

#### Path 3: Agent Worker Start

**File:** `crates/agent/src/agent_worker.rs`

When an AgentWorker starts for an agent, it registers event subscriptions as part of
its lifecycle setup. Uses `process_agent_bindings` internally.

### Subscription Cleanup

| Event | Cleanup Method |
|-------|---------------|
| Agent uninstall | `dispatcher.unsubscribe_agent(agent_id)` |
| Binding delete | `dispatcher.unsubscribe_binding(agent_id, binding)` |
| Trigger type change | `unsubscribe_binding()` then re-register |
| Agent worker stop | `dispatcher.unsubscribe_agent(agent_id)` |
| Binding toggle off | `unsubscribe_binding()` |

---

## 9. Event-Triggered Workflow Execution

When the dispatcher matches an event against a subscription:

### Step 1: Input Merging

```rust
let mut inputs = sub.default_inputs.clone();
inputs["_event_source"] = json!(event.source);
inputs["_event_payload"] = event.payload.clone();
inputs["_event_origin"] = json!(event.origin);
```

The `_` prefix keys are reserved — the automation editor frontend strips them from
user-defined inputs.

### Step 2: Inline Workflow Execution

```rust
manager.run_inline(
    def_json,           // from WorkflowBinding::to_workflow_json()
    inputs,             // merged event data + defaults
    "event",            // trigger_type
    &sub.agent_source,  // agent_id
    sub.emit_source,    // for chaining
).await
```

### Step 3: WorkflowManager Spawns Background Task

**File:** `crates/server/src/workflow_manager.rs`

`run_inline()` creates a `workflow_runs` record, spawns a tokio task, and returns
the `run_id` immediately. The background task:

1. Loads AI provider
2. Resolves tools from the registry
3. Calls `workflow::engine::execute_workflow()`
4. Posts automation status messages to the agent's chat session
5. Updates `last_fired` on the `agent_workflows` row
6. Records completion/failure in `workflow_runs`
7. Broadcasts WebSocket status events

### Step 4: Activity Execution

Each activity in the workflow definition runs sequentially:
- System prompt built from: intent + steps + skills + inputs + prior activity results
- Hard token budget per activity
- Max 20 iterations per activity
- EmitTool and ExitTool always injected
- On error: retry up to `on_error.retry`, then apply fallback (Skip/Abort/NotifyOwner)

---

## 10. Event Chaining

Workflow A can trigger Workflow B through events:

```
  Agent: Chief of Staff             Agent: Content Writer
  ┌──────────────────┐              ┌──────────────────┐
  │ morning-briefing │              │ draft-summary    │
  │                  │              │                  │
  │ Activity 1       │              │ trigger:         │
  │ Activity 2       │              │   type: event    │
  │   emit_tool →    │──event──────>│   sources:       │
  │   "briefing.done"│              │   - "chief-of-*" │
  │                  │              │                  │
  │ emit: "done"     │              │ Activity 1       │
  └──────────────────┘              └──────────────────┘
```

### Emit Source Namespacing

The `emit` field in agent.json is a short name (e.g. `"done"`). At runtime it's
namespaced with the agent slug:

```
emit_source = "{agent-slug}.{emit-name}"
            = "chief-of-staff.done"
```

This becomes the `source` field of the emitted event, matching the subscribing
agent's pattern (`"chief-of-*"` or `"chief-of-staff.done"`).

### Chain Depth

There is no explicit chain depth limit. A workflow triggered by an event can emit
another event, which triggers another workflow, and so on. The practical limit is:
- Token budgets (each workflow has a `total_per_run` budget)
- The EventBus unbounded channel (events pile up in memory)
- No cycle detection — a cycle would run until budgets are exhausted

---

## 11. System-Emitted Events

The server emits events directly into the EventBus (not via EmitTool) for external
messages arriving via NeboLoop:

**File:** `crates/server/src/lib.rs`

### NeboLoop Agent Space Messages (line 1172)

```rust
source: format!("neboloop.agent_space.{}", agent_slug),
payload: { from, content, conversation_id, agent_slug },
origin: "neboloop",
```

Fired when an agent-to-agent message arrives via the NeboLoop broker.

### NeboLoop Chat/DM Messages (line 1246)

```rust
source: format!("neboloop.{}", msg.topic),  // e.g. "neboloop.chat", "neboloop.dm"
payload: { from, content, conversation_id },
origin: "neboloop",
```

Fired after the message has been dispatched to `run_chat()`. This allows agent
event triggers to react to inbound NeboLoop messages.

### NeboLoop Other Topic Messages (line 1263)

```rust
source: format!("neboloop.{}", msg.topic),
payload: { from, content, topic },
origin: "neboloop",
```

Catch-all for non-chat message types (e.g. webhooks, notifications).

### Pattern Summary

| Source Pattern | When Emitted |
|----------------|--------------|
| `neboloop.agent_space.{slug}` | Agent-to-agent message |
| `neboloop.chat` | Chat message from NeboLoop |
| `neboloop.dm` | DM from NeboLoop |
| `neboloop.{topic}` | Any other NeboLoop topic |

Agent event triggers can subscribe to these patterns. Example:
```json
{
    "trigger": { "type": "event", "sources": ["neboloop.chat"] }
}
```

---

## 12. MCP Server Event Emission

**File:** `crates/server/src/handlers/mcp_server.rs:345`

The MCP server exposes an `emit_event` tool that allows external MCP clients to
inject events into the EventBus:

```rust
source: user-provided,
payload: user-provided,
origin: "mcp",
```

This is the primary way external systems (webhooks, integrations) can trigger
event-based workflows.

---

## 13. EventTool (Cron/Task Management)

**File:** `crates/tools/src/event_tool.rs`

**IMPORTANT:** Despite the name, `EventTool` is NOT part of the event system. It
manages cron jobs / scheduled tasks. It is the agent-facing tool for:

- `create` — Schedule a new cron job (cron expression or relative time)
- `list` — List all cron jobs
- `delete` — Remove by name
- `pause` / `resume` — Enable/disable
- `run` — Immediately execute
- `history` — Show execution log

The naming is a historical artifact. The EventTool operates on the `cron_jobs` table,
not the EventBus.

---

## 14. Event Sources Discovery API

**Endpoint:** `GET /api/v1/agents/event-sources`

Returns all emit sources from active agent workflow bindings:

```sql
SELECT rw.emit, a.name, rw.binding_name, rw.description
FROM agent_workflows rw
JOIN agents a ON rw.agent_id = a.id
WHERE rw.emit IS NOT NULL AND rw.emit != '' AND rw.is_active = 1;
```

**Response model:**
```rust
pub struct EmitSource {
    pub emit: String,           // "briefing.ready"
    pub agent_name: String,     // "Chief of Staff"
    pub binding_name: String,   // "morning-briefing"
    pub description: Option<String>,
}
```

Used by the frontend AutomationEditor to show available events when configuring
event triggers. Helps users discover what events other workflows emit.

---

## 15. Frontend Integration

### Automation Editor — Event Trigger

**File:** `app/src/lib/components/agent/AutomationEditor.svelte`

When trigger type = "Event":
- `TagInput` component for entering source patterns
- Lazy-loads available event sources via `listEventSources()` API
- Shows "emitted by: {agent-name} / {binding-name}" annotations
- Multiple sources supported (stored as comma-separated in `trigger_config`)

### Automations List — Chain Visualization

**File:** `app/src/lib/components/agent/AutomationsSection.svelte`

- Event-triggered automations show "Triggered by" annotation linking to emitter
- Automations with `emit` field show "announces: {event-name}"
- Trigger icon: lightning bolt for event triggers

### Trigger Summary

```typescript
// event: "email.urgent" → "When email.urgent fires"
// event: "email.urgent,email.info" → "When email.urgent, email.info fires"
```

---

## 16. Database Schema

### agent_workflows (event triggers stored here)

```sql
CREATE TABLE agent_workflows (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    binding_name TEXT NOT NULL,
    trigger_type TEXT NOT NULL,       -- "schedule" | "heartbeat" | "event" | "manual"
    trigger_config TEXT NOT NULL,     -- For events: "email.urgent,email.*" (comma-separated)
    description TEXT,
    inputs TEXT,                      -- JSON default inputs
    is_active INTEGER NOT NULL DEFAULT 1,
    emit TEXT,                        -- Event name emitted on completion (short, unnamespaced)
    activities TEXT,                  -- JSON inline activity definitions
    last_fired TEXT,                  -- ISO timestamp of last execution
    UNIQUE(agent_id, binding_name)
);
```

**Event-specific columns:**
- `trigger_config`: Comma-separated source patterns for event type
- `emit`: Short event name (namespaced to `{agent-slug}.{emit}` at runtime)
- `is_active`: Checked during subscription registration (inactive = not subscribed)

**Note:** Event subscriptions are NOT persisted in their own table. They are derived
from `agent_workflows` rows and held in-memory in the EventDispatcher.

### Key queries

```sql
-- Load all active event triggers on startup
SELECT * FROM agent_workflows rw
JOIN agents a ON rw.agent_id = a.id
WHERE rw.trigger_type = 'event' AND rw.is_active = 1 AND a.is_enabled = 1;

-- List emit sources for discovery
SELECT rw.emit, a.name, rw.binding_name, rw.description
FROM agent_workflows rw JOIN agents a ON rw.agent_id = a.id
WHERE rw.emit IS NOT NULL AND rw.emit != '' AND rw.is_active = 1;
```

---

## 17. Boot Sequence

1. **`lib.rs:733`** — `EventBus::new()` creates channel
2. **`lib.rs:734`** — `EventDispatcher::new()` creates empty subscription list
3. **`lib.rs:737`** — `EmitTool::new(event_bus.clone())` registered in tool registry
4. **`lib.rs:806`** — `EventDispatcher::spawn(rx, manager)` starts consumer loop
5. **Agent workers start** — For each active agent:
   - `process_agent_bindings()` upserts `agent_workflows` rows
   - Event-type bindings create `EventSubscription` objects
   - Each source pattern in `trigger_config` becomes a separate subscription
   - Subscriptions registered via `dispatcher.subscribe()`

After boot, the dispatcher is consuming events and matching against all registered
subscriptions.

---

## 18. Code Paths That Register Event Triggers

| Path | When | File:Line |
|------|------|-----------|
| `process_agent_bindings()` | Agent install, config re-process | `handlers/agents.rs:572` |
| `register_binding_triggers()` | Binding CRUD (create, update, toggle) | `handlers/agents.rs:1434` |
| `PersonaTool::register_config_triggers()` | CLI/agent install | `tools/agent_tool.rs:1147` |
| `register_agent_triggers()` | Cron job registration (schedule only) | `workflow/triggers.rs:40` |

**Critical note:** `register_agent_triggers()` in `triggers.rs` only handles **schedule**
triggers (cron jobs). Event triggers are registered separately via the EventDispatcher
in the calling code.

---

## 19. Code Paths That Emit Events

| Emitter | Source Pattern | Origin | File |
|---------|---------------|--------|------|
| EmitTool (workflow activities) | User-defined | `ctx.session_key` | `tools/emit_tool.rs` |
| NeboLoop agent_space messages | `neboloop.agent_space.{slug}` | `"neboloop"` | `server/lib.rs:1172` |
| NeboLoop chat/DM messages | `neboloop.{topic}` | `"neboloop"` | `server/lib.rs:1246` |
| NeboLoop other topics | `neboloop.{topic}` | `"neboloop"` | `server/lib.rs:1263` |
| MCP server emit_event tool | User-defined | `"mcp"` | `handlers/mcp_server.rs:355` |
| Plugin watch NDJSON auto-emit | `{plugin-slug}.{event-name}` (e.g. `gws.email.new`) | `"plugin:{slug}:{binding}"` | `agent/agent_worker.rs` |

**Plugin watch auto-emission:** When an agent's `watch` trigger references a plugin
event (via the `event` field), the AgentWorker's `watch_loop()` parses NDJSON lines
from the plugin's stdout and emits them directly into the EventBus. Supports single-event
and multiplexed modes. See [PLUGIN_SYSTEM.md §5](PLUGIN_SYSTEM.md#5-plugin-events) for
the full NDJSON protocol, `PluginEventDef` schema, and multiplexing details.

---

## 20. Naming Conventions

### Event Source Patterns

**User-defined events (via EmitTool):**
- `{domain}.{action}` — e.g. `"email.urgent"`, `"lead.qualified"`
- `{domain}.{subdomain}.{action}` — e.g. `"api.webhook.received"`

**System events:**
- `neboloop.{topic}` — NeboLoop message events
- `neboloop.agent_space.{slug}` — Agent-to-agent messages

**Plugin events (via watch trigger auto-emit):**
- `{plugin-slug}.{event-name}` — e.g. `"gws.email.new"`, `"gws.calendar.event"`
- Declared in `plugin.json` `events[]` array, prefixed with plugin slug at runtime

**Emit sources (agent-namespaced):**
- `{agent-slug}.{emit-name}` — e.g. `"chief-of-staff.briefing.ready"`
- Built at runtime: `agent_name.to_lowercase().replace(' ', '-')` + `.` + emit field

### Subscription Patterns

- Exact: `"email.urgent"` — matches only that source
- Wildcard: `"email.*"` — matches any source starting with `"email."`
- Multiple: stored as `"email.urgent,email.info"` in DB, split into separate subscriptions

### Reserved Input Keys

Keys prefixed with `_` are injected by the dispatcher:
- `_event_source` — The event's source string
- `_event_payload` — The event's payload object
- `_event_origin` — The event's origin trace

The frontend automation editor strips `_`-prefixed keys from user input rows.

---

## 21. Gotchas & Known Issues

### No Persistence

Events are in-memory only. If the server restarts, any events in the channel are lost.
If the EventDispatcher is slow, events queue in the unbounded channel but are never
written to disk. This is by design — events are ephemeral signals, not durable messages.

### No Backpressure

`EventBus` uses `mpsc::unbounded_channel()`. In a runaway event chain scenario
(workflow A emits → triggers B → B emits → triggers A), events accumulate in memory
without limit. The practical bound is token budgets exhausting, but the channel itself
has no cap.

### No Cycle Detection

Event chains can create cycles. There is no detection or prevention mechanism. A
cycle would continue until workflow token budgets are exhausted or the server runs
out of memory.

### No Delivery Guarantees

- At-most-once delivery (fire-and-forget)
- No acknowledgment, no retry on failure
- If `run_inline()` fails, the error is logged but the event is consumed

### Subscriptions are In-Memory

Event subscriptions are rebuilt from `agent_workflows` on each relevant lifecycle event
(boot, install, toggle). If a subscription is lost from memory (e.g., due to a bug
in cleanup), it won't fire until the next rebuild.

### Dispatcher Is Single-Threaded

The dispatcher loop processes events sequentially. `run_inline()` is async (spawns
a background task), so the dispatcher doesn't block on execution, but matching is
single-threaded through the event loop.

### EventTool Naming Confusion

`EventTool` (`event_tool.rs`) manages cron jobs, NOT events. `EmitTool`
(`emit_tool.rs`) is the actual event emission tool. The naming is a historical
artifact from when the cron system was called "events."

### Cron Expression Normalization Bug (fixed 2026-03-24)

Three code paths stored cron expressions for schedule triggers. Only one
(`role_tool.rs::register_config_triggers`) normalized 5-field cron to the 7-field
format required by the `cron` crate v0.12. The other two paths
(`flatten_trigger_config` and `process_role_bindings`) passed raw 5-field
expressions through, causing "Invalid cron expression" errors at runtime.

**Fix:** Both `flatten_trigger_config` and `process_agent_bindings` now call
`PersonaTool::normalize_cron()`.

---

## 22. Relationship to Other Systems

### Events vs Cron Scheduler

| | Events | Cron |
|---|--------|------|
| **Trigger** | Source pattern match | Time-based (cron expression) |
| **Timing** | Real-time (immediate) | 60s polling interval |
| **Storage** | In-memory subscriptions | `cron_jobs` DB table |
| **Execution** | `manager.run_inline()` | Shell, agent, or `manager.run_inline()` |
| **Interaction** | Independent — no overlap | Independent — no overlap |

They share the same `WorkflowManager.run_inline()` execution path but are otherwise
completely independent systems.

### Events vs Heartbeat

Heartbeats use `tokio::interval` with `run_inline()` or `run_chat()`. They do not
emit events and are not triggered by events. Heartbeat is a separate scheduling
mechanism for proactive agent behavior.

### Events and WebSocket

Workflow run status changes (started, completed, failed, cancelled) are broadcast to
WebSocket clients via `hub.broadcast()`. These are NOT events in the EventBus sense —
they are UI notifications only.

### Events and NeboLoop

NeboLoop is both a consumer and producer:
- **Producer:** Inbound NeboLoop messages emit events into the EventBus
- **Consumer:** Agents can subscribe to `neboloop.*` patterns to react to messages

---

## 23. File Reference

| File | Role | Key Exports |
|------|------|-------------|
| `crates/tools/src/events.rs` | Event struct, EventBus | `Event`, `EventBus` |
| `crates/tools/src/emit_tool.rs` | Workflow emit tool | `EmitTool` |
| `crates/tools/src/event_tool.rs` | Cron job management tool (NOT events) | `EventTool` |
| `crates/workflow/src/events.rs` | Dispatcher, matching, subscriptions | `EventDispatcher`, `EventSubscription` |
| `crates/workflow/src/triggers.rs` | Trigger registration helpers | `register_agent_triggers()` |
| `crates/workflow/src/engine.rs` | Workflow execution, emit injection | `execute_workflow()` |
| `crates/server/src/lib.rs` | Boot wiring, system event emitters | Lines 733-806, 1172-1275 |
| `crates/server/src/handlers/agents.rs` | Binding CRUD, subscription registration | `process_agent_bindings()`, `register_binding_triggers()` |
| `crates/server/src/handlers/mcp_server.rs` | MCP event emission | `emit_event` handler |
| `crates/server/src/workflow_manager.rs` | `run_inline()` execution | `WorkflowManagerImpl` |
| `crates/agent/src/agent_worker.rs` | Agent worker lifecycle | `AgentWorker`, `AgentWorkerRegistry` |
| `crates/napp/src/agent.rs` | Trigger types, WorkflowBinding | `AgentTrigger::Event`, `WorkflowBinding` |
| `crates/db/src/queries/agents.rs` | Event-related DB queries | `list_event_triggers()`, `list_emit_sources()` |

---

*Last updated: 2026-03-24*
