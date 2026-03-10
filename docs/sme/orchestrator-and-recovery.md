# Orchestrator & Recovery: Deep-Dive Logic Document

Source files:
- `nebo/internal/agent/orchestrator/orchestrator.go`
- `nebo/internal/agent/recovery/recovery.go`
- `nebo/internal/db/migrations/0022_pending_tasks.sql`
- Integration: `nebo/internal/agent/tools/bot_tool.go`, `nebo/internal/agent/tools/types.go`, `nebo/internal/agent/runner/runner.go`, `nebo/cmd/nebo/agent.go`

> **Rust implementation status (2026-03-10):**
> The Rust orchestrator is in `crates/agent/src/orchestrator.rs` and differs from Go in several ways:
> - Implements `tools::SubAgentOrchestrator` trait (spawn, execute_dag, cancel, status, list_active, recover).
> - Uses `tokio::sync::RwLock` + `CancellationToken` instead of `sync.RWMutex` + `context.CancelFunc`.
> - Concurrency is managed by `ConcurrencyController` (in `crates/agent/src/concurrency.rs`) with LLM permits, not a hard-coded `maxConcurrent: 5`.
> - **DAG execution**: Rust adds `execute_dag_internal` which decomposes tasks via `crates/agent/src/decompose.rs`, builds a `TaskGraph` (in `task_graph.rs`), and schedules sub-tasks reactively using `FuturesUnordered`. Go had no DAG support.
> - Agent types: Explore, Plan, General (same as Go) with `system_prompt_for_type()`.
> - Session keys use format `subagent:{parent_session_key}:{task_id}` (Go used `subagent-agent-{UnixNano}`).
> - Task IDs use `sa-{uuid}` format (Go used `agent-{UnixNano}`).
> - Recovery: `recover_internal()` matches Go's rules (2h age limit, retry exhaustion check, completion heuristic) but routes recovered tasks through `LaneManager` when available.
> - `check_completion_heuristic()` matches Go's 4-rule heuristic exactly.

---

## 1. Recovery Package (`internal/agent/recovery/`)

### 1.1 Constants

```go
// TaskStatus
StatusPending   TaskStatus = "pending"
StatusRunning   TaskStatus = "running"
StatusCompleted TaskStatus = "completed"
StatusFailed    TaskStatus = "failed"
StatusCancelled TaskStatus = "cancelled"

// TaskType
TaskTypeSubagent   TaskType = "subagent"
TaskTypeRun        TaskType = "run"
TaskTypeEventAgent TaskType = "event_agent"  // Scheduled/triggered tasks
```

### 1.2 PendingTask Struct

```go
type PendingTask struct {
    ID           string      // UUID, auto-generated if empty
    TaskType     TaskType    // "subagent", "run", "event_agent"
    Status       TaskStatus  // Current lifecycle state
    SessionKey   string      // Session key for this task's conversation
    UserID       string      // Owner user ID (nullable)
    Prompt       string      // The task/prompt text to execute
    SystemPrompt string      // Optional system prompt override (nullable)
    Description  string      // Human-readable description (nullable)
    Lane         string      // Which lane: main, events, subagent (default: "main")
    Priority     int         // Higher = more urgent (default: 0)
    Attempts     int         // Number of execution attempts so far
    MaxAttempts  int         // Max retries before permanent failure (default: 3)
    LastError    string      // Last error message if failed (nullable)
    CreatedAt    time.Time   // When the task was created
    StartedAt    *time.Time  // When execution began (nullable)
    CompletedAt  *time.Time  // When execution ended (nullable)
    ParentTaskID string      // FK to parent pending_tasks.id (nullable, CASCADE SET NULL)
}
```

### 1.3 Manager Struct

```go
type Manager struct {
    db *sql.DB
}

func NewManager(db *sql.DB) *Manager
```

Holds a raw `*sql.DB` reference. No connection pooling or prepared statements -- uses `ExecContext`/`QueryContext` directly.

### 1.4 CreateTask

```go
func (m *Manager) CreateTask(ctx context.Context, task *PendingTask) error
```

**Defaults applied before insert:**
- `task.ID` = `uuid.New().String()` if empty
- `task.Status` = `StatusPending` if empty
- `task.Lane` = `"main"` if empty
- `task.MaxAttempts` = `3` if zero

**SQL:**
```sql
INSERT INTO pending_tasks (
    id, task_type, status, session_key, user_id, prompt, system_prompt,
    description, lane, priority, max_attempts, created_at, parent_task_id
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
```

- `created_at` is stored as `time.Now().Unix()` (Unix timestamp integer).
- `user_id`, `system_prompt`, `description`, `parent_task_id` use `sql.NullString` (stored as NULL when empty).
- `attempts` is NOT set on insert -- defaults to 0 via the column default.
- The `task.ID` is mutated in-place on the passed-in struct pointer, so the caller can read it back.

### 1.5 MarkRunning

```go
func (m *Manager) MarkRunning(ctx context.Context, taskID string) error
```

**SQL:**
```sql
UPDATE pending_tasks
SET status = 'running', started_at = ?, attempts = attempts + 1
WHERE id = ?
```

