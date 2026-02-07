# ADR-003: Multi-Lane Monitoring & Observability

**Status:** Proposed  
**Date:** 2026-02-07  
**Author:** Alma Tuck  
**Depends on:** ADR-002 (Cancel Active Task — active task tracking in LaneManager)

## Context

Nebo's lane system is the concurrency backbone — every piece of work flows through one of six lanes (main, events, subagent, nested, heartbeat, comm). But today, there is **zero visibility** into lane activity.

### What exists today

- `LaneManager.GetLaneStats()` returns `map[string]LaneStats` with `queued`, `active`, and `max_concurrent` per lane — but nothing consumes it
- The `/agent/status` endpoint returns only `connected: bool`, `agentId`, and `uptime`
- The Settings → Status page shows: MCP server online/offline, database, WebSocket, uptime, connected agents table, and four stat cards (sessions, clients, memory, agents) — all of which are mostly hardcoded placeholders
- No WebSocket events for lane state changes
- No way to see what the agent is currently doing, what's queued, or which lanes are busy
- No historical view of lane throughput or task durations

### Why this matters

1. **Debugging.** When the agent seems stuck, the user has no way to tell if it's processing a long task, waiting in queue, or actually hung.
2. **Trust.** A visible "heartbeat lane: idle, main lane: processing, 2 subagents active" readout makes the agent feel alive and responsive even when it's thinking.
3. **Capacity planning.** Seeing that the events lane is perpetually at capacity tells you to bump its concurrency.
4. **ADR-002 dependency.** The cancel feature (ADR-002) needs the user to see *what* is running before they can decide to cancel it.

## Decision

### 1. Enrich `LaneStats` with task-level detail

Extend the existing `LaneStats` struct and add a new `LaneTaskInfo` for individual task visibility:

```go
type LaneStats struct {
    Lane          string         `json:"lane"`
    Queued        int            `json:"queued"`
    Active        int            `json:"active"`
    MaxConcurrent int            `json:"max_concurrent"`
    ActiveTasks   []LaneTaskInfo `json:"active_tasks,omitempty"`
    QueuedTasks   []LaneTaskInfo `json:"queued_tasks,omitempty"`
}

type LaneTaskInfo struct {
    ID          string `json:"id"`
    Description string `json:"description"`      // Human-readable (e.g., "User chat", "Sub-agent: find Go files")
    EnqueuedAt  string `json:"enqueued_at"`       // ISO 8601
    StartedAt   string `json:"started_at,omitempty"`
    ElapsedMs   int64  `json:"elapsed_ms,omitempty"`
}
```

This requires ADR-002's `active []*laneEntry` tracking to be in place — without it, we can only report queue contents, not running tasks.

Add a `Description` field to `LaneTask` so callers can label their work when enqueuing:

```go
type LaneTask struct {
    ID          string
    Lane        string
    Description string  // NEW — human-readable label
    Task        func(ctx context.Context) error
    // ... existing fields
}
```

And a new enqueue option:

```go
func WithDescription(desc string) EnqueueOption {
    return func(c *enqueueConfig) { c.description = desc }
}
```

### 2. New API endpoint: `GET /agent/lanes`

Returns the full lane dashboard in one call:

```json
{
  "lanes": {
    "main":      { "queued": 0, "active": 1, "max_concurrent": 1, "active_tasks": [...] },
    "events":    { "queued": 0, "active": 0, "max_concurrent": 2 },
    "subagent":  { "queued": 0, "active": 3, "max_concurrent": 0, "active_tasks": [...] },
    "nested":    { "queued": 0, "active": 0, "max_concurrent": 3 },
    "heartbeat": { "queued": 0, "active": 0, "max_concurrent": 1 },
    "comm":      { "queued": 0, "active": 1, "max_concurrent": 5, "active_tasks": [...] }
  },
  "total_active": 5,
  "total_queued": 0
}
```

