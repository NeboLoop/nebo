# Lanes and Hub: Complete Logic Deep-Dive

This document captures every detail of the Go `internal/agenthub/` package, sufficient for reimplementation in Rust without referencing the Go source.

---

## Table of Contents

1. [Package Overview](#1-package-overview)
2. [Lane Types and Concurrency Limits](#2-lane-types-and-concurrency-limits)
3. [Data Structures](#3-data-structures)
4. [LaneManager: Initialization and Lifecycle](#4-lanemanager-initialization-and-lifecycle)
5. [Enqueue / EnqueueAsync Mechanism](#5-enqueue--enqueueasync-mechanism)
6. [The Pump: run() and processAvailable()](#6-the-pump-run-and-processavailable)
7. [Backpressure Handling](#7-backpressure-handling)
8. [Lane Operations: Clear, Cancel, Stats, Shutdown](#8-lane-operations-clear-cancel-stats-shutdown)
9. [Hub Struct: All Fields and Initialization](#9-hub-struct-all-fields-and-initialization)
10. [Frame Routing](#10-frame-routing)
11. [Connection Management (WebSocket)](#11-connection-management-websocket)
12. [Event Broadcasting](#12-event-broadcasting)
13. [Synchronous Request/Response](#13-synchronous-requestresponse)
14. [Mutex/Lock Patterns](#14-mutexlock-patterns)
15. [Error Handling and Recovery](#15-error-handling-and-recovery)
16. [Shutdown and Cleanup](#16-shutdown-and-cleanup)
17. [Timers, Timeouts, and Periodic Behavior](#17-timers-timeouts-and-periodic-behavior)
18. [Enqueue Options (Functional Options Pattern)](#18-enqueue-options-functional-options-pattern)
19. [Context Propagation](#19-context-propagation)
20. [Lifecycle Events Integration](#20-lifecycle-events-integration)

---

## 1. Package Overview

Package: `agenthub`

Two logical subsystems in one package:

- **Lanes** (`lane.go`): A multi-lane work queue system with per-lane concurrency limits, backpressure, and lifecycle events. Each lane has its own pump goroutine.
- **Hub** (`hub.go`): A WebSocket connection manager for agent processes. Manages registration, deregistration, frame routing, and synchronous request/response correlation.

Shared logger: `hubLog = logging.L("AgentHub")` (package-level variable in `hub.go`, used by both files).

---

## 2. Lane Types and Concurrency Limits

### Lane Type Constants (string values)

```
LaneMain      = "main"       // Primary user interactions (phone + desktop + voice)
LaneEvents    = "events"     // Scheduled/triggered tasks
LaneSubagent  = "subagent"   // Sub-agent operations
LaneNested    = "nested"     // Nested tool calls
LaneHeartbeat = "heartbeat"  // Proactive heartbeat ticks
LaneComm      = "comm"       // Inter-agent communication messages
LaneDev       = "dev"        // Developer assistant (independent of main lane)
LaneDesktop   = "desktop"    // Serialized desktop automation (one screen, one mouse)
```

### Default Concurrency (map[string]int)

| Lane        | Default Max Concurrent | Semantics                                        |
|-------------|------------------------|--------------------------------------------------|
| `main`      | 2                      | Concurrent phone + desktop + voice on same session |
| `events`    | 0 (unlimited)          | Each scheduled task gets own session              |
| `subagent`  | 5                      | Max 5 concurrent sub-agents (backpressure)        |
| `nested`    | 3                      | Nested tool calls                                 |
| `heartbeat` | 1                      | Sequential heartbeat processing                   |
| `comm`      | 5                      | Concurrent comm message processing                |
| `dev`       | 1                      | Serialized per project                            |
| `desktop`   | 1                      | One screen, one mouse, one keyboard               |

**A value of 0 means unlimited concurrency.** Any positive integer is the max.

### Hard Caps (map[string]int)

These cannot be exceeded even via `SetConcurrency()`:

| Lane       | Hard Cap |
|------------|----------|
| `nested`   | 3        |
| `subagent` | 10       |

If `SetConcurrency()` is called with a value exceeding the hard cap (or 0/unlimited), the hard cap is enforced instead. Only lanes listed in `MaxLaneConcurrency` have hard caps; all others can be set to any value including unlimited.

### Fallback for Unknown Lanes

If a lane name is not in `DefaultLaneConcurrency`, its default max concurrent is **1**.

---

## 3. Data Structures

### LaneTask

```rust
// Rust equivalent
struct LaneTask {
    id: String,              // Format: "{lane}-{unix_nanos}" e.g. "main-1709571234567890000"
    lane: String,
    description: String,     // Human-readable, set via WithDescription option
    task: Box<dyn FnOnce(Context) -> Result<(), Error>>,  // The actual work
    enqueued_at: Instant,    // When Enqueue/EnqueueAsync was called
    started_at: Instant,     // When processAvailable() dequeued and started it
    completed_at: Instant,   // When the task function returned
    error: Option<Error>,    // Error from task execution (or panic recovery)
    on_wait: Option<Box<dyn Fn(i64, i32)>>,  // Callback(wait_ms, queued_ahead) if wait exceeds threshold
    warn_after_ms: i64,      // Threshold for triggering on_wait (default: 2000ms)
}
```

### laneEntry (internal, not exported)

```rust
struct LaneEntry {
    task: LaneTask,
    resolve: oneshot::Sender<Result<(), Error>>,  // Buffered channel of size 1 in Go
    ctx: Context,               // Derived from caller's context with lane value added
    cancel: CancellationToken,  // Cancel function for ctx
}
```

### LaneState

```rust
struct LaneState {
    lane: String,
    queue: Vec<LaneEntry>,     // Pending tasks (FIFO)
    active: Vec<LaneEntry>,    // Currently executing tasks
    max_concurrent: usize,     // 0 = unlimited
    notify: mpsc::Sender<()>,  // Buffered(1) wakeup signal for pump goroutine
    stop_ch: broadcast::Sender<()>,  // Close to stop pump goroutine
    mu: Mutex,                 // Protects queue, active, max_concurrent
}
```

### LaneManager

```rust
struct LaneManager {
    mu: RwLock,                          // Protects lanes map
    lanes: HashMap<String, LaneState>,
    on_event: Option<Box<dyn Fn(LaneEvent)>>,  // Lifecycle event callback
}
```

### LaneTaskInfo (JSON-serializable summary)

```rust
struct LaneTaskInfo {
    id: String,
    description: String,
    enqueued_at: i64,    // Unix milliseconds
    started_at: i64,     // Unix milliseconds, 0 if not started
}
```

### LaneStats (JSON-serializable)

```rust
struct LaneStats {
    lane: String,
    queued: usize,
    active: usize,
    max_concurrent: usize,
    active_tasks: Vec<LaneTaskInfo>,
    queued_tasks: Vec<LaneTaskInfo>,
}
```

### LaneEvent (JSON-serializable lifecycle event)

```rust
struct LaneEvent {
    type_: String,       // "task_enqueued", "task_started", "task_completed", "task_cancelled"
    lane: String,
    task: LaneTaskInfo,
}
```

### enqueueConfig (internal options struct)

```rust
struct EnqueueConfig {
    warn_after_ms: i64,     // Default: 2000
    description: String,
    on_wait: Option<Box<dyn Fn(i64, i32)>>,  // (wait_ms, queued_ahead)
}
```

---

## 4. LaneManager: Initialization and Lifecycle

### Construction

```go
func NewLaneManager() *LaneManager
```

Creates an empty `LaneManager` with an initialized (empty) `lanes` map. No lanes are created at construction time. No event callback is registered.

### Lane Creation (Lazy)

Lanes are created on first access via `getLaneState(lane)`:

1. Acquire write lock on `m.mu`.
2. Check if lane already exists in `m.lanes` -- if so, return it.
3. Look up `DefaultLaneConcurrency[lane]`; if not found, default to `maxConcurrent = 1`.
4. Create `LaneState` with:
   - `Lane`: the lane name
   - `Queue`: empty slice
   - `MaxConcurrent`: from step 3
   - `notify`: buffered channel of size 1
   - `stopCh`: unbuffered channel (closed to signal stop)
5. Store in `m.lanes[lane]`.
6. Spawn a goroutine: `go state.run(m)` -- this is the pump.
7. Return the state.

**Important**: The pump goroutine starts immediately on first access. It runs forever until `stopCh` is closed.

### Event Registration

```go
func (m *LaneManager) OnEvent(fn func(LaneEvent))
```

Sets the `onEvent` callback. Not mutex-protected (set once at startup). The `emit()` method calls `fn` in a new goroutine (`go fn(event)`), so events are non-blocking and asynchronous.

---

## 5. Enqueue / EnqueueAsync Mechanism

### Enqueue (Synchronous -- blocks until task completes)

```go
func (m *LaneManager) Enqueue(ctx context.Context, lane string, task func(ctx context.Context) error, opts ...EnqueueOption) error
```

**Algorithm:**

1. If `lane` is empty string, default to `LaneMain` ("main").
2. Build `enqueueConfig` with defaults (`warnAfterMs: 2000`), apply option functions.
3. Call `getLaneState(lane)` to get or create the lane (spawning pump if new).
4. Create a derived context: `taskCtx, cancel = context.WithCancel(WithLane(ctx, lane))`.
   - `WithLane()` stores the lane name in the context via a private key type.
5. Build `laneEntry`:
   - `task.ID` = `"{lane}-{unix_nanos}"`
   - `task.EnqueuedAt` = `time.Now()`
   - `resolve` = buffered channel of size 1
6. Lock `state.mu`, append entry to `state.Queue`, compute queue size (queued + active), unlock.
7. Log: "enqueued task".
8. Emit `LaneEvent{Type: "task_enqueued", ...}` (async, in goroutine).
9. Send wakeup signal to `state.notify` (non-blocking: `select` with `default`).
10. **Block** on `select`:
    - `case err := <-entry.resolve:` -- task completed, return err.
    - `case <-ctx.Done():` -- caller cancelled, call `cancel()`, return `ctx.Err()`.

**Critical detail**: When the caller's context is cancelled, `cancel()` is called on the task's derived context. However, the task entry remains in the queue. The task may still execute later (its context will be cancelled). The caller simply stops waiting.

### EnqueueAsync (Fire-and-forget)

```go
func (m *LaneManager) EnqueueAsync(ctx context.Context, lane string, task func(ctx context.Context) error, opts ...EnqueueOption)
```

**Identical to Enqueue** in steps 1-9, but **does NOT block** on `entry.resolve`. Returns immediately after enqueueing. The resolve channel is still created (the pump writes to it on completion), but nobody reads it in the async case.

### Key Difference

| Aspect | Enqueue | EnqueueAsync |
|--------|---------|--------------|
| Returns | After task completes (or ctx cancelled) | Immediately after enqueueing |
| Error | Returns task error | No error returned |
| Blocking | Yes | No |
| Context cancellation | Unblocks caller with ctx.Err() | No effect on caller (already returned) |

---

## 6. The Pump: run() and processAvailable()

### run() -- Per-Lane Pump Goroutine

```go
func (s *LaneState) run(mgr *LaneManager)
```

**Infinite loop:**

```
loop {
    select {
        case <-s.notify:
            s.processAvailable(mgr)
        case <-s.stopCh:
            return
    }
}
```

The pump sleeps until either:
- A wakeup signal arrives on `notify` (buffered channel of size 1).
- The `stopCh` channel is closed (shutdown).

### processAvailable() -- Dequeue and Execute

```go
func (s *LaneState) processAvailable(mgr *LaneManager)
```

**Algorithm (inner loop, drains queue up to capacity):**

```
loop {
    1. Lock s.mu
    2. Check capacity:
       - at_capacity = (s.MaxConcurrent > 0) AND (len(s.active) >= s.MaxConcurrent)
       - If at_capacity OR queue is empty: unlock and return
    3. Pop first entry from s.Queue (FIFO)
    4. Compute waited_ms = time.Since(entry.task.EnqueuedAt)
    5. If waited_ms >= warn_after_ms AND on_wait callback exists:
       - Call on_wait(waited_ms, len(s.Queue))
       - Log warning
    6. Append entry to s.active
    7. Set entry.task.StartedAt = time.Now()
    8. Capture snapshot: startedInfo, activeCount, queuedCount
    9. Unlock s.mu
    10. Log: "dequeued task"
    11. Spawn goroutine for task execution (see below)
    // Loop continues -- will dequeue more if capacity allows
}
```

### Task Execution Goroutine (spawned in step 11)

For each dequeued task, a new goroutine runs:

```
goroutine {
    1. Emit LaneEvent{Type: "task_started", ...}

    2. Execute task with panic recovery:
       defer func() {
           if r := recover() {
               crashlog.LogPanic("lane", r, {"lane": s.Lane})
               err = fmt.Errorf("panic in lane task: %v", r)
           }
       }()
       err = entry.task.Task(entry.ctx)

    3. Lock s.mu
    4. Set entry.task.CompletedAt, entry.task.Error
    5. Remove this entry from s.active (linear scan by pointer equality)
    6. Capture durationMs, activeAfter, queuedAfter
    7. Unlock s.mu

    8. Log result (error or success)
    9. Emit LaneEvent{Type: "task_completed", ...}

    10. Send result: entry.resolve <- err
    11. Close entry.resolve channel

    12. Signal pump that capacity freed up:
        select {
            case s.notify <- struct{}{}:
            default:
        }
}
```

**Critical details:**
- Step 5: Active slice removal is O(n) linear scan comparing pointers.
- Step 10-11: Write result to resolve channel, then close it. The channel is buffered(1), so the write never blocks.
- Step 12: After a task completes, the pump is re-signaled. This is how queued tasks get promoted when capacity frees up. The non-blocking send ensures no deadlock if the pump already has a pending notification.

### Wakeup Signal Protocol

The `notify` channel is `make(chan struct{}, 1)` -- buffered with capacity 1.

All wakeup sends use the non-blocking pattern:
```go
select {
case state.notify <- struct{}{}:
default:
}
```

This means at most one wakeup can be queued. Multiple rapid enqueues or completions coalesce into a single wakeup. The pump's `processAvailable()` drains the entire queue up to capacity on each wakeup, so no tasks are lost.

**Lost wakeup prevention**: When a task completes (step 12 above), it re-signals the pump. Even if the pump was already notified by an enqueue, the inner loop in `processAvailable()` will keep dequeuing until at capacity or queue empty.

---

## 7. Backpressure Handling

### When a Lane is Full

A lane is "full" when `len(s.active) >= s.MaxConcurrent` (and `MaxConcurrent > 0`).

When full:
1. `processAvailable()` returns without dequeuing.
2. New tasks enqueued via `Enqueue()` sit in the queue. The caller blocks on `entry.resolve`.
3. New tasks enqueued via `EnqueueAsync()` sit in the queue. The caller returns immediately.
4. Tasks remain in FIFO order.
5. When an active task completes, it signals the pump, which dequeues the next task.

### Wait Warning

If a task waits in the queue longer than `WarnAfterMs` (default 2000ms):
- The `OnWait` callback is invoked with `(waitedMs, queuedAhead)`.
- A warning is logged.

This happens at dequeue time (when the task is about to start), not while waiting.

### Unlimited Lanes

When `MaxConcurrent == 0`, the capacity check `s.MaxConcurrent > 0 && len(s.active) >= s.MaxConcurrent` is false (short-circuit), so tasks are dequeued immediately with no limit.

### Context Cancellation as Escape Hatch

A caller blocked in `Enqueue()` can cancel their context to stop waiting. This does NOT remove the task from the queue -- the task may still execute (with a cancelled context). The caller simply gets `context.Canceled`.

---

## 8. Lane Operations: Clear, Cancel, Stats, Shutdown

### ClearLane

```go
func (m *LaneManager) ClearLane(lane string) int
```

Removes all **queued** (not active) tasks from a lane:

1. Default empty lane to "main".
2. Get lane state (read lock on manager, then lock lane).
3. For each queued entry:
   - Call `entry.cancel()` to cancel the task's context.
   - Write `context.Canceled` to `entry.resolve`.
   - Close `entry.resolve`.
4. Replace queue with empty slice.
5. Return count of removed entries.

**Active tasks are NOT affected.** They continue running.

### CancelActive

```go
func (m *LaneManager) CancelActive(lane string) int
```

Cancels all **active** (running) tasks in a lane:

1. Default empty lane to "main".
2. Get lane state.
3. Lock lane, iterate active entries:
   - Emit `LaneEvent{Type: "task_cancelled", ...}` for each.
   - Call `entry.cancel()`.
4. Unlock, return count.

**Does NOT remove entries from the active slice.** The task goroutines detect cancellation via their context, return an error, and the normal completion path removes them from active and signals the pump.

### GetQueueSize

```go
func (m *LaneManager) GetQueueSize(lane string) int
```

Returns `len(queue) + len(active)` for a single lane. Returns 0 if lane doesn't exist.

### GetTotalQueueSize

```go
func (m *LaneManager) GetTotalQueueSize() int
```

Sums `len(queue) + len(active)` across all lanes.

### GetLaneStats

```go
func (m *LaneManager) GetLaneStats() map[string]LaneStats
```

Returns a snapshot of all lanes with their queued count, active count, max concurrent, and task info lists.

### SetConcurrency

```go
func (m *LaneManager) SetConcurrency(lane string, maxConcurrent int)
```

1. Get or create lane state.
2. Lock lane.
3. Clamp negative to 0 (unlimited).
4. If lane has a hard cap in `MaxLaneConcurrency`, enforce it: if `maxConcurrent == 0` (unlimited) or exceeds hard cap, set to hard cap.
5. Set `state.MaxConcurrent`.
6. Unlock.
7. Signal pump (non-blocking send to `notify`).

The pump wakeup allows previously blocked tasks to start if capacity increased.

---

## 9. Hub Struct: All Fields and Initialization

### Frame (JSON wire format)

```rust
struct Frame {
    type_: String,           // "req", "res", "event", "stream", "approval_request",
                             // "approval_response", "ask_request", "ask_response"
    id: Option<String>,      // Correlation ID for req/res pairs
    method: Option<String>,  // For requests: "ping", "status", etc.
    params: Option<Value>,   // Request parameters (any JSON)
    ok: Option<bool>,        // Response success flag
    payload: Option<Value>,  // Response data (any JSON)
    error: Option<String>,   // Error message string
}
```

### AgentConnection

```rust
struct AgentConnection {
    id: String,                     // Unique agent ID (passed in from handler)
    name: String,                   // Agent name: "main", "coder", "researcher", etc.
    conn: WebSocketConnection,      // gorilla/websocket.Conn
    send: mpsc::Sender<Vec<u8>>,    // Buffered channel, capacity 256
    created_at: Instant,
    metadata: HashMap<String, Value>,
}
```

### Hub Fields

```rust
struct Hub {
    // Multi-agent registry: name -> connection
    agent_mu: RwLock,
    agents: HashMap<String, AgentConnection>,

    // Registration channels (buffered, capacity 1 each)
    register: mpsc::Sender<AgentConnection>,
    unregister: mpsc::Sender<AgentConnection>,

    // Response handler + its lock
    response_handler: Option<Box<dyn Fn(String, &Frame)>>,
    response_handler_mu: RwLock,

    // Approval handler + its lock
    approval_handler: Option<Box<dyn Fn(String, String, String, Vec<u8>)>>,
    approval_handler_mu: RwLock,

    // Ask handler + its lock
    ask_handler: Option<Box<dyn Fn(String, String, String, Vec<u8>)>>,
    ask_handler_mu: RwLock,

    // Event handler + its lock
    event_handler: Option<Box<dyn Fn(String, &Frame)>>,
    event_handler_mu: RwLock,

    // Sync request/response tracking
    pending_sync: HashMap<String, PendingSync>,
    pending_sync_mu: RwLock,

    // WebSocket upgrader
    upgrader: WebSocketUpgrader,
}

struct PendingSync {
    ch: oneshot::Sender<Frame>,  // Buffered channel, capacity 1
}
```

### NewHub()

```go
func NewHub() *Hub
```

Creates Hub with:
- `agents`: empty map
- `register`: buffered channel, capacity 1
- `unregister`: buffered channel, capacity 1
- `upgrader`: gorilla/websocket.Upgrader with:
  - `ReadBufferSize`: 1024
  - `WriteBufferSize`: 1024
  - `CheckOrigin`: allows empty origin or localhost origins (via `middleware.IsLocalhostOrigin`)

All handler fields are nil. `pendingSync` map is nil (lazily initialized on first `SendRequestSync`).

---

## 10. Frame Routing

### handleFrame() -- Central Router

Called by `readPump()` for every message received from an agent.

```
match frame.type_ {
    "res" => {
        // 1. Check if this is a sync response (correlating to SendRequestSync)
        if routeSyncResponse(frame) { return }
        // 2. Otherwise, route to the response handler
        if response_handler != nil { response_handler(agent_id, frame) }
    }

    "stream" => {
        // Streaming chunk -- same handler as responses
        if response_handler != nil { response_handler(agent_id, frame) }
    }

    "approval_request" => {
        // Extract tool name and input from payload
        // payload is expected to be {"tool": "...", "input": {...}}
        if approval_handler != nil {
            tool_name = payload["tool"]
            input_raw = json.Marshal(payload["input"])
            approval_handler(agent_id, frame.id, tool_name, input_raw)
        }
    }

    "ask_request" => {
        // Interactive prompt from agent
        // payload is expected to be {"prompt": "...", "widgets": [...]}
        if ask_handler != nil {
            prompt = payload["prompt"]
            widgets_raw = json.Marshal(payload["widgets"])
            ask_handler(agent_id, frame.id, prompt, widgets_raw)
        }
    }

    "event" => {
        // Log the event, route to event handler
        if event_handler != nil { event_handler(agent_id, frame) }
        else { log warning "no event handler registered" }
    }

    "req" => {
        // Agent-initiated request -- handle locally
        handleRequest(agent, frame)
    }
}
```

### handleRequest() -- Agent-Initiated Requests

Handles requests FROM the agent (not from clients):

```
match frame.method {
    "ping" => {
        response.ok = true
        response.payload = {"pong": true, "time": unix_timestamp}
    }

    "status" => {
        response.ok = true
        response.payload = {
            "agent_id": agent.id,
            "connected": true,
            "uptime_sec": seconds_since(agent.created_at)
        }
    }

    _ => {
        response.ok = false
        response.error = "unknown method: {method}"
    }
}

// Response is sent directly to the agent's Send channel (unbuffered write)
agent.Send <- json.Marshal(response)
```

**Note**: The response send in `handleRequest` uses a direct channel write (NOT the non-blocking select pattern). This could block if the agent's send buffer is full. This is a potential issue but unlikely since the buffer is 256.

---

## 11. Connection Management (WebSocket)

### HandleWebSocket -- Entry Point

```go
func (h *Hub) HandleWebSocket(w http.ResponseWriter, r *http.Request, agentID string)
```

1. Upgrade HTTP to WebSocket via `h.upgrader.Upgrade(w, r, nil)`.
2. Parse agent name from URL query: `?name=main` (default: "main").
3. Create `AgentConnection`:
   - `ID`: passed in `agentID`
   - `Name`: from query param
   - `Conn`: the WebSocket connection
   - `Send`: buffered channel, **capacity 256**
   - `CreatedAt`: now
   - `Metadata`: empty map
4. Send to `h.register` channel.
5. Spawn two goroutines: `readPump(agent)` and `writePump(agent)`.

### addAgent -- Registration

```go
func (h *Hub) addAgent(newAgent *AgentConnection)
```

1. Lock `agentMu` (write lock).
2. Default name to "main" if empty.
3. If an agent with the same name already exists:
   - Close its `Send` channel.
   - Close its WebSocket connection.
   - Emit `EventAgentDisconnected` lifecycle event.
   - **This disconnects the old agent before registering the new one.**
4. Store new agent: `h.agents[name] = newAgent`.
5. Emit `EventAgentConnected` lifecycle event.
6. Spawn goroutine to send "ready" event to the agent:
   - Frame: `{type: "event", method: "ready", payload: {agent_id, name}}`
   - Non-blocking send to `agent.Send` (drops if buffer full).

### removeAgent -- Deregistration

```go
func (h *Hub) removeAgent(agent *AgentConnection)
```

1. Lock `agentMu` (write lock).
2. Default name to "main" if empty.
3. **Only remove if the registered agent for this name has the same ID as the agent being removed.** This prevents double-removal when `addAgent` already replaced the old agent.
4. If matched:
   - Close `agent.Send` (with panic recovery -- channel may already be closed by `addAgent`).
   - Close WebSocket connection.
   - Delete from `h.agents`.
   - Emit `EventAgentDisconnected`.

### Agent Lookup Methods

| Method | Lookup Strategy | Fallback |
|--------|----------------|----------|
| `GetTheAgent()` | By name "main" | None |
| `GetAgentByName(name)` | By name (default "main" if empty) | None |
| `GetAgent(agentID)` | Linear scan by ID | None |
| `GetAnyAgent()` | Returns first from map iteration | None |
| `GetAllAgents()` | Returns all as slice | Empty slice |
| `IsConnected()` | len(agents) > 0 | -- |
| `IsAgentConnected(name)` | Map lookup by name | -- |
| `AgentCount()` | len(agents) | -- |

All methods acquire `agentMu` read lock.

### readPump -- Incoming Messages

```go
func (h *Hub) readPump(agent *AgentConnection)
```

**Setup:**
- `defer`: send agent to `h.unregister` channel on exit.
- Set read limit: **10 MB** (`10 * 1024 * 1024`).
- Set initial read deadline: **10 minutes**.
- Set pong handler: resets read deadline to 10 minutes.
- Set ping handler: resets read deadline to 10 minutes, responds with pong (write control frame with 1s deadline).

**Loop:**
1. `ReadMessage()` -- blocks until message or error.
2. On error:
   - If unexpected close error (not GoingAway or AbnormalClosure), log error.
   - Break loop (triggers deferred unregister).
3. Reset read deadline to 10 minutes (belt-and-suspenders with pong handler).
4. Unmarshal JSON to `Frame`.
5. On unmarshal error: log error with first 200 bytes preview, `continue` (skip frame).
6. Call `handleFrame(agent, &frame)`.

### writePump -- Outgoing Messages

```go
func (h *Hub) writePump(agent *AgentConnection)
```

**Setup:**
- Create ticker: **30 seconds** (for ping keep-alive).
- `defer`: stop ticker, close WebSocket connection.

**Loop:**
```
select {
    case message, ok := <-agent.Send:
        - Set write deadline: 10 seconds
        - If channel closed (!ok): send WebSocket close message, return
        - Get NextWriter for TextMessage
        - Write message bytes
        - Close writer
        - On any write error: return (triggers deferred close)

    case <-ticker.C:
        - Set write deadline: 10 seconds
        - Send WebSocket PingMessage
        - On error: return
}
```

**Note**: Unlike some WebSocket implementations, this does NOT batch multiple pending messages in a single write. Each message gets its own NextWriter/Close cycle.

---

## 12. Event Broadcasting

### Broadcast (to all agents)

```go
func (h *Hub) Broadcast(frame *Frame)
```

1. Read lock `agentMu`, copy all agents to a local slice, unlock.
2. Marshal frame to JSON.
3. For each agent, non-blocking send:
   ```
   select {
       case agent.Send <- data:
       default:
           // Skip agents with full buffers
   }
   ```

### SendToAgent (by ID, with fallback)

```go
func (h *Hub) SendToAgent(agentID string, frame *Frame) error
```

1. Look up agent by ID (linear scan).
2. **If not found, fall back to main agent** (`GetTheAgent()`).
3. If still not found: return error "agent not connected".
4. Marshal frame.
5. Non-blocking send. Returns error "agent send buffer full" if buffer is full.

### SendToAgentByName (by name, no fallback)

```go
func (h *Hub) SendToAgentByName(name string, frame *Frame) error
```

1. Look up by name.
2. If not found: return error "agent {name} not connected".
3. Marshal + non-blocking send. Error if buffer full.

### Send (convenience for main agent)

```go
func (h *Hub) Send(frame *Frame) error
```

Calls `SendToAgentByName("main", frame)`.

### SendApprovalResponse / SendApprovalResponseWithAlways

```go
func (h *Hub) SendApprovalResponse(agentID, requestID string, approved bool) error
func (h *Hub) SendApprovalResponseWithAlways(agentID, requestID string, approved, always bool) error
```

Sends to main agent (ignores agentID -- always uses `h.Send()`):
- Frame type: `"approval_response"`
- Frame ID: requestID
- Payload: `{"approved": bool, "always": bool}`

### SendAskResponse

```go
func (h *Hub) SendAskResponse(agentID, requestID, value string) error
```

Sends to main agent:
- Frame type: `"ask_response"`
- Frame ID: requestID
- Payload: `{"request_id": requestID, "value": value}`

---

## 13. Synchronous Request/Response

### SendRequestSync

```go
func (h *Hub) SendRequestSync(ctx context.Context, method string, params map[string]any) (*Frame, error)
```

**Algorithm:**

1. Generate correlation ID: `"sync-{unix_nanos}"`.
2. Build request frame: `{type: "req", id: id, method: method, params: params}`.
3. Create response channel: `make(chan *Frame, 1)` (buffered 1).
4. Lock `pendingSyncMu`, lazily initialize map if nil, store `pendingSync{ch}` keyed by ID, unlock.
5. Defer: lock `pendingSyncMu`, delete pending entry, unlock.
6. Send frame via `h.Send()`. If error (no agent), return immediately.
7. Block on select:
   - `case resp := <-ch:` -- return response frame.
   - `case <-ctx.Done():` -- return ctx.Err().

### routeSyncResponse

```go
func (h *Hub) routeSyncResponse(frame *Frame) bool
```

Called by `handleFrame` for every "res" frame before the generic response handler:

1. Read lock `pendingSyncMu`, look up `frame.ID` in `pendingSync` map, unlock.
2. If found: non-blocking send frame to `ps.ch`. Return true.
3. If not found: return false (let normal response handler process it).

---

## 14. Mutex/Lock Patterns

### LaneManager Locks

| Lock | Type | Protects | Held During |
|------|------|----------|-------------|
| `LaneManager.mu` | `sync.RWMutex` | `lanes` map | getLaneState (write), GetQueueSize/GetTotalQueueSize/GetLaneStats/ClearLane/CancelActive/Shutdown (read) |
| `LaneState.mu` | `sync.Mutex` | `Queue`, `active`, `MaxConcurrent` | Enqueue (append), processAvailable (pop/remove), ClearLane, CancelActive, GetLaneStats, SetConcurrency |

**Lock ordering**: Always `LaneManager.mu` first (read), then `LaneState.mu`. Never reversed. `getLaneState` holds `LaneManager.mu` (write) but does NOT lock `LaneState.mu`.

### Hub Locks

| Lock | Type | Protects |
|------|------|----------|
| `agentMu` | `sync.RWMutex` | `agents` map |
| `responseHandlerMu` | `sync.RWMutex` | `responseHandler` |
| `approvalHandlerMu` | `sync.RWMutex` | `approvalHandler` |
| `askHandlerMu` | `sync.RWMutex` | `askHandler` |
| `eventHandlerMu` | `sync.RWMutex` | `eventHandler` |
| `pendingSyncMu` | `sync.RWMutex` | `pendingSync` map |

Each handler has its own RWMutex. Handlers are read-locked during dispatch, write-locked during registration. This allows concurrent frame processing without blocking handler setup.

### Lock Duration

All locks are held for minimal durations:
- `agentMu`: only during map operations, not during WebSocket I/O.
- Handler locks: only to read the handler reference, not during handler execution.
- `LaneState.mu`: only during queue manipulation, not during task execution.

---

## 15. Error Handling and Recovery

### Panic Recovery in Lane Tasks

Every task execution is wrapped in a deferred panic recovery:

```go
defer func() {
    if r := recover(); r != nil {
        crashlog.LogPanic("lane", r, map[string]string{"lane": s.Lane})
        err = fmt.Errorf("panic in lane task: %v", r)
    }
}()
```

If a task panics:
1. The panic is caught.
2. It is logged via `crashlog.LogPanic`.
3. An error is synthesized: `"panic in lane task: {panic_value}"`.
4. The error is returned through the normal completion path (written to `resolve` channel).
5. The task is removed from `active`.
6. The pump is re-signaled.
7. **The lane continues operating normally.** Subsequent tasks are unaffected.

### WebSocket Error Handling

- `readPump`: On read error, logs if unexpected close, then exits (triggering unregister).
- `writePump`: On write error, returns (triggering deferred close).
- `handleFrame`: On JSON unmarshal error, logs preview and continues (does not disconnect).

### Channel Close Safety

`removeAgent` uses `defer recover()` around channel close to handle the case where `addAgent` already closed the channel.

### Send Buffer Full

All send operations use non-blocking select. If the agent's 256-slot buffer is full:
- `SendToAgent`/`SendToAgentByName`: return error "agent send buffer full".
- `Broadcast`: silently skip the agent.
- `addAgent` ready event: silently skip (log warning).

---

## 16. Shutdown and Cleanup

### LaneManager.Shutdown()

```go
func (m *LaneManager) Shutdown()
```

1. Read lock `m.mu`.
2. For each lane state: close `state.stopCh` (if not already closed, checked via select).
3. Unlock.

**Effects:**
- Each pump goroutine (`run()`) exits when it receives from `stopCh`.
- Active tasks are NOT cancelled or waited on.
- Queued tasks are NOT drained or cancelled.
- Double-shutdown is safe (select checks if channel is already closed).

### Hub.Run() Context Cancellation

```go
func (h *Hub) Run(ctx context.Context)
```

When `ctx` is cancelled, the select loop exits. This stops processing register/unregister channels. Existing agent connections are NOT explicitly cleaned up by `Run()`.

### Agent Disconnect Cleanup

When an agent disconnects:
1. `readPump` exits, sends agent to `unregister` channel.
2. `removeAgent` closes `Send` channel and WebSocket connection.
3. `writePump` detects closed `Send` channel, sends WebSocket close message, exits.

When an agent reconnects with the same name:
1. `addAgent` disconnects the old agent (close Send, close Conn).
2. Old `writePump` detects closed channel, exits.
3. Old `readPump` detects closed Conn, sends to unregister.
4. `removeAgent` does nothing (ID mismatch check prevents double-cleanup).

---

## 17. Timers, Timeouts, and Periodic Behavior

### WebSocket Timeouts

| Timeout | Value | Purpose |
|---------|-------|---------|
| Read deadline | 10 minutes | Max time between messages from agent |
| Write deadline | 10 seconds | Max time to write a single message |
| Ping interval | 30 seconds | Keep-alive ping from server to agent |
| Pong handler | Resets read deadline to 10 minutes | Agent responded to ping |
| Ping handler | Resets read deadline to 10 minutes + sends pong (1s deadline) | Agent initiated ping |

### Lane Wait Warning

Default `WarnAfterMs`: 2000ms. Configurable per-enqueue via `WithWarnAfter(ms)`.

Checked at dequeue time: if `time.Since(enqueued_at) >= warn_after_ms`, the `OnWait` callback fires.

### Read Limit

Max WebSocket message size: **10 MB** (`10 * 1024 * 1024`). Comment notes: "tool results can be large".

---

## 18. Enqueue Options (Functional Options Pattern)

Three option functions are available:

### WithWarnAfter(ms int64)

Sets `enqueueConfig.warnAfterMs`. Controls when the wait-too-long warning triggers.

### WithOnWait(fn func(waitMs int64, queuedAhead int))

Sets `enqueueConfig.onWait`. Called once when the task is dequeued if wait exceeded threshold. Parameters:
- `waitMs`: actual milliseconds waited in queue
- `queuedAhead`: number of tasks still ahead in queue at dequeue time

### WithDescription(desc string)

Sets `enqueueConfig.description`. Used in `LaneTaskInfo` for observability/debugging.

---

## 19. Context Propagation

### Lane Context Value

```go
type laneContextKey struct{}

func WithLane(ctx context.Context, lane string) context.Context
func GetLane(ctx context.Context) string  // returns "" if not set
```

Every task context carries its lane name. This allows code deep in the call stack to know which lane it's running in.

### Context Chain

For `Enqueue()`:
```
caller_ctx
  -> WithCancel (creates taskCtx with cancel)
    -> WithLane(taskCtx, lane)
      -> passed to task function
```

The cancel function is stored in `laneEntry.cancel` and can be triggered by:
1. `CancelActive()` -- explicitly cancels running tasks.
2. `ClearLane()` -- cancels queued tasks.
3. `Enqueue()` caller's context being cancelled (only cancels via the entry's cancel, but the task might still execute later).

---

## 20. Lifecycle Events Integration

The Hub integrates with the `lifecycle` package for system-wide event broadcasting:

| Event | When Emitted |
|-------|-------------|
| `lifecycle.EventAgentConnected` | After agent is stored in map (with agent ID as data) |
| `lifecycle.EventAgentDisconnected` | After agent is removed from map, or when old agent is displaced by same-name reconnect |

These events are package-level (`lifecycle.Emit()`) and are separate from the lane events (which go through `LaneManager.onEvent` callback).

### Lane Events

| Event Type | When Emitted |
|-----------|-------------|
| `task_enqueued` | After task is appended to queue |
| `task_started` | At beginning of task execution goroutine |
| `task_completed` | After task function returns (success or error) |
| `task_cancelled` | When `CancelActive()` cancels a running task |

All lane events are emitted asynchronously via `go fn(event)` to avoid blocking the lane machinery.

---

## Appendix: Complete Function Signature Reference

### LaneManager Public API

```go
func NewLaneManager() *LaneManager
func (m *LaneManager) OnEvent(fn func(LaneEvent))
func (m *LaneManager) SetConcurrency(lane string, maxConcurrent int)
func (m *LaneManager) Enqueue(ctx context.Context, lane string, task func(ctx context.Context) error, opts ...EnqueueOption) error
func (m *LaneManager) EnqueueAsync(ctx context.Context, lane string, task func(ctx context.Context) error, opts ...EnqueueOption)
func (m *LaneManager) GetQueueSize(lane string) int
func (m *LaneManager) GetTotalQueueSize() int
func (m *LaneManager) ClearLane(lane string) int
func (m *LaneManager) CancelActive(lane string) int
func (m *LaneManager) GetLaneStats() map[string]LaneStats
func (m *LaneManager) Shutdown()

func WithWarnAfter(ms int64) EnqueueOption
func WithOnWait(fn func(waitMs int64, queuedAhead int)) EnqueueOption
func WithDescription(desc string) EnqueueOption

func WithLane(ctx context.Context, lane string) context.Context
func GetLane(ctx context.Context) string
```

### Hub Public API

```go
func NewHub() *Hub
func (h *Hub) Run(ctx context.Context)
func (h *Hub) HandleWebSocket(w http.ResponseWriter, r *http.Request, agentID string)
func (h *Hub) GetTheAgent() *AgentConnection
func (h *Hub) GetAgentByName(name string) *AgentConnection
func (h *Hub) GetAgent(agentID string) *AgentConnection
func (h *Hub) GetAnyAgent() *AgentConnection
func (h *Hub) GetAllAgents() []*AgentConnection
func (h *Hub) IsConnected() bool
func (h *Hub) IsAgentConnected(name string) bool
func (h *Hub) AgentCount() int
func (h *Hub) SendToAgent(agentID string, frame *Frame) error
func (h *Hub) SendToAgentByName(name string, frame *Frame) error
func (h *Hub) Send(frame *Frame) error
func (h *Hub) Broadcast(frame *Frame)
func (h *Hub) SendRequestSync(ctx context.Context, method string, params map[string]any) (*Frame, error)
func (h *Hub) SetResponseHandler(handler ResponseHandler)
func (h *Hub) SetApprovalHandler(handler ApprovalRequestHandler)
func (h *Hub) SetAskHandler(handler AskRequestHandler)
func (h *Hub) SetEventHandler(handler func(agentID string, frame *Frame))
func (h *Hub) SendApprovalResponse(agentID, requestID string, approved bool) error
func (h *Hub) SendApprovalResponseWithAlways(agentID, requestID string, approved, always bool) error
func (h *Hub) SendAskResponse(agentID, requestID, value string) error
```

### Handler Type Signatures

```go
type ResponseHandler func(agentID string, frame *Frame)
type ApprovalRequestHandler func(agentID string, requestID string, toolName string, input json.RawMessage)
type AskRequestHandler func(agentID string, requestID string, prompt string, widgets json.RawMessage)
```