- Atomically increments `attempts` in the DB.
- Sets `started_at` to `time.Now().Unix()`.

### 1.6 MarkCompleted

```go
func (m *Manager) MarkCompleted(ctx context.Context, taskID string) error
```

**SQL:**
```sql
UPDATE pending_tasks
SET status = 'completed', completed_at = ?
WHERE id = ?
```

- Sets `completed_at` to `time.Now().Unix()`.

### 1.7 MarkFailed (with auto-requeue)

```go
func (m *Manager) MarkFailed(ctx context.Context, taskID, errorMsg string) error
```

**SQL:**
```sql
UPDATE pending_tasks
SET status = CASE WHEN attempts >= max_attempts THEN 'failed' ELSE 'pending' END,
    last_error = ?,
    completed_at = CASE WHEN attempts >= max_attempts THEN ? ELSE NULL END
WHERE id = ?
```

**Critical behavior:** This is NOT a simple "mark as failed". It implements automatic retry:
- If `attempts >= max_attempts`: status becomes `'failed'`, `completed_at` is set.
- If `attempts < max_attempts`: status becomes `'pending'` (re-queued for retry), `completed_at` stays NULL.
- `last_error` is always updated.

This means calling `MarkFailed` after a crash on attempt 1 (with max_attempts=3) will re-queue the task as `pending`, making it eligible for recovery on next restart.

### 1.8 MarkCancelled

```go
func (m *Manager) MarkCancelled(ctx context.Context, taskID string) error
```

**SQL:**
```sql
UPDATE pending_tasks
SET status = 'cancelled', last_error = 'shutdown', completed_at = ?
WHERE id = ?
```

**Difference from MarkFailed:** This NEVER re-queues. It unconditionally sets `status = 'cancelled'` regardless of attempt count. Used during graceful shutdown and explicit user cancellation. The `last_error` is hardcoded to `'shutdown'`.

### 1.9 GetRecoverableTasks

```go
func (m *Manager) GetRecoverableTasks(ctx context.Context) ([]*PendingTask, error)
```

**SQL:**
```sql
SELECT id, task_type, status, session_key, user_id, prompt, system_prompt,
       description, lane, priority, attempts, max_attempts, last_error,
       created_at, started_at, completed_at, parent_task_id
FROM pending_tasks
WHERE status IN ('pending', 'running')
ORDER BY priority DESC, created_at ASC
```

**Key details:**
- Returns tasks in BOTH `pending` and `running` states. Tasks left in `running` state indicate a crash mid-execution.
- Ordered by priority (highest first), then creation time (oldest first).
- Nullable columns (`user_id`, `system_prompt`, `description`, `last_error`, `parent_task_id`, `started_at`, `completed_at`) are scanned via `sql.NullString` / `sql.NullInt64`.
- `created_at` is converted from Unix timestamp: `time.Unix(createdAt, 0)`.
- `started_at` and `completed_at` are `*time.Time` pointers (nil if NULL).

### 1.10 CheckTaskCompletion

```go
func (m *Manager) CheckTaskCompletion(ctx context.Context, task *PendingTask) (bool, error)
```

Examines the task's session in the `chat_messages` table to determine if the task was actually completed despite its DB status saying otherwise (crash recovery scenario).

**Step 1: Count messages**
```sql
SELECT
    COUNT(*) as total,
    SUM(CASE WHEN role = 'assistant' THEN 1 ELSE 0 END) as assistant_count,
    SUM(CASE WHEN tool_calls IS NOT NULL AND tool_calls != '' AND tool_calls != 'null' THEN 1 ELSE 0 END) as tool_count
FROM chat_messages
WHERE chat_id = ?
```
Uses `task.SessionKey` as the `chat_id`.

**Completion rules (evaluated in order):**

1. **No messages at all** (`totalCount == 0` or `sql.ErrNoRows`): Task never started. Returns `false`.
2. **Has tool calls** (`toolCallCount > 0`): The agent was actively working. Returns `true`. Rationale: "better to lose partial work than re-run and duplicate side effects."
3. **Has assistant responses AND multiple messages** (`assistantCount > 0 && totalCount > 2`): The agent went through at least one loop iteration. Returns `true`.
4. **Last message check** (fallback):
```sql
SELECT role, content
FROM chat_messages
WHERE chat_id = ?
ORDER BY created_at DESC
LIMIT 1
```
If the last message is from `assistant` with content length > 50 characters, returns `true`.

5. **Otherwise**: Returns `false` (task is genuinely incomplete).

**Design philosophy:** Deliberately generous -- it is better to skip a completed task than re-run it (avoid duplicate side effects like file writes, API calls, etc.).

### 1.11 RecoverTasks

```go
func (m *Manager) RecoverTasks(ctx context.Context) ([]*PendingTask, error)
```

Higher-level recovery function (used by the recovery manager itself, not directly by the orchestrator).

**Algorithm:**
1. Call `GetRecoverableTasks()` to get all `pending`/`running` tasks.
2. For each task:
   a. Call `CheckTaskCompletion()`. On error, log warning and **skip** (do not recover).
   b. If completed: call `MarkCompleted()` and skip.
   c. If NOT completed and status is `running`: reset to `pending` via:
      ```sql
      UPDATE pending_tasks SET status = 'pending' WHERE id = ?
      ```
   d. Add to recovery list.