Implementation: handler calls `hub.GetAnyAgent()` → agent state → `lanes.GetLaneStats()` (enriched version).

### 3. Real-time lane updates via WebSocket

Push `lane_update` events whenever lane state changes (task starts, completes, enqueues, or cancels):

```json
{
  "type": "lane_update",
  "data": {
    "lane": "main",
    "event": "task_started",  // task_enqueued | task_started | task_completed | task_cancelled | task_error
    "task": {
      "id": "main-1738900000000",
      "description": "User chat"
    },
    "stats": {
      "queued": 0,
      "active": 1,
      "max_concurrent": 1
    }
  }
}
```

Emit from four points in `lane.go`:
- `pump()` when moving entry from queue → active → **`task_started`**
- `pump()` goroutine on task completion → **`task_completed`** or **`task_error`**
- `Enqueue()` when adding to queue → **`task_enqueued`**
- `CancelActive()` / `ClearLane()` → **`task_cancelled`**

The emit mechanism: `LaneManager` gets an optional `OnEvent func(event LaneEvent)` callback. The agent command wires this to broadcast via the hub's WebSocket.

```go
type LaneEvent struct {
    Lane        string       `json:"lane"`
    Event       string       `json:"event"`
    Task        LaneTaskInfo `json:"task"`
    Stats       LaneStats    `json:"stats"`
}
```

### 4. Frontend: Lane Monitor panel

**Location:** Replace or augment the existing Settings → Status page. Also add a compact lane indicator to the chat page header.

#### Status page — full dashboard

Replace the placeholder "Quick Stats" section with a real lane monitor:

```
┌─────────────────────────────────────────────────────┐
│  Lane Monitor                          Auto-refresh │
├─────────────────────────────────────────────────────┤
│                                                     │
│  main ━━━━━━━━━━━━━━━━━━━  1/1 active              │
│    ▸ "User chat" — running 3.2s                     │
│                                                     │
│  events ──────────────────  0/2                     │
│                                                     │
│  subagent ━━━━━━━━━━━━━━━  3/∞ active              │
│    ▸ "Find Go files with errors" — 12.1s            │
│    ▸ "Summarize PR #42" — 5.7s                      │
│    ▸ "Check deploy status" — 1.3s                   │
│                                                     │
│  nested ──────────────────  0/3                     │
│                                                     │
│  heartbeat ───────────────  0/1                     │
│                                                     │
│  comm ────────────────────  0/5                     │
│                                                     │
└─────────────────────────────────────────────────────┘
```

Visual encoding:
- **━━ thick line** = has active tasks (colored by lane: main=primary, subagent=secondary, etc.)
- **── thin line** = idle
- **Pulsing dot** on active tasks (CSS animation)
- Task elapsed time updates every second via `$effect` interval
- Queue items shown below active items with a ⏳ icon

#### Chat page — compact indicator

In the chat header, next to the connection badge, show a small lane summary when there's background activity:

```
[🟢 Connected]  [⚡ 3 subagents · 1 event]
```

Clicking it navigates to the full Status page.

### 5. Historical metrics (deferred — Phase 2)

For v1, we track only live state. Future work:
- Store completed task durations in SQLite (`lane_task_history` table)
- Show throughput charts (tasks/minute per lane)
- Show P50/P95 task duration per lane
- Alert when lanes are consistently at capacity

This is explicitly **out of scope** for the initial implementation.

## File Changes

