# Lane System — SME Deep-Dive

> **Audience:** Anyone who needs complete understanding of Nebo's lane-based concurrency system.
> **Last updated:** 2026-02-28

---

## 1. What Lanes Are

Lanes are **named work queues with configurable concurrency limits**. They are the central concurrency control mechanism for the entire Nebo agent — a supervisor pattern that serializes, parallelizes, or rate-limits every category of work the agent performs.

Every piece of work the agent does (user chat, heartbeat tick, scheduled reminder, comm message, sub-agent recovery, developer assistant) is enqueued to exactly one lane. The lane decides when it runs, how many can run concurrently, and provides cancellation, monitoring, and crash recovery.

---

## 2. The Lanes

| Lane | Constant | Default Concurrency | Hard Cap | Purpose |
|------|----------|-------------------|----------|---------|
| **main** | `LaneMain` | 2 (concurrent) | — | User conversations: web UI, CLI, phone, voice — all Main Chat inputs |
| **desktop** | `LaneDesktop` | 1 (serialized) | — | Desktop tool execution (one screen, one mouse, one keyboard) |
| **events** | `LaneEvents` | 0 (unlimited) | — | Reminders, routines, scheduled tasks (each gets own session) |
| **subagent** | `LaneSubagent` | 5 | 10 | Sub-agent goroutine recovery (orchestrator has separate limit) |
| **nested** | `LaneNested` | 3 | 3 | Reserved for nested tool call chains |
| **heartbeat** | `LaneHeartbeat` | 1 (serialized) | — | Proactive heartbeat ticks (2-minute watchdog) |
| **comm** | `LaneComm` | 5 | — | Bot-to-bot DMs (external non-owner messages) |
| **loop-{channelID}** | (dynamic) | 1 (serialized) | — | Per-channel lanes for loop channels (auto-created) |
| **dev** | `LaneDev` | 1 (serialized) | — | Developer assistant (independent of main lane) |

**Concurrency semantics:**
- `1` = strictly serialized — one task at a time, others queue
- `0` = unlimited — all tasks start immediately
- `N` = at most N tasks concurrently, excess tasks queue
- Hard caps are enforced by `SetConcurrency()` — you cannot exceed them even via config

### Lane Routing

Lane selection is determined by a combination of session key prefixes and explicit lane names:

**Session key prefix routing** (in `cmd/nebo/agent.go` WebSocket handler):
```
"heartbeat-*"  → LaneHeartbeat
"reminder-*"   → LaneEvents
"routine-*"    → LaneEvents
"comm-*"       → LaneComm
"dev-*"        → LaneDev
everything else → LaneMain
```

**Explicit lane routing** (in specific handlers):
```
Loop channel messages  → "loop-{channelID}" (dynamic, auto-created with concurrency 1)
Voice input            → LaneMain (routed through lanes, not direct r.Run())
Owner DMs              → LaneMain (same session as web UI — they ARE Main Chat)
External DMs           → LaneComm
Desktop tool execution → LaneDesktop (intercepted at tool execution layer)
```

### Origin Mapping

Each lane implies an **origin** that controls tool access restrictions:

| Lane | Origin | Restrictions |
|------|--------|-------------|
| main, dev | `OriginUser` | None |
| heartbeat, events | `OriginSystem` | None |
| comm, loop-* | `OriginComm` | Denies shell access |
| desktop | (caller's origin) | Inherits from the calling lane |

**Origin-based policy** is active — `OriginComm`, `OriginApp`, and `OriginSkill` all deny `shell` access by default (configured in `defaultOriginDenyList()` in `internal/agent/tools/policy.go`). Loop channels use tools freely (skills, apps, desktop) — they just can't run shell commands.

---

## 3. Core Implementation

**File:** `internal/agenthub/lane.go` (~490 lines)

### 3.1 Data Structures

```go
// Top-level manager — owns all lanes
type LaneManager struct {
    mu      sync.RWMutex
    lanes   map[string]*LaneState
    onEvent func(LaneEvent)
}

// Per-lane state — queue + active tracking
type LaneState struct {
    Lane          string
    Queue         []*laneEntry     // Waiting tasks (FIFO)
    active        []*laneEntry     // Currently executing
    MaxConcurrent int
    notify        chan struct{}    // Buffered(1) — wakeup signal for pump goroutine
    stopCh        chan struct{}    // Close to stop pump goroutine
    mu            sync.Mutex
}

// Internal wrapper around a task
type laneEntry struct {
    task    *LaneTask
    resolve chan error          // Buffered(1) — signals completion
    ctx     context.Context     // Per-task context with lane injected
    cancel  context.CancelFunc  // For cancellation
}

// The actual unit of work
type LaneTask struct {
    ID          string                              // "{lane}-{unix_nano}"
    Lane        string
    Description string                              // Human-readable, shown in UI
    Task        func(ctx context.Context) error     // The work function
    EnqueuedAt  time.Time
    StartedAt   time.Time
    CompletedAt time.Time
    Error       error
    OnWait      func(waitMs int64, queuedAhead int) // Called when wait exceeds threshold
    WarnAfterMs int64                               // Default: 2000ms
}
```

### 3.2 Context Propagation

The lane name is injected into Go's `context.Context` via an unexported key type:

```go
func WithLane(ctx context.Context, lane string) context.Context
func GetLane(ctx context.Context) string
```

**Downstream consumer:** The Janus (OpenAI-compatible) provider reads the lane from context and sends it as `X-Lane` HTTP header for per-lane analytics and billing:

```go
// internal/agent/ai/api_openai.go:196-199
if p.providerID == "janus" {
    if lane := agenthub.GetLane(ctx); lane != "" {
        reqOpts = append(reqOpts, option.WithHeader("X-Lane", lane))
    }
}
```

---

## 4. Enqueue Mechanics

### 4.1 Synchronous: `Enqueue()`

Blocks until the task completes or the parent context is cancelled.

```go
func (m *LaneManager) Enqueue(ctx, lane, taskFn, opts...) error
```

Flow:
1. Default to `LaneMain` if lane is empty
2. Create per-task context: `context.WithCancel(WithLane(ctx, lane))`
3. Wrap in `laneEntry` with buffered resolve channel (size 1)
4. Append to lane queue, snapshot `entryToInfo` under lock, emit `task_enqueued` event
5. Send non-blocking signal to `notify` channel to wake pump goroutine
6. **Block** on `select { <-entry.resolve, <-ctx.Done() }`

If parent context cancels while waiting, the task's context is also cancelled.

### 4.2 Asynchronous: `EnqueueAsync()`

Fire-and-forget — directly creates the entry, appends to queue, and signals the pump. No goroutine wrapper needed since we don't wait for the resolve channel.

```go
func (m *LaneManager) EnqueueAsync(ctx, lane, taskFn, opts...) {
    // Creates entry, appends to Queue under lock, signals notify channel
}
```

**This is the primary method used throughout the codebase.** Almost all lane work is enqueued async because callers (WebSocket handlers, comm callbacks, channel callbacks) must not block.

### 4.3 Enqueue Options

```go
WithWarnAfter(ms int64)       // Threshold for logging wait warnings (default: 2000ms)
WithOnWait(fn)                // Callback when wait exceeds threshold
WithDescription(desc string)  // Human-readable label for monitoring UI
```

---

## 5. The Pump: Task Execution Engine

### 5.1 Channel-Based Wakeup

Each lane has a dedicated **pump goroutine** that loops on a `select` waiting for wakeup signals:

```go
func (s *LaneState) run(mgr *LaneManager) {
    for {
        select {
        case <-s.notify:          // Wakeup: new task enqueued or capacity freed
            s.processAvailable(mgr)
        case <-s.stopCh:          // Shutdown signal
            return
        }
    }
}
```

`processAvailable()` is a `for` loop that:
1. Locks lane state
2. Checks capacity: `MaxConcurrent > 0 && len(active) >= MaxConcurrent`
3. If at capacity or queue empty → unlock, return
4. Pop first entry from queue (FIFO)
5. Check wait time, fire `OnWait` callback if exceeded
6. Set `StartedAt`, snapshot `entryToInfo` under lock, add to active list, unlock
7. Spawn goroutine for the task:

```
goroutine(entry, startInfo):
    emit task_started (using pre-snapshotted info)

    start watchdog timer:
        15 minutes (most lanes)
        2 minutes (heartbeat lane)

    execute task with panic recovery (crashlog.LogPanic)
    stop watchdog

    lock → set CompletedAt, remove from active, snapshot completedInfo → unlock
    log completion/error
    emit task_completed

    send result to resolve channel

    signal notify channel (non-blocking)  ← wakes pump for next task
```

### 5.2 Key Design Decisions

**Dedicated pump goroutine:** Each lane gets its own `run()` goroutine started when the lane is first created in `getLaneState()`. Stopped cleanly via `Shutdown()` which closes all `stopCh` channels.

**Buffered(1) notify channel:** The `notify` channel has capacity 1. Non-blocking sends (`select { case notify <- struct{}{}: default: }`) ensure producers never block. The pump always drains the full queue when woken, so one pending signal is sufficient — no signals are lost even with multiple concurrent producers.

**No draining flag:** The old `drain()`/`pump()`/`draining` pattern used a boolean flag that could lose wakeup signals when the flag was set to false between producers checking it. The channel-based design eliminates this class of bug entirely.

**Race-safe snapshots:** `entryToInfo()` is called under the mutex to snapshot `StartedAt`/`CompletedAt` before emitting events. This prevents data races between task goroutines writing these fields and `GetLaneStats()` reading them.

**Watchdog timer:** Safety net that force-cancels stuck tasks. If a task hangs past the watchdog duration, its context is cancelled and the lane slot is freed. Prevents a single hung task from permanently blocking a lane.

**Panic recovery:** Uses `crashlog.LogPanic` to log panics with lane metadata. The panic is converted to an error and returned via the resolve channel — the lane keeps running.

---

## 6. Lane Operations

### Cancel Active Tasks

```go
func (m *LaneManager) CancelActive(lane string) int
```

Calls `cancel()` on all active tasks in a lane, emits `task_cancelled` events. Returns count. **Used for barge-in** — when a user sends a new message while the previous is still processing.

**Important:** The `cancel` WebSocket frame (agent.go:2906-2914) intentionally does NOT call `ClearLane()`. Only `CancelActive(LaneMain)` is called. The comment explains: the frontend manages its own message queue, and clearing the lane queue would race with new `run` frames arriving.

### Clear Lane (queued only)

```go
func (m *LaneManager) ClearLane(lane string) int
```

Removes all **queued** (not active) tasks. Each removed task has its context cancelled and `context.Canceled` sent to its resolve channel.

### Queue Size

```go
func (m *LaneManager) GetQueueSize(lane string) int     // queued + active for one lane
func (m *LaneManager) GetTotalQueueSize() int            // total across all lanes
```

### Set Concurrency

```go
func (m *LaneManager) SetConcurrency(lane string, maxConcurrent int)
```

Enforces hard caps from `MaxLaneConcurrency`. Negative values treated as unlimited (0). Signals `notify` channel after update to potentially start queued tasks.

### Lane Statistics

```go
func (m *LaneManager) GetLaneStats() map[string]LaneStats
```

Returns per-lane stats including active/queued counts, max concurrency, and task details. Powers the Lane Monitor UI.

```go
type LaneStats struct {
    Lane          string         `json:"lane"`
    Queued        int            `json:"queued"`
    Active        int            `json:"active"`
    MaxConcurrent int            `json:"max_concurrent"`
    ActiveTasks   []LaneTaskInfo `json:"active_tasks,omitempty"`
    QueuedTasks   []LaneTaskInfo `json:"queued_tasks,omitempty"`
}
```

---

## 7. Event System

### Lifecycle Events

```go
type LaneEvent struct {
    Type string       `json:"type"` // task_enqueued | task_started | task_completed | task_cancelled
    Lane string       `json:"lane"`
    Task LaneTaskInfo `json:"task"`
}
```

Events are emitted asynchronously (`go fn(event)`) to prevent blocking the pump loop. This means events are non-blocking but not guaranteed to arrive in order relative to lane work.

### Event Flow: Lane → Web UI

```
LaneManager.emit()
  → OnEvent callback (agent.go:956)
    → sendFrame(type="event", method="lane_update")
      → Agent Hub readPump
        → eventHandler
          → ChatContext.handleAgentEvent (realtime/chat.go)
            → clientHub.Broadcast
              → Browser WebSocket → Lane Monitor UI
```

---

## 8. How Each Lane Is Used

### LaneMain — User Conversations (Concurrent)

**Concurrency:** 2. Allows two inputs to run simultaneously (e.g., phone + desktop, voice + text). Safe because the runner uses per-run local state (`runState` struct) and per-session compaction mutexes.

**Enqueue points:**
- User chat from WebSocket `run` frame
- Introduction requests
- Owner DMs from NeboLoop (same companion chat session as web UI)
- Voice input (routed through LaneMain via `state.lanes.Enqueue()`)
- Local channel app messages

**Cancel:** `cancel` WebSocket frame → `CancelActive(LaneMain)` — barge-in. Queue is NOT cleared.

### LaneDesktop — Desktop Tool Serialization

**Concurrency:** 1 (serialized). One screen, one mouse, one keyboard.

Desktop-category tools (`desktop`, `accessibility`, `screenshot`, `app`, `browser`, `window`, `menubar`, `dialog`, `shortcuts`) are intercepted at the `Registry.Execute()` layer. When `IsDesktopTool(name)` returns true and a `DesktopQueueFunc` is set, the execution is wrapped through the queue function which enqueues to LaneDesktop.

**Behavior:**
- Main Chat using desktop tools: enqueues to LaneDesktop, blocks until result, feels transparent
- Loop channel workflow using desktop tools: queues behind any other desktop work
- Multiple workflows hitting desktop: serialized, first come first served

### LaneEvents — Scheduled Tasks

**Concurrency:** 0 (unlimited). Each task gets its own session, so they can't conflict.

**Enqueue points:**
- Cron tool's `SetAgentCallback` fires reminders here (agent.go:1345)
- Session keys with `"reminder-"` or `"routine-"` prefix (agent.go:2964-2965)
- Recovery re-enqueue for `TaskTypeEventAgent` (agent.go:3259-3260)

### LaneSubagent — Sub-Agent Recovery

**Concurrency:** 5, hard cap 10.

**Important nuance:** The orchestrator (`internal/agent/orchestrator/orchestrator.go`) has its own `maxConcurrent` limit (default 5) and spawns sub-agents as raw goroutines — it does NOT enqueue to the LaneManager. The lane is only used for **recovery re-enqueue** of `TaskTypeSubagent` tasks after a restart (agent.go:3261-3262). So at runtime, the orchestrator's own limit is the effective constraint; the lane limit applies only to recovered tasks.

The orchestrator defines its own copy of lane constants (lines 59-66) — identical string values but a separate declaration.

### LaneNested — Tool Call Chains

**Concurrency:** 3, hard cap 3.

Defined and configurable but **not visibly enqueued to** in the current codebase. Reserved for recursive tool call chains. The hard cap of 3 prevents runaway nesting.

### LaneHeartbeat — Proactive Ticks

**Concurrency:** 1 (serialized).

The heartbeat daemon (`internal/daemon/heartbeat.go`) sends a `"req"` WebSocket frame with `method: "run"` and `session_key: "heartbeat-{timestamp}"`. The agent's WebSocket handler detects the `"heartbeat-"` prefix and routes to `LaneHeartbeat`.

**Special treatment:** Gets a shorter watchdog timeout of **2 minutes** (vs 15 minutes for other lanes).

### LaneComm — Bot-to-Bot Communication

**Concurrency:** 5.

**Enqueue points:**
- `CommHandler.Handle()` in `internal/agent/comm/handler.go` — all incoming comm messages (tasks, task results, general messages)
- External (non-owner) DMs (when `!msg.IsOwner`)

**Privacy boundary:** The comm lane gets NO raw chat history — hard-coded privacy protection.

**Origin restriction:** Runs with `OriginComm`, which denies shell access.

**A2A task lifecycle:** The CommHandler tracks active tasks with cancel functions, supports `working → completed/failed` status flow, and sends failure status on graceful shutdown.

**Note:** Loop channel messages no longer use LaneComm — they get their own dynamic lanes (see below).

### Dynamic `loop-{channelID}` Lanes — Loop Channels

**Concurrency:** 1 (serialized, auto-created).

Each loop channel gets its own lane: `fmt.Sprintf("loop-%s", msg.ChannelID)`. The `getLaneState()` auto-creates lanes with default concurrency 1. This isolates channels from each other — a noisy channel can't starve other channels or DMs.

**Enqueue points:**
- `neboloopPlugin.OnLoopChannelMessage()` callback

**Channel skill bindings:** Before building the RunRequest, the handler checks `channel_skills` table for bindings. If a binding exists, the skill is loaded via `ForceSkill` on the RunRequest.

**Origin:** `OriginComm` — denies shell but allows tools, skills, and apps.

### LaneDev — Developer Assistant

**Concurrency:** 1 (serialized).

Routed when session key starts with `"dev-"` (agent.go:2968-2969). Runs independently from main lane so developer assistant work doesn't block user conversations.

**Not in frontend Lane Monitor** — the status page displays lanes in fixed order: `[main, events, subagent, heartbeat, comm, nested]`. The `dev` lane only appears if data exists and uses its raw name.

---

## 9. Lane-Based Model Routing

Each lane can use a different AI model, configured via `models.yaml`:

```yaml
lane_routing:
  heartbeat: "claude-haiku"     # Cheaper model for background work
  events: "claude-sonnet"       # Mid-tier for reminders
  comm: "claude-haiku"          # Cheap for bot-to-bot
  subagent: "claude-sonnet"     # Mid-tier for sub-agents
```

**Implementation:** `internal/provider/models.go`:

```go
type LaneRouting struct {
    Heartbeat string `yaml:"heartbeat,omitempty"`
    Events    string `yaml:"events,omitempty"`
    Comm      string `yaml:"comm,omitempty"`
    Subagent  string `yaml:"subagent,omitempty"`
}
```

Resolved at enqueue time in agent.go:2989-3001, passed to `runner.Run()` as `ModelOverride`. Also resolved independently for:
- Loop channel messages (agent.go:1705) → `LaneRouting.Comm`
- External DMs (agent.go:1790) → `LaneRouting.Comm`
- Sub-agent spawning in `agent_tool.go:663` and `task.go:191` → `LaneRouting.Subagent`

**Frontend:** Settings → Routing page exposes lane routing configuration.

---

## 10. Configuration

### config.yaml

```yaml
lanes:
  main: 1       # User conversations (serialized)
  events: 2     # Scheduled/triggered tasks
  subagent: 0   # Sub-agent operations (0 = unlimited, clamped to hard cap of 10)
  nested: 3     # Nested tool calls (hard cap: 3)
  heartbeat: 1  # Sequential heartbeat ticks
  comm: 5       # Inter-agent communication
```

Parsed into `LaneConfig` struct in `internal/agent/config/config.go`:

```go
type LaneConfig struct {
    Main      int `yaml:"main"`
    Events    int `yaml:"events"`
    Subagent  int `yaml:"subagent"`
    Nested    int `yaml:"nested"`
    Heartbeat int `yaml:"heartbeat"`
    Comm      int `yaml:"comm"`
}
```

Applied at agent startup (agent.go:935-953): each non-zero value overrides the default via `SetConcurrency()`. Zero values in config are intentionally skipped (they mean "use default"), which is distinct from runtime zero (unlimited).

---

## 11. Recovery and Persistence

Tasks are persisted to the `pending_tasks` SQLite table, which includes a `lane` column.

**File:** `internal/agent/recovery/recovery.go`

```go
type PendingTask struct {
    ID, TaskType, Status, SessionKey, UserID, Prompt string
    Lane        string    // Persisted lane name
    Priority    int
    Attempts    int
    MaxAttempts int       // Default: 3
    // ...
}
```

**Recovery flow on restart** (`agent.go:3248-3273`):
1. `recovery.RecoverTasks()` reads pending/running tasks from DB
2. `CheckTaskCompletion()` examines session messages to avoid re-running completed work
3. Tasks are re-enqueued by type:
   - `TaskTypeEventAgent` → `LaneEvents`
   - `TaskTypeSubagent` → `LaneSubagent`
   - Default → `LaneMain`
4. Max 3 attempts; exceeded → `MarkFailed`
5. Old completed/failed tasks cleaned up after 7 days

**Note:** Recovery re-enqueue uses `TaskType` to determine the lane, not the persisted `lane` field.

---

## 12. HTTP API

**`GET /api/v1/agent/lanes`** — returns lane statistics (active/queued counts, task details). Handler sends `get_lanes` request to agent via Agent Hub.

**`GET /api/v1/agent/loops`** — returns loop/channel hierarchy from NeboLoop. Handler sends `get_loops` request to agent; agent queries `LoopChannelLister` on the active comm plugin. Returns `{ loops: [{ id, name, channels: [{ channel_id, channel_name }] }] }`.

---

## 13. Frontend Lane Monitor

**File:** `app/src/routes/(app)/settings/status/+page.svelte`

Displays a "Lane Monitor" section:
- Calls `api.getLanes()` for initial data
- Listens for `lane_update` WebSocket events → triggers `loadLanes()` refresh
- Fixed display order: `[main, events, subagent, heartbeat, comm, nested]`
- Shows per lane: active/queued counts, max concurrency (infinity symbol for 0), capacity bar, active task descriptions with elapsed time, queued task descriptions

Labels:
```typescript
const laneLabels = {
    main: 'Main',
    events: 'Events',
    subagent: 'Sub-agents',
    heartbeat: 'Heartbeat',
    comm: 'Communication',
    nested: 'Nested'
};
```

`dev` is NOT in `laneOrder` and won't display unless explicitly added.

---

## 14. Key Files Reference

| File | Role |
|------|------|
| `internal/agenthub/lane.go` | Core: LaneManager, LaneState, pump, Enqueue, EnqueueAsync, CancelActive, ClearLane, GetLaneStats, Shutdown. Defines LaneDesktop constant |
| `internal/agenthub/lane_test.go` | 14 tests with race detector coverage |
| `cmd/nebo/agent.go` | Wiring: creates LaneManager, applies config, registers OnEvent, routes work by session prefix, dynamic loop-{channelID} lanes, handles cancel/get_lanes/get_loops, channel skill bindings |
| `internal/agent/runner/runner.go` | Per-run `runState` struct (cachedThresholds, promptOverhead, lastInputTokens), per-session compaction mutex via `sessionLocks sync.Map` |
| `internal/agent/config/config.go` | LaneConfig struct and defaults |
| `internal/agent/comm/handler.go` | CommHandler enqueues all comm work to LaneComm |
| `internal/agent/orchestrator/orchestrator.go` | Duplicate lane constants; sub-agent Lane field for metadata |
| `internal/agent/recovery/recovery.go` | PendingTask persistence with lane column |
| `internal/agent/ai/api_openai.go` | Reads lane from context → X-Lane header to Janus |
| `internal/agent/tools/desktop_queue.go` | DesktopQueueFunc, IsDesktopTool(), executeWithDesktopQueue() — intercepts desktop tools at Registry.Execute() |
| `internal/agent/tools/notify_owner.go` | notify_owner tool — cross-lane owner notifications |
| `internal/agent/tools/query_sessions.go` | query_sessions tool — cross-session awareness for Main Chat |
| `internal/agent/tools/channel_send.go` | message_send tool — extended for loop:{channelID} via LoopSender interface |
| `internal/agent/tools/policy.go` | Origin-based deny list — OriginComm/App/Skill deny shell |
| `internal/provider/models.go` | LaneRouting struct for per-lane model overrides |
| `internal/handler/agent/laneshandler.go` | HTTP handler for GET /api/v1/agent/lanes |
| `internal/handler/agent/loopshandler.go` | HTTP handler for GET /api/v1/agent/loops |
| `internal/db/migrations/0044_channel_skills.sql` | channel_skills table for per-channel skill bindings |
| `internal/voice/duplex.go` | Voice RunnerFunc routes through LaneMain |
| `internal/realtime/chat.go` | Forwards lane_update events to browser clients |
| `internal/daemon/heartbeat.go` | Sends heartbeat frames → routed to LaneHeartbeat |
| `app/src/lib/components/sidebar/Sidebar.svelte` | Sidebar UI: My Chat, loops/channels hierarchy, desktop activity indicator |
| `app/src/routes/(app)/agent/+layout.svelte` | Agent layout with sidebar integration |
| `app/src/routes/(app)/settings/status/+page.svelte` | Frontend Lane Monitor UI |
| `app/src/routes/(app)/settings/routing/+page.svelte` | Frontend lane routing config UI |
| `app/src/lib/api/nebo.ts` | `getLanes()`, `getLoops()` API client |

---

## 15. Architectural Notes and Gotchas

1. **Tests exist** in `internal/agenthub/lane_test.go` — 14 tests covering sync/async enqueue, concurrency limits, multiple producers, lost wakeup regression, cancel, clear, mid-stream concurrency change, context cancellation, watchdog, panic recovery, shutdown, and queue size. All pass with `-race`.

2. **LaneNested is defined but never enqueued to** in the current codebase. Reserved for future recursive tool chains.

3. **Two concurrency layers for sub-agents:** The orchestrator has its own `maxConcurrent` (default 5) and spawns goroutines directly. The LaneSubagent only applies to recovery re-enqueue. These are independent — the orchestrator does NOT use the LaneManager at runtime.

4. **Voice routes through LaneMain:** `voice.DuplexDeps.RunnerFunc` routes through `state.lanes.Enqueue(ctx, LaneMain, ...)` — voice is subject to lane concurrency control and gets the same backpressure as text input. With LaneMain at concurrency 2, voice and text can run simultaneously.

5. **Duplicate lane constants:** The orchestrator package (`internal/agent/orchestrator/orchestrator.go:59-66`) re-declares lane constants with identical values. These shadow the canonical ones in `agenthub`.

6. **Config zero vs runtime zero:** In `config.yaml`, `0` means "use default" (the `> 0` check skips it). At runtime, `MaxConcurrent = 0` means unlimited. This distinction matters: you cannot set a lane to unlimited via config — you'd need to set it to a very large number and let the hard cap clamp it.

7. **Events are fire-and-forget:** `emit()` wraps the callback in `go fn(event)` — non-blocking but unordered relative to lane work.

8. **CancelActive vs ClearLane:** The `cancel` WebSocket frame only calls `CancelActive` (abort running task), NOT `ClearLane` (drain queue). The comment in agent.go explains the race condition that would occur.

9. **Recovery uses TaskType, not persisted Lane:** When re-enqueueing after restart, the lane is determined by `task.TaskType`, not `task.Lane`. The persisted lane field is metadata-only for this purpose.

10. **Runner per-run state is local:** The `Runner` struct no longer holds per-run mutable state (`lastInputTokens`, `cachedThresholds`, `promptOverhead`). These are extracted into a local `runState` struct created per `Run()` call and passed through the run loop. This eliminates data races when LaneMain concurrency > 1.

11. **Per-session compaction mutex:** A `sync.Map` on the Runner keyed by sessionID guards the compaction decision block. Two concurrent runs on the same session cannot both compact simultaneously.

12. **Bridge handlers removed:** Telegram/Discord/Slack bridge handling (`OnChannelMessage` and `ChannelMessage` types) has been deleted. NeboLoop has its own mobile app and web chat — bridges are no longer needed.

13. **Desktop tool queuing:** Desktop-category tools are intercepted in `Registry.Execute()` and routed through a `DesktopQueueFunc` callback. The callback is wired in agent.go to enqueue on LaneDesktop. The tool list: `desktop`, `accessibility`, `screenshot`, `app`, `browser`, `window`, `menubar`, `dialog`, `shortcuts`.

14. **Cross-lane tools:** Three tools support cross-lane awareness:
    - `notify_owner` — any lane can post a notification to the owner's Main Chat session
    - `query_sessions` — Main Chat can read messages from any session (loop channels, DMs, etc.)
    - `message_send` — extended to support `loop:{channelID}` for posting to loop channels via NeboLoop SDK

15. **Channel skill bindings:** Stored in `channel_skills` SQLite table (channel_id, skill_name). The loop channel handler checks for bindings before building the RunRequest and sets `ForceSkill` if found.