3. Return the filtered list of genuinely incomplete tasks.

**Note:** The orchestrator's `RecoverAgents()` method performs its OWN recovery logic (see section 2.12) and calls `GetRecoverableTasks()` directly, not `RecoverTasks()`. So `RecoverTasks()` appears to be an unused or alternative code path -- the orchestrator implements additional rules (max age, retry exhaustion) that `RecoverTasks()` does not.

### 1.12 CleanupOldTasks

```go
func (m *Manager) CleanupOldTasks(ctx context.Context) (int64, error)
```

**SQL:**
```sql
DELETE FROM pending_tasks
WHERE status IN ('completed', 'failed', 'cancelled')
  AND completed_at < ?
```

- TTL: **7 days** (`time.Now().Add(-7 * 24 * time.Hour).Unix()`).
- Only deletes terminal-state tasks. `pending` and `running` tasks are never cleaned up by this.
- Returns the number of rows deleted.

### 1.13 Helper: nullString

```go
func nullString(s string) sql.NullString
```
Returns `sql.NullString{String: s, Valid: true}` if `s != ""`, otherwise `sql.NullString{}` (NULL).

---

## 2. Orchestrator Package (`internal/agent/orchestrator/`)

### 2.1 Constants

**AgentStatus (mirrors recovery.TaskStatus but is a separate type):**
```go
StatusPending   AgentStatus = "pending"
StatusRunning   AgentStatus = "running"
StatusCompleted AgentStatus = "completed"
StatusFailed    AgentStatus = "failed"
StatusCancelled AgentStatus = "cancelled"
```

**Lane constants:**
```go
LaneMain      = "main"       // User conversations (serialized)
LaneEvents    = "events"     // Scheduled/triggered tasks
LaneSubagent  = "subagent"   // Sub-agent goroutines
LaneNested    = "nested"     // Tool recursion/callbacks
LaneHeartbeat = "heartbeat"  // Proactive heartbeat ticks
LaneComm      = "comm"       // Inter-agent communication messages
```

### 2.2 Interfaces

```go
type ToolExecutor interface {
    Execute(ctx context.Context, call *ai.ToolCall) *ToolExecResult
    List() []ai.ToolDefinition
}
```

Implemented by `registryAdapter` (in `tools/types.go`), which wraps the tool `Registry`:
```go
type registryAdapter struct {
    registry *Registry
}
func (a *registryAdapter) Execute(ctx context.Context, call *ai.ToolCall) *orchestrator.ToolExecResult
func (a *registryAdapter) List() []ai.ToolDefinition
```

### 2.3 ToolExecResult

```go
type ToolExecResult struct {
    Content string
    IsError bool
}
```

### 2.4 SubAgent Struct

```go
type SubAgent struct {
    ID               string          // Format: "agent-{UnixNano}" or "agent-recovered-{taskID[:8]}"
    TaskID           string          // recovery.PendingTask.ID for persistence
    Task             string          // Full task/prompt text
    Description      string          // Short human-readable description
    Lane             string          // Lane this agent runs in (default: LaneSubagent)
    ModelOverride    string          // Override model for this sub-agent
    Status           AgentStatus     // Current lifecycle state
    Result           string          // Final text result from the agentic loop
    Error            error           // Error if failed
    StartedAt        time.Time       // When spawned
    CompletedAt      time.Time       // When finished (zero value if still running)
    Events           []ai.StreamEvent // All streaming events collected during execution
    cancel           context.CancelFunc // Context cancellation function (unexported)
    parentSessionID  string          // Parent session DB ID for result delivery (unexported)
    waited           bool            // Whether parent is blocking on this agent (unexported)
}
```

### 2.5 SpawnRequest Struct

```go
type SpawnRequest struct {
    ParentSessionKey string        // Parent session for context inheritance (currently unused in Spawn)
    ParentSessionID  string        // Parent session DB ID -- used to deliver results for wait=false agents
    Task             string        // Task description for the sub-agent
    Description      string        // Short description for tracking
    Lane             string        // Lane to run in (default: LaneSubagent)
    Wait             bool          // Wait for completion before returning
    Timeout          time.Duration // Context timeout for the agent
    SystemPrompt     string        // Optional custom system prompt
    ModelOverride    string        // Override model (e.g., "anthropic/claude-haiku-4-5")
}
```

### 2.6 AgentResult Struct

```go
type AgentResult struct {
    AgentID string
    Success bool
    Result  string
    Error   error
}
```

Sent through the `results` channel when a sub-agent completes.

### 2.7 Orchestrator Struct

```go
type Orchestrator struct {
    mu        sync.RWMutex
    agents    map[string]*SubAgent    // All agents, keyed by agent ID
    sessions  *session.Manager        // Session manager for creating sub-agent sessions
    providers []ai.Provider           // AI providers (first is used)
    tools     ToolExecutor            // Tool registry adapter
    config    *config.Config          // Agent config (MaxIterations, MaxContext)
    recovery  *recovery.Manager       // Optional: for persisting subagent runs

    // Limits
    maxConcurrent int   // Default: 5. Max running sub-agents at any time.
    maxPerParent  int   // Default: 0 (unlimited). Max agents per parent session. (NOT ENFORCED in current code)

    // Channels for coordination
    results chan AgentResult  // Buffered channel (capacity 100) for completion notifications
}
```

