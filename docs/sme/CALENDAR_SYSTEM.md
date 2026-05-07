# Calendar System — SME Reference

Complete Subject Matter Expert document covering the Nebo schedule/calendar UI:
the `/schedule` route, calendar views (day/week/month), event rendering, the schedule
store, detail pane, user-created items, and integration with the backend task/cron
system.

**Status:** Current (Rust + SvelteKit) | **Last updated:** 2026-05-05

**Related docs:**
- [AUTOMATION_SME.md](AUTOMATION_SME.md) — backend automation pipeline (cron, heartbeat, workflows)
- [EVENT_SYSTEM_SME.md](EVENT_SYSTEM_SME.md) — EventBus, emit tool, event-triggered workflows

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [Frontend Components](#2-frontend-components)
3. [Schedule Store](#3-schedule-store)
4. [Data Model](#4-data-model)
5. [Three-Tier Item System](#5-three-tier-item-system)
6. [API Data Loading](#6-api-data-loading)
7. [Day View Rendering](#7-day-view-rendering)
8. [Week View Rendering](#8-week-view-rendering)
9. [Month View Rendering](#9-month-view-rendering)
10. [Detail Pane & Editing](#10-detail-pane--editing)
11. [User Item CRUD](#11-user-item-crud)
12. [Lane Packing Algorithm](#12-lane-packing-algorithm)
13. [Backend: Event Domain Tool](#13-backend-event-domain-tool)
14. [Backend: Scheduler Daemon](#14-backend-scheduler-daemon)
15. [Backend: REST API](#15-backend-rest-api)
16. [Database Schema](#16-database-schema)
17. [Workflow Canvas Integration](#17-workflow-canvas-integration)
18. [Time Representation](#18-time-representation)
19. [Known Issues & TODOs](#19-known-issues--todos)
20. [File Reference](#20-file-reference)

---

## 1. Architecture Overview

```
  ┌──────────────────────────────────────────────────────────────────┐
  │                      /schedule route                              │
  │  ┌─────────────┐  ┌──────────────────────────┐  ┌────────────┐  │
  │  │   Sidebar    │  │   ColorCalendarShell      │  │ DetailPane │  │
  │  │  (agents +   │  │  ┌────────────────────┐   │  │ (create or │  │
  │  │  MiniMonth)  │  │  │ Day/Week/MonthView │   │  │  detail)   │  │
  │  └─────────────┘  │  └────────────────────┘   │  └────────────┘  │
  │                    └──────────────────────────┘                    │
  └───────────────────────────────┬──────────────────────────────────┘
                                  │ onMount → loadScheduleFromAPI()
                                  ▼
  ┌──────────────────────────────────────────────────────────────────┐
  │                     Backend REST APIs                              │
  │  GET /api/v1/agents                                               │
  │  GET /api/v1/agents/:id/runs                                      │
  │  GET /api/v1/agents/:id/workflows                                 │
  │  GET /api/v1/tasks (cron jobs)                                    │
  └───────────────────────────────┬──────────────────────────────────┘
                                  │
                                  ▼
  ┌──────────────────────────────────────────────────────────────────┐
  │                     Scheduler Daemon                               │
  │  60s tick → list_enabled_cron_jobs → execute due jobs             │
  │  Task types: bash | agent | workflow | agent_workflow              │
  └──────────────────────────────────────────────────────────────────┘
```

**Key design:** The calendar UI is a **read-only visualization** of backend workflow
triggers (schedule/heartbeat) plus a **local-only** user item system. User-created
items are client-side state (Svelte writable store) — they are NOT persisted to the
backend.

---

## 2. Frontend Components

### Component Hierarchy

```
+page.svelte (schedule route)
├── Sidebar
│   ├── Agent checkboxes (toggle visibility)
│   ├── MiniMonth (date picker)
│   └── UserMenu
├── ColorCalendarShell (header + view switch)
│   ├── ColorDayView
│   │   └── DayDetailPane
│   ├── ColorWeekView
│   │   └── DayDetailPane
│   └── ColorMonthView
│       └── DayDetailPane
└── WorkflowBuilder (modal overlay)
```

### File Locations

| Component | Path | Lines |
|-----------|------|-------|
| Schedule page | `app/src/routes/schedule/+page.svelte` | 205 |
| Calendar shell | `app/src/lib/components/ColorCalendarShell.svelte` | 87 |
| Day view | `app/src/lib/components/ColorDayView.svelte` | 144 |
| Week view | `app/src/lib/components/ColorWeekView.svelte` | 183 |
| Month view | `app/src/lib/components/ColorMonthView.svelte` | 104 |
| Detail pane | `app/src/lib/components/DayDetailPane.svelte` | 474 |
| Event modal | `app/src/lib/components/ScheduleEventModal.svelte` | 160 |
| Mini month | `app/src/lib/components/MiniMonth.svelte` | 77 |
| Schedule store | `app/src/lib/stores/schedule.ts` | 453 |

---

## 3. Schedule Store

**File:** `app/src/lib/stores/schedule.ts`

The schedule store is the data layer for the calendar. It manages three item sources,
provides query functions for views, and handles API data loading.

### Exports

| Export | Type | Purpose |
|--------|------|---------|
| `userScheduleItems` | `Writable<CalendarItem[]>` | User-created items (local-only) |
| `loadScheduleFromAPI()` | `async () => void` | Fetches agents, runs, workflows from API |
| `getAllItems(userItems)` | `fn` | Merge all three item tiers |
| `itemsForWeekday(wd, enabled, userItems)` | `fn` | Filter by day + enabled agents |
| `flattenForDate(wd, enabled, userItems)` | `fn` | Sort + flatten for rendering |
| `attachRunData(items)` | `fn` | Attach latest run info to items |
| `getRecentRuns(agentFull, wfId)` | `fn` | Get run history for detail pane |
| `runsPerWeek(agentShort, userItems)` | `fn` | Sidebar frequency label |
| `getScheduleAgents(userItems)` | `fn` | List agents with schedule items |
| `addUserItem(item)` | `fn` | Create a user item |
| `updateUserItem(id, changes)` | `fn` | Update a user item |
| `removeUserItem(id)` | `fn` | Delete a user item |
| `snapTo15(h)` | `fn` | Snap fractional hour to 15-min grid |
| `parseScheduleString(s)` | `fn` | Parse "9:00 AM Monday" → `{hour, days}` |

### Internal State

```typescript
let _scheduledItems: CalendarItem[] = [];     // From API workflows (read-only)
let _eventRunItems: CalendarItem[] = [];      // From API run history (read-only)
let _apiRunsCache: Record<string, any[]> = {}; // Keyed by "agentId:workflowId"
```

---

## 4. Data Model

### CalendarItem

```typescript
export interface CalendarItem {
  id: string;              // Prefix: "wf:", "hb:", "ev:", "user:"
  agent: string;           // Short ID ('res', 'cod', etc.)
  agentFull: string;       // Full agent ID ('researcher', 'coder')
  kind: EventKind;         // 'sched' | 'event' | 'user'
  label: string;           // Display name
  days: number[];          // Mon=1..Sun=7
  hour: number;            // Fractional: 9.25 = 9:15 AM
  dur: number;             // Fractional hours (0.5 = 30 min)
  end: number;             // hour + dur (computed)
  workflowId?: string;     // Backend workflow binding name
  triggerType: string;     // 'schedule', 'heartbeat', 'event'
  recurrence?: string;     // Display label ("weekdays", "daily", etc.)
  run?: RunData;           // Most recent execution data
}
```

### RunData

```typescript
export interface RunData {
  id: string;
  status: RunStatus;       // 'success' | 'failed' | 'skipped' | 'running' | 'pending'
  actualDuration: string;  // "2m 14s"
  startedAt: string;
  completedAt: string;
  tokens?: { input: number; output: number };
  activities?: { id: string; status: string; duration: string; output?: string; error?: string }[];
}
```

### ID Prefixes

| Prefix | Source | Example |
|--------|--------|---------|
| `wf:` | Workflow schedule trigger | `wf:researcher:morning-scan` |
| `hb:` | Heartbeat trigger (expanded) | `hb:researcher:email-check:3` |
| `ev:` | Event-triggered run | `ev:researcher:inbox:run-123` |
| `user:` | User-created (local) | `user:1` |

---

## 5. Three-Tier Item System

The calendar displays items from three independent sources:

### Tier 1: Scheduled Items (`_scheduledItems`)

- **Source:** Agent workflow triggers with `type: "schedule"` or `type: "heartbeat"`
- **Populated by:** `loadScheduleFromAPI()` parsing workflow trigger configs
- **Read-only** from the frontend's perspective
- Schedule triggers: single item per binding, with parsed time + days
- Heartbeat triggers: expanded into N items (one per interval occurrence)

### Tier 2: Event Run Items (`_eventRunItems`)

- **Source:** Past runs from webhooks/events that don't have a schedule trigger
- **Populated by:** API run history for non-scheduled workflows
- **Read-only** — shows what already happened
- Positioned on calendar based on actual run start time

### Tier 3: User Items (`userScheduleItems`)

- **Source:** User clicks "Create Event" in the detail pane
- **Stored in:** Svelte writable store (client-side only, lost on refresh)
- **Editable** — can change time, duration, days
- **NOT persisted to backend** (TODO noted in code)

### Merge Order

```typescript
function getAllItems(userItems): CalendarItem[] {
  return [..._scheduledItems, ..._eventRunItems, ...userItems];
}
```

---

## 6. API Data Loading

`loadScheduleFromAPI()` executes on mount and follows this sequence:

```
1. listAgents() → filter enabled → ensureAgent() for display list
2. For each agent: listAgentRuns(id) → cache by "agentId:workflowId"
3. For each agent: listAgentWorkflows(id) → parse triggers:
   - schedule trigger → parseScheduleString() → CalendarItem (kind: 'sched')
   - heartbeat trigger → expandHeartbeat() → N CalendarItems (kind: 'sched')
   - other triggers → skip
4. Build event run items from unclaimed runs in cache (kind: 'event')
```

### Schedule String Parser

Handles trigger.schedule like `"9:00 AM Monday"`:

```typescript
parseScheduleString("9:00 AM Monday")   → { hour: 9, days: [1] }
parseScheduleString("3:00 PM daily")    → { hour: 15, days: [1,2,3,4,5,6,7] }
parseScheduleString("8:30 AM weekdays") → { hour: 8.5, days: [1,2,3,4,5] }
```

### Heartbeat Expander

Converts interval + window to multiple time slots:

```typescript
expandHeartbeat("30m")                    → [0, 0.5, 1, ..., 23.5] (48 items)
expandHeartbeat("1h", {start:"9:00 AM", end:"5:00 PM"}) → [9, 10, ..., 16] (8 items)
```

### Duration Estimation

For each item, duration is estimated from run history (average of past durations).
Falls back to 15 minutes (0.25 hours) if no run data exists.

---

## 7. Day View Rendering

**File:** `app/src/lib/components/ColorDayView.svelte`

### Layout

```
┌─────────┬────────────────────────────────────────────┐
│ Hour    │  Event Well (24 × HOUR_PX tall)            │
│ Ruler   │                                            │
│ (72px   │  ┌────────────────────────────┐            │
│  wide)  │  │ Event block (absolute pos) │            │
│         │  └────────────────────────────┘            │
│         │                                            │
│  7 AM   │  ─── now line (red, if today) ───          │
│  8 AM   │                                            │
│  ...    │  ┌─ preview block (dashed, pulsing) ──┐    │
│         │  └────────────────────────────────────┘    │
└─────────┴────────────────────────────────────────────┘
```

### Key Constants

- `HOUR_PX = 80` — pixels per hour slot
- Total height: `24 × 80 = 1920px` (scrollable)
- Initial scroll: `7 × 80 = 560px` (starts at 7 AM)
- Min event height: `32px`

### Event Block Positioning

```
top = item.hour × HOUR_PX
height = max(32, item.dur × HOUR_PX)
left = (item.lane × indent)% + 4px
width = (widthPct)% - 8px
z-index = 2 + item.lane (selected: 20)
```

### Interactions

- **Double-click** on event well → opens create form in detail pane
- **Click** on event block → opens detail pane with item info
- **Now line** — red horizontal line at current time (updates every 60s)
- **Live preview** — dashed, pulsing block while create form is open

---

## 8. Week View Rendering

**File:** `app/src/lib/components/ColorWeekView.svelte`

### Layout

```
┌─────────┬─────┬─────┬─────┬─────┬─────┬─────┬─────┐
│         │ Mon │ Tue │ Wed │ Thu │ Fri │ Sat │ Sun │ ← Day strip
├─────────┼─────┼─────┼─────┼─────┼─────┼─────┼─────┤
│ Ruler   │     │     │     │     │     │     │     │
│ (72px)  │ Col │ Col │ Col │ Col │ Col │ Col │ Col │ ← 7 columns
│         │     │     │     │     │     │     │     │
│  7 AM   │     │     │     │     │     │     │     │
│  ...    │     │     │     │     │     │     │     │
└─────────┴─────┴─────┴─────┴─────┴─────┴─────┴─────┘
```

### Behavior

- Week starts on **Monday** (ISO week)
- Each column renders independently via `dayPacked(date)` — same lane packing
- Today column has `bg-primary/5` tint
- Day header shows date, with today circled in primary color
- Events render identically to day view but with `2.5px` left border (vs 3px)
- Double-click on column → create event for that specific day

---

## 9. Month View Rendering

**File:** `app/src/lib/components/ColorMonthView.svelte`

### Layout

- 7 columns × 6 rows grid
- Each cell shows: date number + first 3 events + "+N more" overflow
- Weeks start on Sunday (JS `getDay()` convention for month grid)
- Previous/next month days shown at 50% opacity
- Today cell has `bg-primary/5` tint
- Events render as compact pill buttons with left border accent

### Cell Rendering

```typescript
cellItems(cell) → attachRunData(flattenForDate(weekday, enabled, $userScheduleItems))
visible = items.slice(0, 3)    // Show max 3
hidden = items.length - 3      // "+N more" label
```

---

## 10. Detail Pane & Editing

**File:** `app/src/lib/components/DayDetailPane.svelte`

The detail pane is a right sidebar (w-72 for create, w-80 for detail) that appears
when an event is selected or a create action is initiated.

### Two Modes

#### Create Mode (double-click on calendar well)

Form fields:
- Agent selector (dropdown of all schedule agents)
- Label (text input)
- Trigger type toggle (Schedule / Interval)
- Time (hour + minute selects) or Interval (5m, 10m, 15m, 30m, 1h, 2h, 4h)
- Duration presets (15m, 30m, 45m, 1h, 1.5h, 2h)
- Recurrence (day toggles + Daily/Weekdays presets)

On save: calls `addUserItem()` → closes pane

#### Detail Mode (click on existing event)

Shows:
- Header with agent badge, event label, trigger glyph
- When: time range + recurrence label
- Last Run: status icon + duration + token usage
- Trigger: type label
- "Open in Canvas" button (links to WorkflowBuilder)
- Workflow activities timeline (if available)
- Recent runs list (last 5)

### Inline Editing

All items are editable from the detail pane. Edit mode exposes:
- Time (hour + minute selects)
- Duration (preset buttons)
- Days (day-of-week toggle buttons)

Calls `updateUserItem()` on save.

### Live Preview

While the create form is open, a preview block renders in the calendar:
- Dashed left border, pulsing opacity
- Updates in real-time as form values change
- Driven by `preview` bindable prop passed to the view component

---

## 11. User Item CRUD

```typescript
// Create
addUserItem({
  agent: 'res', agentFull: '', label: 'Morning scan',
  days: [1,2,3,4,5], hour: 9, dur: 0.5,
  triggerType: 'schedule', recurrence: 'weekdays'
})

// Update
updateUserItem('user:1', { hour: 10, dur: 1 })

// Delete
removeUserItem('user:1')
```

All operations update the `userScheduleItems` writable store. Calendar views react
via `$userScheduleItems` subscription in `$derived` blocks.

---

## 12. Lane Packing Algorithm

**File:** `app/src/lib/utils.ts` — `packLanes(items)`

Assigns overlapping events to parallel columns (lanes) to avoid visual overlap.

### Algorithm

```
for each item (sorted by start time):
  find first lane where item doesn't overlap any existing item
  assign item to that lane

for each item:
  item.lane = assigned lane index (0-based)
  item.totalLanes = max lanes needed in its time range
```

### Rendering Math

```
indent = totalLanes > 1 ? min(20, 80 / totalLanes) : 0
leftPct = lane × indent
widthPct = totalLanes > 1 ? 100 - leftPct - (gap if not last) : 100
```

---

## 13. Backend: Event Domain Tool

**File:** `crates/tools/src/event_tool.rs` (356 lines)

The `event` tool is the agent-facing interface for scheduling. Despite the name, it
manages **cron jobs** (not the EventBus).

### Tool Schema

```
event(action, name?, cron?, at?, task_type?, command?, prompt?)
```

### Actions

| Action | Required Params | Behavior |
|--------|----------------|----------|
| `create` | name + (cron OR at) + task_type | Insert into cron_jobs table |
| `list` | — | List all jobs (limit 100) |
| `delete` | name | Delete by name |
| `pause` | name | Set enabled=0 |
| `resume` | name | Set enabled=1 |
| `run` | name | Execute immediately + record history |
| `history` | name | Show last 10 executions |

### Relative Time Parser

`parse_relative_time("in 5 minutes")` → one-shot cron expression:

```rust
// "in 5 minutes" with current time 2:30 PM on March 14, 2026
// → "0 35 14 14 3 * 2026"  (second minute hour day month weekday year)
```

### Task Execution (via `run` action)

1. Look up job by name
2. Create history entry
3. Execute based on task_type:
   - `bash` → `tokio::process::Command::new("bash").arg("-c").arg(command)`
   - `agent` → `runner.deliberate(prompt)` via AdvisorDeliberator trait
4. Update history with result
5. Return success/error message

---

## 14. Backend: Scheduler Daemon

**File:** `crates/server/src/scheduler.rs` (289 lines)

### Lifecycle

- Spawned at server startup with **10s initial delay**
- Polls every **60 seconds**
- Also cleans up expired browser snapshots each tick

### Tick Logic

```rust
for each enabled cron job:
  1. Normalize cron expression (5-field → 7-field)
  2. Parse with `cron::Schedule`
  3. Find next occurrence after last_run
  4. If next <= now → execute
  5. Record history, update last_run, send desktop notification
```

### Task Type Execution

| Type | Handler | Details |
|------|---------|---------|
| `bash` / `shell` | `execute_shell()` | `sh -c {command}` |
| `agent` | `execute_agent()` | Full Runner.run() with streaming, registered in RunRegistry |
| `workflow` | `execute_workflow_task()` | `manager.run(workflow_id)` |
| `agent_workflow` | `execute_agent_workflow_task()` | Load agent config, verify active, `manager.run_inline()` |

### Agent Task Execution Details

Agent tasks via the scheduler:
- Create a unique session key: `cron-{job.name}`
- Register in global `RunRegistry` (visible in runs list, cancellable)
- Stream responses via `RunRequest` with `Origin::System`
- Broadcast `chat_stream` events to WebSocket hub
- Use `cancel_token` for graceful cancellation

### Cron Normalization

Legacy 5-field cron (`minute hour day month weekday`) is auto-upgraded to 7-field
(`second minute hour day month weekday year`) via `PersonaTool::normalize_cron()`.
This is required by the `cron` crate v0.12.

---

## 15. Backend: REST API

**File:** `crates/server/src/handlers/tasks.rs` (241 lines)

All endpoints at `/api/v1/tasks/*`:

| Method | Path | Handler | Purpose |
|--------|------|---------|---------|
| GET | `/api/v1/tasks` | `list_tasks` | Paginated list + total count |
| POST | `/api/v1/tasks` | `create_task` | Create new cron job |
| GET | `/api/v1/tasks/:name` | `get_task` | Single job by name |
| PUT | `/api/v1/tasks/:name` | `update_task` | Upsert (update or create) |
| DELETE | `/api/v1/tasks/:name` | `delete_task` | Delete by name |
| POST | `/api/v1/tasks/:name/toggle` | `toggle_task` | Flip enabled state |
| POST | `/api/v1/tasks/:name/run` | `run_task` | Immediate execution (background) |
| GET | `/api/v1/tasks/:name/history` | `list_task_history` | Execution audit log |

### Create/Update Body

```json
{
  "name": "morning-brief",
  "schedule": "0 0 8 * * 1-5",
  "command": "",
  "taskType": "agent",
  "message": "Check today's calendar and send a summary",
  "instructions": "",
  "enabled": true
}
```

### Run Task Response

```json
{
  "success": true,
  "historyId": 42,
  "message": "Task execution started"
}
```

Execution happens in a background `tokio::spawn`. On completion, broadcasts
`task_complete` WebSocket event:

```json
{
  "task": "morning-brief",
  "success": true,
  "output": "..." // truncated to 500 chars
}
```

---

## 16. Database Schema

### cron_jobs (migration 0015)

```sql
CREATE TABLE cron_jobs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT UNIQUE NOT NULL,
    schedule TEXT NOT NULL,           -- cron expression (7-field preferred)
    command TEXT DEFAULT '',          -- bash command
    task_type TEXT DEFAULT 'bash',    -- 'bash' | 'agent' | 'workflow' | 'agent_workflow'
    message TEXT DEFAULT '',          -- agent prompt (for agent tasks)
    deliver TEXT DEFAULT '',          -- (unused legacy field)
    instructions TEXT,               -- optional system prompt override
    enabled INTEGER DEFAULT 1,
    last_run DATETIME,
    run_count INTEGER DEFAULT 0,
    last_error TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
```

### cron_history (execution audit)

```sql
CREATE TABLE cron_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id INTEGER NOT NULL,
    started_at DATETIME NOT NULL,
    finished_at DATETIME,
    success INTEGER DEFAULT 0,
    output TEXT,
    error TEXT,
    FOREIGN KEY (job_id) REFERENCES cron_jobs(id) ON DELETE CASCADE
);
```

### Key Queries

| Method | Purpose |
|--------|---------|
| `list_cron_jobs(limit, offset)` | Paginated list (ordered by created_at DESC) |
| `list_enabled_cron_jobs()` | Only enabled (for scheduler tick) |
| `get_cron_job_by_name(name)` | Single lookup |
| `create_cron_job(...)` | Insert with RETURNING |
| `upsert_cron_job(...)` | ON CONFLICT(name) DO UPDATE |
| `delete_cron_job_by_name(name)` | Delete by name, returns affected count |
| `toggle_cron_job(id)` | `SET enabled = NOT enabled` |
| `enable_cron_job_by_name(name)` | Set enabled=1 |
| `disable_cron_job_by_name(name)` | Set enabled=0 |
| `update_cron_job_last_run(id, error)` | Update timestamp + increment run_count |
| `create_cron_history(job_id)` | Start audit entry |
| `update_cron_history(id, success, output, error)` | Complete audit entry |
| `list_cron_history(job_id, limit, offset)` | Paginated history |
| `get_recent_cron_history(job_id)` | Last 10 executions |
| `count_cron_jobs()` | Total count (for pagination) |
| `count_cron_history(job_id)` | History count |

---

## 17. Workflow Canvas Integration

The schedule page includes a **WorkflowBuilder** modal overlay accessible from the
detail pane's "Open in Canvas" button.

### Flow

1. User clicks "Open in Canvas" on a scheduled event that has `agentFull` + `workflowId`
2. `handleOpenCanvas(agentFull)` called → fetches agent's workflows via API
3. Parses workflow definitions (trigger, activities, connections, emit)
4. Opens full-screen WorkflowBuilder modal with the agent's workflow data
5. Builder shows visual node-graph editor for workflow activities
6. On save: `handleCanvasSave()` — currently a TODO (not persisted)

---

## 18. Time Representation

The calendar system uses **fractional hours** throughout for simplicity:

| Time | Fractional | Usage |
|------|-----------|-------|
| 9:00 AM | `9.0` | `item.hour` |
| 9:15 AM | `9.25` | After `snapTo15()` |
| 9:30 AM | `9.5` | Heartbeat expansion |
| 2:45 PM | `14.75` | AM/PM → 24h conversion |

### Snap-to-Grid

```typescript
snapTo15(h: number): number → Math.round(h * 4) / 4
```

All user interactions (double-click to create) snap to the nearest 15-minute boundary.

### Duration as Fractional Hours

```
15 min = 0.25
30 min = 0.5
1 hour = 1.0
90 min = 1.5
```

### Day-of-Week Convention

- Monday = 1, Tuesday = 2, ..., Sunday = 7 (ISO weekday)
- JS `getDay()` returns 0=Sunday → converted: `d === 0 ? 7 : d`

---

## 19. Known Issues & TODOs

### User Items Not Persisted

User-created schedule items exist only in the Svelte writable store. They are lost
on page refresh. The backend `/api/v1/tasks` endpoints exist but the frontend
create form does NOT call them.

### Workflow Definition Not Loaded in Detail Pane

```typescript
const workflowDef = $derived.by(() => {
  return null; // TODO: load from listAgentWorkflows() API when item selected
});
```

The workflow activities section in the detail pane is always empty because it requires
a second API call when an item is selected.

### Workflow Canvas Save Not Wired

`handleCanvasSave()` stores data locally but does not persist to the backend:
```typescript
function handleCanvasSave(workflows) {
  // TODO: persist to backend via API
  canvasWorkflowsData = workflows;
  canvasAgentFull = null;
}
```

### Event View Background Pattern

The day/week views use inline `style` attributes for the time grid background
(repeating-linear-gradient). This is acceptable because it depends on computed pixel
values (`HOUR_PX`) that can't be expressed in pure Tailwind utilities.

### No Real-Time Updates

The schedule view loads data once on mount (`onMount → loadScheduleFromAPI()`). It does
NOT subscribe to WebSocket events for real-time updates when workflows execute or
new runs complete. A full page reload is required to see new data.

---

## 20. File Reference

### Frontend

| File | Role |
|------|------|
| `app/src/routes/schedule/+page.svelte` | Route page, sidebar, canvas modal |
| `app/src/lib/components/ColorCalendarShell.svelte` | Header nav, view toggle, date controls |
| `app/src/lib/components/ColorDayView.svelte` | Single-day 24h grid with event blocks |
| `app/src/lib/components/ColorWeekView.svelte` | 7-column week grid |
| `app/src/lib/components/ColorMonthView.svelte` | 6×7 month grid |
| `app/src/lib/components/DayDetailPane.svelte` | Right sidebar: create form + detail view |
| `app/src/lib/components/ScheduleEventModal.svelte` | Quick-create modal (alternative to pane) |
| `app/src/lib/components/MiniMonth.svelte` | Small month calendar for sidebar nav |
| `app/src/lib/stores/schedule.ts` | Store: types, parsing, loading, queries |
| `app/src/lib/utils.ts` | `packLanes()`, `triggerGlyph()`, `fmtTime()` |
| `app/src/lib/data.js` | `AGENTS`, `AGENT_ID_MAP`, `CAL_DAYS` |
| `app/src/lib/tokens.js` | `AGENT_COLORS`, `ensureAgentColor()` |

### Backend

| File | Role |
|------|------|
| `crates/tools/src/event_tool.rs` | `event` domain tool (cron CRUD for agents) |
| `crates/server/src/scheduler.rs` | Background scheduler daemon (60s poll) |
| `crates/server/src/handlers/tasks.rs` | REST API handlers for `/api/v1/tasks/*` |
| `crates/server/src/routes/tasks.rs` | Axum route definitions |
| `crates/db/src/queries/cron_jobs.rs` | SQLite query implementations |
| `crates/db/src/models.rs` | `CronJob`, `CronHistory` structs |
| `crates/db/migrations/0015_cron_jobs.sql` | Table creation |

---

*Last updated: 2026-05-05*