| File | Change |
|------|--------|
| `internal/agenthub/lane.go` | Enrich `LaneStats`, add `LaneTaskInfo`, `LaneEvent`, `OnEvent` callback, `WithDescription()` option, emit events from pump/enqueue/cancel |
| `internal/agenthub/lane.go` | Add `Description` field to `LaneTask` |
| `internal/server/server.go` | Register `GET /agent/lanes` route |
| `internal/handler/agent/getlanestatushandler.go` | New handler — calls `GetLaneStats()` |
| `internal/types/types.go` | Add `LaneStatusResponse`, `LaneStatsDTO`, `LaneTaskInfoDTO` |
| `cmd/nebo/agent.go` | Wire `LaneManager.OnEvent` → hub broadcast as `lane_update` WS message |
| `internal/realtime/client.go` | No change needed — `lane_update` is server→client only (broadcast), not a client message type |
| `app/src/lib/api/nebo.ts` | Add `getLaneStatus()` function |
| `app/src/routes/(app)/settings/status/+page.svelte` | Replace placeholder stats with lane monitor UI |
| `app/src/routes/(app)/agent/+page.svelte` | Add compact lane indicator in header |
| `app/static/app.css` | Styles for lane bars, pulsing dots, task rows |

## Enqueue Site Updates

Every place that calls `Enqueue()` or `EnqueueAsync()` should pass `WithDescription()`:

| Call Site | Description |
|-----------|-------------|
| `agent.go` — main lane chat | `"User chat"` |
| `agent.go` — events lane | `"Scheduled: {job_name}"` |
| `agent.go` — heartbeat lane | `"Heartbeat tick"` |
| `agent.go` — comm lane | `"Comm: {topic}"` |
| `orchestrator.go` — subagent lane | `"Sub-agent: {description}"` |
| Nested tool calls | `"Tool: {tool_name}"` |

## Wire Protocol

### REST: `GET /agent/lanes`

No request params. Returns `LaneStatusResponse`.

### WebSocket: `lane_update` (server → client)

Pushed on every lane state change. Frontend subscribes with:
```typescript
client.on('lane_update', (data) => { ... })
```

No client-initiated lane messages needed — this is purely observational.

## Risks & Mitigations

| Risk | Mitigation |
|------|------------|
| High-frequency lane events flood the WebSocket (especially nested lane with rapid tool calls) | Debounce: batch events, emit at most every 250ms per lane. Collapse consecutive `task_started` + `task_completed` within the debounce window into a single update. |
| `OnEvent` callback blocks the lane pump goroutine | Make the callback non-blocking: send to a buffered channel, drop if full. The broadcast goroutine drains the channel independently. |
| Adding `Description` to all enqueue sites is tedious | Default to `"{lane}-{timestamp}"` if no description provided. Enrich incrementally — even without descriptions, the count/active/queued data is valuable. |
| Elapsed time display drifts on the frontend | Use `enqueued_at` / `started_at` timestamps from the server and compute elapsed client-side. Re-sync on each `lane_update` event. |
| Status page polling + WebSocket events = redundant | Use WebSocket as primary for live updates. REST endpoint only for initial page load and as a fallback if WS is disconnected. |

## Alternatives Considered

1. **Poll-only (no WebSocket events).** Simpler, but introduces up to 10s latency (current poll interval) and misses transient events like a fast sub-agent that starts and finishes between polls.

2. **Expose raw lane data in the existing `/agent/status` endpoint.** Muddies the simple connected/uptime purpose of that endpoint. Better as a separate `/agent/lanes` endpoint.

3. **Full OpenTelemetry tracing.** Overkill for a single-user desktop app. The custom lane events give us exactly the observability we need without the infrastructure.

4. **Separate monitoring page (new route).** Adds nav complexity. Better to enrich the existing Status page which already has the scaffolding.

## Success Criteria

- [ ] `GET /agent/lanes` returns accurate per-lane stats with task-level detail
- [ ] WebSocket `lane_update` events fire within 250ms of state changes
- [ ] Status page shows all 6 lanes with active/queued counts and task descriptions
- [ ] Active task elapsed time updates live (every 1s)
- [ ] Chat page shows compact background activity indicator when subagents/events are running
- [ ] No measurable performance impact from lane event emission (< 1ms overhead per event)
- [ ] Works correctly when lanes are empty, at capacity, or have unlimited concurrency (subagent lane)