### 2.8 NewOrchestrator

```go
func NewOrchestrator(cfg *config.Config, sessions *session.Manager, providers []ai.Provider, toolExecutor ToolExecutor) *Orchestrator
```

**Defaults:**
- `maxConcurrent`: 5
- `maxPerParent`: 0 (unlimited)
- `results` channel: buffered with capacity 100
- `agents` map: initialized empty
- `recovery`: nil (must be set via `SetRecoveryManager`)

### 2.9 Spawn -- The Core Spawning Algorithm

```go
func (o *Orchestrator) Spawn(ctx context.Context, req *SpawnRequest) (*SubAgent, error)
```

This is a three-phase spawn with double-checked locking:

**Phase 1: First concurrency check (under lock)**
```go
o.mu.Lock()
// Count agents with StatusRunning
// If runningCount >= maxConcurrent: unlock and return error
o.mu.Unlock()
```

**Phase 2: Generate IDs and persist (NO lock held)**

This is intentionally done outside the lock because DB operations can be slow.

1. Generate agent ID: `fmt.Sprintf("agent-%d", time.Now().UnixNano())`
2. Create context:
   - If `req.Timeout > 0`: `context.WithTimeout(ctx, req.Timeout)`
   - Otherwise: `context.WithCancel(ctx)`
   - **Note:** When timeout is set, `context.WithTimeout` replaces `context.WithCancel` -- the `cancel` from `WithCancel` is leaked (minor bug). Both cases produce a valid cancel function.
3. Determine lane: `req.Lane` or default `LaneSubagent`.
4. Generate session key: `fmt.Sprintf("subagent-%s", agentID)` -- e.g., `"subagent-agent-1709571234567890123"`.
5. Create the `SubAgent` struct with `StatusPending`.
6. If recovery manager is set, persist via `recovery.CreateTask()`:
   ```go
   task := &recovery.PendingTask{
       TaskType:     recovery.TaskTypeSubagent,
       Status:       recovery.StatusPending,
       SessionKey:   sessionKey,
       Prompt:       req.Task,
       SystemPrompt: req.SystemPrompt,
       Description:  req.Description,
       Lane:         lane,
   }
   ```
   On failure: cancel context and return error.
   On success: capture `task.ID` (which was auto-generated by `CreateTask`).

**Phase 3: Register under lock (double-check)**
```go
o.mu.Lock()
// Re-count running agents (another Spawn may have raced us during Phase 2)
// If runningCount >= maxConcurrent: unlock, cancel context, best-effort MarkFailed, return error
agent.TaskID = taskID
o.agents[agentID] = agent
o.mu.Unlock()
```

**Launch goroutine:**
```go
go o.runAgent(agentCtx, agent, req, sessionKey)
```

**Wait behavior:**
- If `req.Wait == true`: calls `o.waitForAgent(context.Background(), agentID)`.
  - **Critical:** Uses `context.Background()`, NOT the parent context. This decouples the wait from parent cancellation. The wait continues even if the parent context is cancelled (e.g., user closes UI, switches conversations).
- If `req.Wait == false`: returns immediately with the agent in `StatusPending` or `StatusRunning`.

### 2.10 runAgent -- The Goroutine Execution Function

```go
func (o *Orchestrator) runAgent(ctx context.Context, agent *SubAgent, req *SpawnRequest, sessionKey string)
```

**Panic recovery (outermost defer):**
```go
defer func() {
    if r := recover(); r != nil {
        // Log panic
        // Set agent.Status = StatusFailed, agent.Error = fmt.Errorf("panic: %v", r)
        // Mark failed in recovery DB (using context.Background())
        // Send result to results channel (so waiters don't hang forever)
    }
}()
```

**Status transition:** `StatusPending` -> `StatusRunning` (under lock).

**Mark running in recovery DB:**
```go
if o.recovery != nil && agent.TaskID != "" {
    o.recovery.MarkRunning(ctx, agent.TaskID)
}
```
This increments `attempts` in the DB.

**Completion defer (second defer, runs before panic recovery):**
1. Clean up browser windows owned by this session key:
   ```go
   webview.GetManager().CloseWindowsByOwner(sessionKey)
   ```
2. Set `CompletedAt` and final status:
   - If still `StatusRunning` and `Error != nil`: -> `StatusFailed`
   - If still `StatusRunning` and `Error == nil`: -> `StatusCompleted`
   - If already `StatusCancelled` (set by `CancelAgent`): stays `StatusCancelled`
3. Update recovery DB (using `context.Background()`):
   - `StatusCompleted` -> `MarkCompleted()`
   - `StatusFailed` -> `MarkFailed()` with error message
   - `StatusCancelled` -> `MarkFailed()` with `"cancelled"` message
4. Send `AgentResult` to `results` channel.
5. For non-waited agents (`!agent.waited && agent.parentSessionID != ""`): call `deliverResultToParent()`.

**Session creation:**
```go
sess, err := o.sessions.GetOrCreate(sessionKey, "")
```
- `sessionKey` = `"subagent-agent-{UnixNano}"`
- `userID` = `""` (empty) -- sub-agent sessions are task-scoped, not user-scoped.

**System prompt selection:**
- If `req.SystemPrompt` is set: use it directly.
- Otherwise: call `buildSubAgentPrompt(req.Task)` which produces:
```
You are a focused sub-agent working on a specific task.

Your task: {task}

Guidelines:
1. Focus ONLY on the assigned task
2. Work efficiently and complete the task as quickly as possible
3. Use tools as needed to accomplish the task
4. When the task is complete, provide a clear summary of what was done
5. Do not ask for clarification - make reasonable assumptions
6. Do not engage in conversation - just complete the task
7. When writing code, edit existing files instead of creating new ones. Never leave dead code behind.
8. Never create summary documents, report files, or documentation unless the task specifically requires it

When you have completed the task, provide your final response summarizing what was accomplished.
```

**Message injection:**
Appends the task as a user message to the new session:
```go
o.sessions.AppendMessage(sess.ID, session.Message{
    SessionID: sess.ID,
    Role:      "user",
    Content:   req.Task,
})
```

**Execute the loop:**
```go
result, err := o.executeLoop(ctx, sess.ID, systemPrompt, agent.ModelOverride, agent)
```

### 2.11 executeLoop -- The Agentic Loop

```go
func (o *Orchestrator) executeLoop(ctx context.Context, sessionID, systemPrompt, modelOverride string, agent *SubAgent) (string, error)
```

**Max iterations:** `config.MaxIterations` or 50 if not set / <= 0.

**Loop structure:**
```
for iteration := 0; iteration < maxIterations; iteration++ {
    1. Check context cancellation -> set StatusCancelled, return
    2. Get session messages (with MaxContext limit)
    3. Stream from provider[0] (always first provider)
    4. Process events:
       - EventTypeText: append to assistantContent + finalResult
       - EventTypeToolCall: collect tool calls
       - EventTypeError: return with error
    5. Save assistant message (content + tool_calls JSON)
    6. If has tool calls:
       - Execute each tool call via o.tools.Execute()
       - Save tool results as "tool" role message
       - continue (next iteration)
    7. If no tool calls: break (task complete)
}
return finalResult, nil
```

**Key details:**
- Only uses `providers[0]` -- no fallback to other providers.
- All stream events are stored in `agent.Events` under lock for tracking.
- Tool calls and results are serialized as JSON via `json.Marshal`.
- Tool results use `session.ToolResult` struct with `ToolCallID`, `Content`, `IsError`.
- The `finalResult` is a `strings.Builder` that accumulates ALL text across all iterations.
- `modelOverride` is passed through to `provider.Stream()` -- if non-empty, it overrides the provider's default model.

### 2.12 RecoverAgents -- Restart Recovery

```go
func (o *Orchestrator) RecoverAgents(ctx context.Context) (int, error)
```

**Recovery age limit:** `const maxRecoveryAge = 2 * time.Hour`

**Algorithm:**
1. Call `o.recovery.GetRecoverableTasks(ctx)` -- gets all `pending`/`running` tasks.
2. For each task:
   a. Skip if `task.TaskType != TaskTypeSubagent`.
   b. **Rule 1 (Age check):** If `time.Since(task.CreatedAt) > 2h`, mark failed with `"stale: exceeded max recovery age"` and skip.
   c. **Rule 2 (Retry exhaustion):** If `task.Attempts >= task.MaxAttempts`, mark failed with `"exhausted retry attempts"` and skip.
   d. **Rule 3 (Completion check):** Call `o.recovery.CheckTaskCompletion(ctx, task)`.
      - On error: mark failed with error message and skip (err on side of NOT re-running).
      - If completed: `MarkCompleted()` and skip.
   e. **Re-spawn:** Task is genuinely incomplete and recent.

**Re-spawn details:**
```go
agentID := fmt.Sprintf("agent-recovered-%s", task.ID[:8])
agentCtx, cancel := context.WithCancel(ctx)
lane := task.Lane or LaneSubagent
```

The recovered agent reuses the original `task.ID` (as `TaskID`) and `task.SessionKey`. This means the re-spawned agent will pick up the same session conversation that was interrupted.

```go
agent := &SubAgent{
    ID:          agentID,
    TaskID:      task.ID,   // Original PendingTask ID
    Task:        task.Prompt,
    Description: task.Description,
    Lane:        lane,
    Status:      StatusPending,
    StartedAt:   time.Now(),
    cancel:      cancel,
}

o.agents[agentID] = agent
go o.runAgent(agentCtx, agent, req, task.SessionKey)
```

Important: recovered agents are always `Wait: false` (fire-and-forget). The `parentSessionID` is not preserved during recovery.

### 2.13 waitForAgent

```go
func (o *Orchestrator) waitForAgent(ctx context.Context, agentID string) (*SubAgent, error)
```

Blocking wait that polls two sources:

```
for {
    select {
    case <-ctx.Done():
        return nil, ctx.Err()

    case result := <-o.results:
        if result.AgentID == agentID:
            return agent, result.Error
        else:
            // Put back results for other agents (spawns a goroutine to avoid blocking)
            go func(r AgentResult) { o.results <- r }(result)

    case <-time.After(100ms):
        // Polling fallback: check agent status directly
        if agent.Status in {Completed, Failed, Cancelled}:
            return agent, agent.Error
    }
}
```

**Subtleties:**
- The `results` channel is shared across ALL agents. When waiting for a specific agent, results for other agents are re-enqueued via a goroutine (to avoid blocking the current goroutine on a full channel).
- The 100ms polling fallback handles the race where the result was sent before `waitForAgent` started listening.
- Uses `context.Background()` from the caller (see Spawn), so this wait is decoupled from parent context cancellation.

### 2.14 CancelAgent

```go
func (o *Orchestrator) CancelAgent(agentID string) error
```

1. Lock, find agent, verify status is `Running` or `Pending`.
2. Set `agent.Status = StatusCancelled`, `agent.CompletedAt = time.Now()`.
3. Capture `taskID` and `cancelFn` from agent.
4. Unlock.
5. If recovery manager set, call `recovery.MarkCancelled()` -- unconditional cancellation, no re-queue.
6. Call `cancelFn()` -- cancels the context, which will cause the goroutine's `executeLoop` to see `ctx.Done()` on next iteration.

### 2.15 deliverResultToParent

```go
func (o *Orchestrator) deliverResultToParent(agent *SubAgent)
```

For non-waited (`wait=false`) agents, appends a summary message to the parent session so the parent model sees it.

**Message format (appended as `role: "user"`):**
```
[Sub-agent completed: {description}]
Agent ID: {id}
Status: {status}
Duration: {duration rounded to seconds}
Error: {error if any}

Result:
{result text, truncated to 4000 chars if longer}
```

- Result truncation: if `len(result) > 4000`, truncates and appends `"\n... (truncated)"`.
- Uses `o.sessions.AppendMessage()` with the `agent.parentSessionID` as the session ID.

### 2.16 Shutdown

```go
func (o *Orchestrator) Shutdown(ctx context.Context)
```

Graceful shutdown of all running/pending agents:
1. Lock, collect all agents with `StatusRunning` or `StatusPending`, unlock.
2. For each: set `StatusCancelled`, `CompletedAt = time.Now()`.
3. Mark cancelled in recovery DB (so they won't be recovered on next restart).
4. Call `cancelFn()` to stop the goroutine.

### 2.17 Cleanup

```go
func (o *Orchestrator) Cleanup(maxAge time.Duration) int
```

Removes completed agents from the in-memory `agents` map (NOT from the database):
- Only removes agents not in `StatusRunning` or `StatusPending`.
- Only removes if `agent.CompletedAt` is older than `maxAge`.
- Returns number removed.

### 2.18 GetAgent, ListAgents, RunningCount

```go
func (o *Orchestrator) GetAgent(agentID string) (*SubAgent, bool)   // By ID
func (o *Orchestrator) ListAgents() []*SubAgent                      // All agents
func (o *Orchestrator) RunningCount() int                            // Count of StatusRunning
func (o *Orchestrator) Results() <-chan AgentResult                  // Read-only channel
```

All use `RLock` for read access.

### 2.19 GetMaxConcurrent / SetMaxConcurrent

```go
func (o *Orchestrator) GetMaxConcurrent() int
func (o *Orchestrator) SetMaxConcurrent(max int)  // Clamps to minimum of 1
```

---

## 3. Database Schema (`pending_tasks`)

```sql
CREATE TABLE IF NOT EXISTS pending_tasks (
    id TEXT PRIMARY KEY,
    task_type TEXT NOT NULL,                          -- 'subagent', 'run', 'cron_agent'
    status TEXT NOT NULL DEFAULT 'pending',           -- 'pending', 'running', 'completed', 'failed', 'cancelled'
    session_key TEXT NOT NULL,                        -- Session for this task
    user_id TEXT,                                     -- Owner user (nullable)
    prompt TEXT NOT NULL,                             -- Task/prompt to execute
    system_prompt TEXT,                               -- System prompt override (nullable)
    description TEXT,                                 -- Human-readable (nullable)
    lane TEXT DEFAULT 'main',                         -- Lane: main, cron, subagent
    priority INTEGER DEFAULT 0,                       -- Higher = more urgent
    attempts INTEGER DEFAULT 0,                       -- Execution attempts
    max_attempts INTEGER DEFAULT 3,                   -- Max retries
    last_error TEXT,                                  -- Last error message (nullable)
    created_at INTEGER NOT NULL,                      -- Unix timestamp
    started_at INTEGER,                               -- Unix timestamp (nullable)
    completed_at INTEGER,                             -- Unix timestamp (nullable)
    parent_task_id TEXT REFERENCES pending_tasks(id) ON DELETE SET NULL
);

-- Indexes
CREATE INDEX idx_pending_tasks_status ON pending_tasks(status);
CREATE INDEX idx_pending_tasks_lane ON pending_tasks(lane, status);
CREATE INDEX idx_pending_tasks_user ON pending_tasks(user_id, status);
CREATE INDEX idx_pending_tasks_parent ON pending_tasks(parent_task_id);
```

**Notes for Rust implementation:**
- All timestamps are Unix epoch integers, not ISO strings.
- The `parent_task_id` FK has `ON DELETE SET NULL` -- if a parent task is deleted, children become orphans.
- The `status` column has a default of `'pending'` but the Go code always sets it explicitly.
- The migration comment says `'cron_agent'` for task_type but the Go code uses `"event_agent"` -- this is a rename that happened after the migration was written.

---

## 4. Integration: How Everything Wires Together

### 4.1 Startup Sequence (cmd/nebo/agent.go)

```
1. Create SQLite DB connection
2. session.New(sqlDB)
3. recovery.NewManager(sqlDB) -> state.recovery
4. createProviders(cfg) -> providers
5. ... (policy, tool registry, runner setup) ...
6. runner.SetupSubagentPersistence(state.recovery)
   -> botTool.SetRecoveryManager(mgr)
     -> orchestrator.SetRecoveryManager(mgr)
7. runner.RecoverSubagents(ctx)
   -> botTool.RecoverSubagents(ctx)
     -> orchestrator.RecoverAgents(ctx)
```

### 4.2 Tool Invocation Path (how the LLM spawns a sub-agent)

```
LLM calls: bot(resource: "task", action: "spawn", prompt: "...", description: "...")
  -> Registry.Execute()
    -> BotTool.Execute()
      -> BotTool.handleTask()
        -> BotTool.taskSpawn()
          -> provider.GetModelsConfig() to get lane-specific model override
          -> orchestrator.Spawn(ctx, &SpawnRequest{
               ParentSessionID: GetSessionID(ctx),  // from context
               Task, Description, Wait, Timeout, SystemPrompt, ModelOverride
             })
```

### 4.3 BotTool Input Fields for Task Spawning

```go
type BotInput struct {
    Resource    string  // "task"
    Action      string  // "spawn", "status", "cancel", "list", "create", "update", "delete"
    Description string  // Short description
    Prompt      string  // Full task text
    Wait        *bool   // Default: true (wait for completion)
    Timeout     int     // Seconds, default: 300 (5 minutes)
    AgentType   string  // "explore", "plan", "general" -- affects system prompt
    AgentID     string  // For status/cancel operations
}
```

### 4.4 Agent Type System Prompts (tools/types.go)

The `buildAgentSystemPrompt` function in `types.go` creates specialized system prompts:

- **"explore"**: Read-only exploration agent. "Do NOT modify any files - only read and analyze."
- **"plan"**: Planning agent. "Do NOT implement the plan - only create it."
- **default/general**: No restrictions, just "Complete the task efficiently."

This is separate from `buildSubAgentPrompt` in `orchestrator.go` which is only used when `req.SystemPrompt` is empty AND the BotTool didn't set one.

### 4.5 Model Override for Sub-agents

The BotTool checks `provider.GetModelsConfig().LaneRouting.Subagent` for a lane-specific model override. This allows configuring a cheaper/faster model for sub-agents (e.g., `claude-haiku-4-5` instead of `claude-sonnet-4-20250514`).

### 4.6 registryAdapter

Bridges the `tools.Registry` (which returns `tools.ToolResult`) to the orchestrator's `ToolExecutor` interface (which returns `orchestrator.ToolExecResult`). This avoids a circular import between the `orchestrator` and `tools` packages.

---

## 5. Task State Machine

```
                    +-----------+
                    |           |
          +------->| pending   |<--------+
          |        |           |         |
          |        +-----+-----+         |
          |              |               |
          |              | Spawn/Recover |
          |              v               |
          |        +-----------+         |
   MarkFailed      |           |   MarkFailed
  (attempts <      | running   |  (attempts <
   max_attempts)   |           |   max_attempts)
          |        +-----+-----+         |
          |              |               |
          |    +---------+---------+     |
          |    |         |         |     |
          |    v         v         v     |
       +------+  +--------+  +--------+ |
       |cancel|  |complete |  | error  |-+
       |      |  |         |  |        |
       +--+---+  +----+----+  +---+----+
          |            |           |
          v            v           v
    +-----------+ +-----------+ +-----------+
    | cancelled | | completed | |  failed   |
    +-----------+ +-----------+ +-----------+
                                (if attempts
                                >= max_attempts)
```

**Transitions:**
- `pending` -> `running`: `MarkRunning()` (increments attempts)
- `running` -> `completed`: `MarkCompleted()`
- `running` -> `failed`: `MarkFailed()` when `attempts >= max_attempts`
- `running` -> `pending`: `MarkFailed()` when `attempts < max_attempts` (auto-requeue)
- `running` -> `cancelled`: `MarkCancelled()` (never re-queues)
- `pending` -> `cancelled`: `MarkCancelled()` (shutdown)
- `pending` -> `failed`: `MarkFailed()` in orchestrator (stale, exhausted retries, or spawn limit race)

---

## 6. Concurrency Model Summary

| Aspect | Value | Details |
|--------|-------|---------|
| Max concurrent sub-agents | 5 (default) | Configurable via `SetMaxConcurrent()`, minimum 1 |
| Max per parent | 0 (unlimited) | Field exists but enforcement code is absent |
| Results channel buffer | 100 | Shared across all agents |
| Goroutine model | One goroutine per sub-agent | `go o.runAgent(...)` |
| Context propagation | Child of caller's context | `WithCancel` or `WithTimeout` based on `req.Timeout` |
| Wait decoupling | `context.Background()` for waiters | Parent cancellation does not affect the wait |
| Locking | `sync.RWMutex` on Orchestrator | Status updates under write lock, reads under read lock |
| Panic safety | `recover()` in deferred function | Panics are caught, logged, and reported as failures |
| Browser cleanup | Per session key | `webview.GetManager().CloseWindowsByOwner(sessionKey)` on completion |

---

## 7. Recovery Timing & TTLs

| Parameter | Value | Location |
|-----------|-------|----------|
| Max recovery age | 2 hours | `orchestrator.RecoverAgents()` |
| Max retry attempts | 3 (default) | `recovery.CreateTask()` default |
| Old task cleanup TTL | 7 days | `recovery.CleanupOldTasks()` |
| Completion check: tool calls | Any tool call = complete | `CheckTaskCompletion()` |
| Completion check: message threshold | assistant > 0 AND total > 2 | `CheckTaskCompletion()` |
| Completion check: content length | Last assistant message > 50 chars | `CheckTaskCompletion()` |

---

## 8. Key Behaviors for Rust Reimplementation

### 8.1 Double-Checked Locking in Spawn

The Go code performs concurrency limit checks twice: once before DB persistence and once after. This is because the DB write (Phase 2) is done without the lock to avoid blocking other operations. A second Spawn call could slip in during that window. The Rust version should replicate this pattern if SQLite writes are similarly slow, or consider using an async mutex if the DB operations are fast.

### 8.2 Context.Background() for Wait

The `waitForAgent` call uses `context.Background()`, not the parent context. This is deliberate: if a user sends a message that spawns a sub-agent with `wait=true`, then the user switches conversations (cancelling the parent context), the wait should continue and the sub-agent should finish. The Rust equivalent would be using `CancellationToken::NONE` or a separate token.

### 8.3 MarkFailed Auto-Requeue

The most subtle behavior: `MarkFailed()` conditionally re-queues tasks. A single SQL statement handles both the "retry" and "permanent failure" cases via a CASE expression. The Rust version must replicate this atomic behavior.

### 8.4 Session Key Scoping

Sub-agent sessions are keyed as `"subagent-agent-{UnixNano}"` with empty `userID`. This means:
- Sub-agent sessions are isolated from user sessions.
- Each sub-agent gets its own conversation history.
- On recovery, the same session key is reused, so the agent can see its previous messages.

### 8.5 Result Delivery for Fire-and-Forget Agents

When `wait=false`, the completed sub-agent appends its result summary to the parent session as a "user" role message. This means the parent model will see the result on its next iteration. Result text is truncated to 4000 characters to avoid context bloat.

### 8.6 Shutdown vs Cancel

- `Shutdown()`: Marks ALL running/pending agents as cancelled in both memory and DB. Used on graceful shutdown.
- `CancelAgent()`: Marks a SINGLE agent as cancelled. Used on explicit user cancellation.
- Both use `MarkCancelled()` which never re-queues (unlike `MarkFailed()`).

### 8.7 Recovery Session Reuse

When recovering a task, the orchestrator reuses `task.SessionKey` (from the DB), not a new key. This means the `runAgent` goroutine will call `sessions.GetOrCreate(task.SessionKey, "")` which will find the existing session with its messages from the interrupted run. The agentic loop will then continue from where the previous run left off (since it reads all messages from the session).

However, `runAgent` also appends a new user message with the task prompt. This means the recovered session will have the task prompt duplicated (once from the original run, once from recovery). The `CheckTaskCompletion` heuristic handles this by detecting if the agent already produced meaningful output.

### 8.8 Provider Selection

The orchestrator only uses `providers[0]` -- no fallback. This is simpler than the runner's multi-provider fallback. The Rust version could keep this simplicity or add fallback.

### 8.9 Events Storage

All `ai.StreamEvent` values are stored in `agent.Events` (in-memory only, not persisted). This is used for tracking/monitoring but is lost on restart.

### 8.10 The Two System Prompt Paths

There are two separate system prompt builders:

1. **`orchestrator.buildSubAgentPrompt(task)`** in `orchestrator.go` -- used when `req.SystemPrompt == ""`. This is the generic sub-agent prompt.

2. **`buildAgentSystemPrompt(agentType, task)`** in `tools/types.go` -- used by `BotTool.taskSpawn()` to create a specialized prompt based on `agent_type` (explore, plan, general). This is passed as `req.SystemPrompt`, so it takes precedence over the orchestrator's default.

In practice, the BotTool always calls `buildAgentSystemPrompt()` and passes the result as `SystemPrompt`, so the orchestrator's `buildSubAgentPrompt()` is only a fallback for direct `Spawn()` calls that don't set `SystemPrompt`.

*Last updated: 2026-03-10*
