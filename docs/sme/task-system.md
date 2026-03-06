# Task System -- Go to Rust Migration Reference

**Source:** `internal/agent/tools/bot_tool.go`, `internal/agent/orchestrator/`, `internal/agent/recovery/`, `internal/agent/steering/generators.go`
**Target:** `crates/tools/src/bot_tool.rs`, `crates/agent/src/runner.rs`
**Status:** Partial -- basic cron/scheduler in Rust, objective detection ported, DB queries for active_task/work_tasks ported, no in-memory work tasks, no orchestrator crash recovery, no state checkpoint, no steering integration for work tasks

---

## Table of Contents

1. [Three-Tier Architecture Overview](#1-three-tier-architecture-overview)
2. [Tier 1 -- Active Objective](#2-tier-1----active-objective)
3. [Compaction Survival](#3-compaction-survival)
4. [Tier 2 -- Work Tasks](#4-tier-2----work-tasks)
5. [Tier 3 -- Sub-agent Tasks](#5-tier-3----sub-agent-tasks)
6. [Crash Recovery](#6-crash-recovery)
7. [Steering Integration](#7-steering-integration)
8. [Runner Integration](#8-runner-integration)
9. [Rust Implementation Status](#9-rust-implementation-status)

---

## 1. Three-Tier Architecture Overview

**File(s):** `internal/agent/tools/bot_tool.go`, `internal/agent/orchestrator/orchestrator.go`, `internal/agent/runner/runner.go`

The Nebo task system uses three tiers of task tracking, each with different storage, lifecycle, and visibility characteristics. Understanding the tier boundaries is CRITICAL for reimplementation because the tiers interact at specific integration points.

### 1.1 Tier Comparison Table

| Property | Tier 1: Active Objective | Tier 2: Work Tasks | Tier 3: Sub-agent Tasks |
|---|---|---|---|
| Storage | SQLite column `sessions.active_task` | `sync.Map` primary + `sessions.work_tasks` JSON backup | `pending_tasks` table + in-memory `agents` map |
| Persistence | Survives restart | In-memory lost, DB backup survives | Full DB persistence |
| Lifespan | Session-scoped | Session-scoped | Independent (outlives parent) |
| Cardinality | Exactly 0 or 1 per session | 0..N per session | 0..N global, max 5 concurrent |
| Created by | LLM classification (automatic) | Agent via `bot(resource: task, action: create)` | Agent via `bot(resource: task, action: spawn)` |
| Survives restart | Y (DB column) | Partially (DB backup, counter reset) | Y (pending_tasks + recovery) |
| Parent/child | None | Belongs to objective | Has parent_task_id FK |

### 1.2 Tier Interaction Summary

- **Objective detection** (Tier 1) runs async in background on every user message >= 20 chars
- When objective is **set**, all work tasks (Tier 2) are cleared -- fresh task list for new goal
- When objective is **updated**, work tasks are kept -- refinement, not replacement
- When objective is **cleared**, work tasks are also cleared
- Sub-agents (Tier 3) are independent -- they do NOT clear when the objective changes
- Steering generators read ALL three tiers to produce per-iteration guidance

---

## 2. Tier 1 -- Active Objective

**File(s):** `internal/agent/runner/runner.go`, `internal/db/queries/sessions.sql`, `internal/db/session_manager.go`

### 2.1 Database Schema

The active objective is stored as a nullable TEXT column on the `sessions` table:

```sql
-- Column added by migration
ALTER TABLE sessions ADD COLUMN active_task TEXT;
```

### 2.2 SQL Queries

```sql
-- name: GetSessionActiveTask :one
SELECT COALESCE(active_task, '') as active_task
FROM sessions WHERE id = ?;

-- name: SetSessionActiveTask :exec
UPDATE sessions
SET active_task = ?, updated_at = unixepoch()
WHERE id = ?;

-- name: ClearSessionActiveTask :exec
UPDATE sessions
SET active_task = NULL, updated_at = unixepoch()
WHERE id = ?;
```

### 2.3 Session Manager Methods

```go
func (m *SessionManager) GetActiveTask(sessionID string) (string, error) {
    ctx := context.Background()
    task, err := m.queries.GetSessionActiveTask(ctx, sessionID)
    if err != nil {
        return "", err
    }
    return task, nil
}

func (m *SessionManager) SetActiveTask(sessionID, task string) error {
    ctx := context.Background()
    return m.queries.SetSessionActiveTask(ctx, SetSessionActiveTaskParams{
        ActiveTask: sql.NullString{String: task, Valid: task != ""},
        ID:         sessionID,
    })
}

func (m *SessionManager) ClearActiveTask(sessionID string) error {
    ctx := context.Background()
    return m.queries.ClearSessionActiveTask(ctx, sessionID)
}
```

### 2.4 Objective Detection Algorithm

**File(s):** `internal/agent/runner/runner.go` (lines ~2015-2161)

The objective detector classifies each user message to determine whether it sets, updates, clears, or keeps the current active objective. It runs asynchronously to avoid blocking the main agentic loop.

**Trigger conditions (in `Run()`):**
```go
// Background objective detection: classify user message to set/update/clear active task.
// Fires before runLoop so the objective is available by iteration 2+.
if req.Prompt != "" && len(req.Prompt) >= 20 && !req.SkipMemoryExtract {
    if r.backgroundFunc != nil {
        sessID, sessKey, prompt := sess.ID, req.SessionKey, req.Prompt
        r.backgroundFunc(func(ctx context.Context) error {
            r.detectAndSetObjective(sessID, sessKey, prompt)
            return nil
        }, "objective-detect:"+req.SessionKey)
    } else {
        go r.detectAndSetObjective(sess.ID, req.SessionKey, req.Prompt)
    }
}
```

**Key constraints:**
- Message must be >= 20 characters (short messages skip detection)
- `SkipMemoryExtract` flag suppresses detection (used for heartbeats, system triggers)
- Uses `sync.Map` deduplication to prevent overlapping detections on the same session
- 15-second timeout via `context.WithTimeout(context.Background(), 15*time.Second)`
- Uses cheapest available model (via `selector.GetCheapestModel()`)

**Full detection function:**

```go
func (r *Runner) detectAndSetObjective(sessionID, sessionKey, userPrompt string) {
    // Prevent overlapping detections for the same session
    if _, running := r.detectingObjective.LoadOrStore(sessionID, true); running {
        return
    }
    defer r.detectingObjective.Delete(sessionID)

    defer func() {
        if rec := recover(); rec != nil {
            crashlog.LogPanic("runner", rec, map[string]string{
                "op": "objective_detection", "session": sessionID,
            })
        }
    }()

    if len(r.providers) == 0 {
        return
    }

    ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
    defer cancel()

    // Read current objective
    currentObjective, _ := r.sessions.GetActiveTask(sessionID)

    // Build classification prompt
    objDisplay := currentObjective
    if objDisplay == "" {
        objDisplay = "none"
    }
    classifyPrompt := fmt.Sprintf(`Classify this user message relative to the current working objective.

Current objective: %s
User message: %s

Respond with ONLY one JSON line, no markdown:
{"action": "set", "objective": "concise 1-sentence objective"}
OR {"action": "update", "objective": "refined objective"}
OR {"action": "clear"}
OR {"action": "keep"}

Rules:
- "set": User stated a new, distinct objective (e.g., "let's build X", "create Y", "fix Z", "make me a spreadsheet", "open Safari")
- "update": User is refining or adding to the current objective (e.g., "also add tests", "and make it async")
- "clear": User is done or moving on without a new goal (e.g., "thanks", "looks good", "never mind")
- "keep": No change needed (questions, feedback, corrections about the CURRENT objective)
- Short messages (<15 words) that are CONVERSATIONAL with no action verb -> "keep" (e.g., "will do", "ok", "sure")
- Short messages (<15 words) that contain an ACTION or REQUEST -> "set" (e.g., "create a file", "open Numbers", "fix the bug")
- If the message asks for something DIFFERENT from the current objective, use "set" -- even if it is short
- If unsure, use "keep"`, objDisplay, userPrompt)

    // Get cheapest model
    var provider ai.Provider
    var modelName string
    if r.selector != nil {
        cheapest := r.selector.GetCheapestModel()
        if cheapest != "" {
            provID, mName := ai.ParseModelID(cheapest)
            if p, ok := r.providerMap[provID]; ok {
                provider = &modelOverrideProvider{Provider: p, model: mName}
                modelName = mName
            }
        }
    }
    if provider == nil && len(r.providers) > 0 {
        provider = r.providers[0]
        modelName = "default"
    }
    if provider == nil {
        return
    }

    // Make LLM call
    streamCh, err := provider.Stream(ctx, &ai.ChatRequest{
        Messages: []session.Message{{Role: "user", Content: classifyPrompt}},
        Model:    modelName,
    })
    if err != nil {
        objectiveLog.Error("detection failed during stream", "error", err)
        return
    }

    // Collect response
    var resp strings.Builder
    for event := range streamCh {
        if event.Type == ai.EventTypeText {
            resp.WriteString(event.Text)
        }
        if event.Type == ai.EventTypeError {
            objectiveLog.Error("detection failed during event", "error", event.Error)
            return
        }
    }

    // Parse JSON response
    respText := strings.TrimSpace(resp.String())
    // Strip markdown code fences if present
    respText = strings.TrimPrefix(respText, "```json")
    respText = strings.TrimPrefix(respText, "```")
    respText = strings.TrimSuffix(respText, "```")
    respText = strings.TrimSpace(respText)

    var result struct {
        Action    string `json:"action"`
        Objective string `json:"objective"`
    }
    if err := json.Unmarshal([]byte(respText), &result); err != nil {
        objectiveLog.Error("detection failed during parse", "error", err, "response", respText)
        return
    }

    switch result.Action {
    case "set":
        if result.Objective != "" {
            objectiveLog.Info("objective set", "objective", result.Objective)
            if err := r.sessions.SetActiveTask(sessionID, result.Objective); err != nil {
                objectiveLog.Error("SetActiveTask failed", "error", err)
            }
            // Clear work tasks -- new objective means fresh task list
            if botTool := r.tools.GetBotTool(); botTool != nil {
                botTool.ClearWorkTasks(sessionKey)
            }
        }
    case "update":
        if result.Objective != "" {
            objectiveLog.Info("objective updated", "objective", result.Objective)
            if err := r.sessions.SetActiveTask(sessionID, result.Objective); err != nil {
                objectiveLog.Error("SetActiveTask failed", "error", err)
            }
        }
    case "clear":
        objectiveLog.Info("objective cleared")
        _ = r.sessions.ClearActiveTask(sessionID)
        if botTool := r.tools.GetBotTool(); botTool != nil {
            botTool.ClearWorkTasks(sessionKey)
        }
    case "keep":
        // No change
    default:
        objectiveLog.Warn("unknown action", "action", result.Action)
    }
}
```

### 2.5 Objective Detection -- Action Semantics

| Action | Effect on Objective | Effect on Work Tasks | When |
|---|---|---|---|
| `set` | Replace with new objective | Clear all | New distinct goal stated |
| `update` | Replace with refined version | Keep existing | Refinement of current goal |
| `clear` | Set to NULL | Clear all | User is done or moving on |
| `keep` | No change | No change | Default; questions, feedback |

### 2.6 Rust Implementation (Partial)

The Rust runner already implements objective detection in `crates/agent/src/runner.rs`:

```rust
/// Detect user's working objective from latest message.
/// Runs as a background task (fire-and-forget) before the main loop.
async fn detect_objective(
    providers: &Arc<RwLock<Vec<Box<dyn Provider>>>>,
    sessions: &SessionManager,
    session_id: &str,
    user_prompt: &str,
) {
    // ... classification prompt identical to Go ...
    match result.action.as_str() {
        "set" if !result.objective.is_empty() => {
            let _ = sessions.set_active_task(session_id, &result.objective);
        }
        "update" if !result.objective.is_empty() => {
            let _ = sessions.set_active_task(session_id, &result.objective);
        }
        "clear" => {
            let _ = sessions.clear_active_task(session_id);
        }
        "keep" | _ => {}
    }
}
```

**CRITICAL gap in Rust:** The Rust `detect_objective` does NOT clear work tasks on `set` or `clear` actions. The Go version calls `botTool.ClearWorkTasks(sessionKey)` in both cases. This must be added when work task support is implemented.

---

## 3. Compaction Survival

**File(s):** `internal/agent/runner/runner.go` (lines ~1843-1893, ~670-694)

### 3.1 State Checkpoint

When the context window exceeds `AutoCompact` threshold, the runner reduces the window to current-run messages only. To prevent the agent from losing focus, a synthetic user-role message called a "state checkpoint" is injected at the START of the reduced window.

```go
func (r *Runner) buildStateCheckpoint(sessionKey, activeTask string, allMessages []session.Message) *session.Message {
    if activeTask == "" {
        return nil
    }

    var b strings.Builder
    b.WriteString("[State Checkpoint -- context was compressed, here is your current state]\n\n")
    b.WriteString("## Active Objective\n")
    b.WriteString(activeTask)
    b.WriteString("\nThe user explicitly asked you to work on this. Continue without hesitation.\n")

    // Inject the original user request to preserve task parameters (dates, quantities, etc.)
    for _, msg := range allMessages {
        if msg.Role == "user" && msg.Content != "" {
            content := msg.Content
            if len(content) > 500 {
                content = content[:500] + "..."
            }
            b.WriteString("\n## Original User Request\n")
            b.WriteString(content)
            b.WriteString("\n")
            break
        }
    }

    // Include work tasks if available
    if botTool := r.tools.GetBotTool(); botTool != nil {
        tasks := botTool.ListWorkTasks(sessionKey)
        if len(tasks) > 0 {
            b.WriteString("\n## Work Tasks\n")
            for _, wt := range tasks {
                icon := "[ ]"
                switch wt.Status {
                case "in_progress":
                    icon = "[>]"
                case "completed":
                    icon = "[x]"
                }
                b.WriteString(fmt.Sprintf("  %s [%s] %s\n", icon, wt.ID, wt.Subject))
            }
        }
    }

    return &session.Message{
        Role:    "user",
        Content: b.String(),
    }
}
```

### 3.2 Checkpoint Injection Site

The checkpoint is injected in `runLoop` when compaction triggers:

```go
if estimatedTokens > thresholds.AutoCompact {
    // Read active task before reducing window (needed for checkpoint)
    compactActiveTask, _ := r.sessions.GetActiveTask(sessionID)

    // Reduce window: keep only messages from the current run + a few prior
    if currentRunStart < len(allMessages) {
        messages = allMessages[currentRunStart:]
    } else if len(allMessages) > 5 {
        messages = allMessages[len(allMessages)-5:]
    }

    // Inject state checkpoint so the agent knows what it was doing
    if checkpoint := r.buildStateCheckpoint(sessionKey, compactActiveTask, allMessages); checkpoint != nil {
        messages = append([]session.Message{*checkpoint}, messages...)
    }
}
```

### 3.3 Rolling Summary Objective Preservation

The rolling summary also preserves the objective. When building a cumulative summary in the background, the runner re-reads the objective for freshness:

```go
// Re-read objective inside goroutine for freshness
obj, _ := r.sessions.GetActiveTask(sessionID)
combined := r.buildCumulativeSummary(sessionID, newSummary, obj)
```

### 3.4 Dynamic Suffix Objective Injection

Every iteration, the objective is injected into the system prompt dynamic suffix:

```go
// 4. Background objective (soft pin -- yields to fresh user requests)
if dctx.ActiveTask != "" {
    sb.WriteString("\n\n---\n## Background Objective\n")
    sb.WriteString("Ongoing work: ")
    sb.WriteString(dctx.ActiveTask)
    sb.WriteString("\nThis is context about previous work in this session. The user's latest message ALWAYS takes priority over this objective. Only continue this work if the user explicitly asks to resume (e.g., \"keep going\", \"continue\", \"back to that\").")
    sb.WriteString("\nFor multi-step work, use bot(resource: task, action: create) to track steps, then update them as you go.")
    sb.WriteString("\n---")
}
```

### 3.5 Rust Implementation Target

```rust
fn build_state_checkpoint(
    active_task: &str,
    all_messages: &[ChatMessage],
    work_tasks: &[WorkTask],
) -> Option<ChatMessage> {
    if active_task.is_empty() {
        return None;
    }

    let mut content = String::new();
    content.push_str("[State Checkpoint -- context was compressed, here is your current state]\n\n");
    content.push_str("## Active Objective\n");
    content.push_str(active_task);
    content.push_str("\nThe user explicitly asked you to work on this. Continue without hesitation.\n");

    // Find first user message for original request
    if let Some(msg) = all_messages.iter().find(|m| m.role == "user" && !m.content.is_empty()) {
        let truncated = if msg.content.len() > 500 {
            format!("{}...", &msg.content[..500])
        } else {
            msg.content.clone()
        };
        content.push_str("\n## Original User Request\n");
        content.push_str(&truncated);
        content.push('\n');
    }

    if !work_tasks.is_empty() {
        content.push_str("\n## Work Tasks\n");
        for wt in work_tasks {
            let icon = match wt.status.as_str() {
                "in_progress" => "[>]",
                "completed" => "[x]",
                _ => "[ ]",
            };
            content.push_str(&format!("  {} [{}] {}\n", icon, wt.id, wt.subject));
        }
    }

    Some(ChatMessage {
        role: "user".to_string(),
        content,
        ..Default::default()
    })
}
```

---

## 4. Tier 2 -- Work Tasks

**File(s):** `internal/agent/tools/bot_tool.go`, `internal/agent/tools/types.go`

### 4.1 WorkTask Struct

```go
// WorkTask is an in-memory work tracking item created by the agent to track progress
// on its current objective. Ephemeral -- does not survive restart.
type WorkTask struct {
    ID        string    `json:"id"`
    Subject   string    `json:"subject"`
    Status    string    `json:"status"` // pending, in_progress, completed
    CreatedAt time.Time `json:"created_at"`
}
```

### 4.2 ID Generation

Work task IDs are short numeric strings generated by an atomic counter:

```go
// botWorkTaskCounter generates short numeric IDs for work tasks.
var botWorkTaskCounter atomic.Int64
```

Each `taskCreate` call increments the counter and formats the result as a decimal string:

```go
id := strconv.FormatInt(botWorkTaskCounter.Add(1), 10)
```

### 4.3 Storage Architecture

Work tasks use a dual-storage architecture:

**Primary:** `sync.Map` keyed by `sessionKey`, values are `*[]WorkTask`:

```go
// Work task tracking (in-memory, session-scoped)
workTasks sync.Map // sessionKey -> *[]WorkTask
```

**Backup:** `sessions.work_tasks` column (JSON serialized):

```sql
-- Column added by migration
ALTER TABLE sessions ADD COLUMN work_tasks TEXT;

-- name: GetSessionWorkTasks :one
SELECT COALESCE(work_tasks, '') as work_tasks
FROM sessions WHERE id = ?;

-- name: SetSessionWorkTasks :exec
UPDATE sessions
SET work_tasks = ?, updated_at = unixepoch()
WHERE id = ?;
```

### 4.4 Hydration (Lazy Load from DB)

On first access, if the `sync.Map` is empty for a session, work tasks are hydrated from the database. The atomic counter is also adjusted to avoid ID collisions:

```go
func (t *BotTool) ListWorkTasks(sessionKey string) []WorkTask {
    if val, ok := t.workTasks.Load(sessionKey); ok {
        return *val.(*[]WorkTask)
    }
    // Hydrate from DB if available
    if t.sessions != nil {
        if tasks := t.hydrateWorkTasks(sessionKey); len(tasks) > 0 {
            return tasks
        }
    }
    return nil
}

func (t *BotTool) hydrateWorkTasks(sessionKey string) []WorkTask {
    sess, err := t.sessions.GetOrCreate(sessionKey, t.currentUserID)
    if err != nil || sess == nil {
        return nil
    }
    tasksJSON, err := t.sessions.GetWorkTasks(sess.ID)
    if err != nil || tasksJSON == "" {
        return nil
    }
    var tasks []WorkTask
    if err := json.Unmarshal([]byte(tasksJSON), &tasks); err != nil {
        return nil
    }
    if len(tasks) > 0 {
        t.workTasks.Store(sessionKey, &tasks)
        // Update the atomic counter to avoid ID collisions
        for _, wt := range tasks {
            if id, err := strconv.ParseInt(wt.ID, 10, 64); err == nil {
                for {
                    cur := botWorkTaskCounter.Load()
                    if id <= cur || botWorkTaskCounter.CompareAndSwap(cur, id) {
                        break
                    }
                }
            }
        }
    }
    return tasks
}
```

### 4.5 Persistence (Write-Through)

Every CRUD operation calls `persistWorkTasks` to serialize the current state to the database:

```go
func (t *BotTool) persistWorkTasks(ctx context.Context, sessionKey string) {
    if t.sessions == nil {
        return
    }
    sessionID := GetSessionID(ctx)
    if sessionID == "" {
        return
    }
    var tasks []WorkTask
    if val, ok := t.workTasks.Load(sessionKey); ok {
        tasks = *val.(*[]WorkTask)
    }
    data, err := json.Marshal(tasks)
    if err != nil {
        return
    }
    _ = t.sessions.SetWorkTasks(sessionID, string(data))
}
```

### 4.6 Clear

```go
func (t *BotTool) ClearWorkTasks(sessionKey string) {
    t.workTasks.Delete(sessionKey)
}
```

Note: `ClearWorkTasks` only clears the in-memory `sync.Map` entry. It does NOT clear the DB column. The next `persistWorkTasks` call (from a CRUD operation) would write an empty array, but if no CRUD follows, the stale data remains in the DB.

### 4.7 Tool API -- Create

```go
func (t *BotTool) taskCreate(ctx context.Context, in BotInput) (*ToolResult, error) {
    if in.Subject == "" {
        return &ToolResult{Content: "Error: 'subject' is required for create action", IsError: true}, nil
    }
    sessionKey := GetSessionKey(ctx)
    if sessionKey == "" {
        sessionKey = "default"
    }
    id := strconv.FormatInt(botWorkTaskCounter.Add(1), 10)
    task := WorkTask{
        ID:        id,
        Subject:   in.Subject,
        Status:    "pending",
        CreatedAt: time.Now(),
    }
    initial := []WorkTask{task}
    for {
        existing, loaded := t.workTasks.LoadOrStore(sessionKey, &initial)
        if !loaded {
            break
        }
        ptr := existing.(*[]WorkTask)
        updated := append(*ptr, task)
        if t.workTasks.CompareAndSwap(sessionKey, ptr, &updated) {
            break
        }
    }
    t.persistWorkTasks(ctx, sessionKey)
    return &ToolResult{Content: fmt.Sprintf("Task [%s] created: %s", id, in.Subject)}, nil
}
```

The `LoadOrStore` + `CompareAndSwap` loop is a lock-free concurrent-safe append pattern. If another goroutine modifies the slice pointer between load and swap, the CAS fails and the loop retries.

### 4.8 Tool API -- Update

```go
func (t *BotTool) taskUpdate(ctx context.Context, in BotInput) (*ToolResult, error) {
    if in.TaskID == "" {
        return &ToolResult{Content: "Error: 'task_id' is required for update action", IsError: true}, nil
    }
    if in.Status == "" {
        return &ToolResult{Content: "Error: 'status' is required for update action (in_progress, completed)", IsError: true}, nil
    }
    if in.Status != "pending" && in.Status != "in_progress" && in.Status != "completed" {
        return &ToolResult{
            Content: fmt.Sprintf("Error: invalid status '%s' -- must be pending, in_progress, or completed", in.Status),
            IsError: true,
        }, nil
    }
    sessionKey := GetSessionKey(ctx)
    if sessionKey == "" {
        sessionKey = "default"
    }
    val, ok := t.workTasks.Load(sessionKey)
    if !ok {
        return &ToolResult{Content: fmt.Sprintf("Error: task %s not found", in.TaskID), IsError: true}, nil
    }
    tasks := val.(*[]WorkTask)
    for i := range *tasks {
        if (*tasks)[i].ID == in.TaskID {
            (*tasks)[i].Status = in.Status
            t.persistWorkTasks(ctx, sessionKey)
            return &ToolResult{Content: fmt.Sprintf("Task [%s] -> %s", in.TaskID, in.Status)}, nil
        }
    }
    return &ToolResult{Content: fmt.Sprintf("Error: task %s not found", in.TaskID), IsError: true}, nil
}
```

### 4.9 Tool API -- Delete

```go
func (t *BotTool) taskDelete(ctx context.Context, in BotInput) (*ToolResult, error) {
    if in.TaskID == "" {
        return &ToolResult{Content: "Error: 'task_id' is required for delete action", IsError: true}, nil
    }
    sessionKey := GetSessionKey(ctx)
    if sessionKey == "" {
        sessionKey = "default"
    }
    val, ok := t.workTasks.Load(sessionKey)
    if !ok {
        return &ToolResult{Content: fmt.Sprintf("Error: task %s not found", in.TaskID), IsError: true}, nil
    }
    ptr := val.(*[]WorkTask)
    tasks := *ptr
    for i, wt := range tasks {
        if wt.ID == in.TaskID {
            updated := append(tasks[:i], tasks[i+1:]...)
            t.workTasks.Store(sessionKey, &updated)
            t.persistWorkTasks(ctx, sessionKey)
            return &ToolResult{Content: fmt.Sprintf("Task [%s] deleted: %s", wt.ID, wt.Subject)}, nil
        }
    }
    return &ToolResult{Content: fmt.Sprintf("Error: task %s not found", in.TaskID), IsError: true}, nil
}
```

### 4.10 Tool API -- List

The list action combines work tasks AND sub-agents into a single response:

```go
func (t *BotTool) taskList(ctx context.Context) (*ToolResult, error) {
    var result strings.Builder
    hasContent := false

    sessionKey := GetSessionKey(ctx)
    if sessionKey == "" {
        sessionKey = "default"
    }
    if val, ok := t.workTasks.Load(sessionKey); ok {
        tasks := *val.(*[]WorkTask)
        if len(tasks) > 0 {
            result.WriteString(fmt.Sprintf("Work tasks (%d):\n", len(tasks)))
            for _, wt := range tasks {
                icon := "[ ]"
                switch wt.Status {
                case "in_progress":
                    icon = "[->]"
                case "completed":
                    icon = "[x]"
                }
                result.WriteString(fmt.Sprintf("  %s [%s] %s\n", icon, wt.ID, wt.Subject))
            }
            hasContent = true
        }
    }

    if t.orchestrator != nil {
        agents := t.orchestrator.ListAgents()
        if len(agents) > 0 {
            if hasContent {
                result.WriteString("\n")
            }
            result.WriteString(fmt.Sprintf("Sub-agents (%d):\n\n", len(agents)))
            for _, agent := range agents {
                result.WriteString(fmt.Sprintf("ID: %s\n", agent.ID))
                result.WriteString(fmt.Sprintf("  Description: %s\n", agent.Description))
                result.WriteString(fmt.Sprintf("  Status: %s\n", agent.Status))
                result.WriteString(fmt.Sprintf("  Started: %s\n", agent.StartedAt.Format(time.RFC3339)))
                if !agent.CompletedAt.IsZero() {
                    result.WriteString(fmt.Sprintf("  Completed: %s\n", agent.CompletedAt.Format(time.RFC3339)))
                }
                result.WriteString("\n")
            }
            hasContent = true
        }
    }

    if !hasContent {
        return &ToolResult{Content: "No tasks or sub-agents"}, nil
    }
    return &ToolResult{Content: result.String()}, nil
}
```

### 4.11 Rust Implementation Target

```rust
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

static WORK_TASK_COUNTER: AtomicI64 = AtomicI64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkTask {
    pub id: String,
    pub subject: String,
    pub status: String, // "pending", "in_progress", "completed"
    pub created_at: DateTime<Utc>,
}

/// In-memory work task store, keyed by session_key.
/// Backed by sessions.work_tasks JSON column for restart survival.
pub struct WorkTaskStore {
    tasks: RwLock<HashMap<String, Vec<WorkTask>>>,
    store: Arc<db::Store>,
}

impl WorkTaskStore {
    pub fn new(store: Arc<db::Store>) -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
            store,
        }
    }

    pub async fn list(&self, session_key: &str, session_id: &str) -> Vec<WorkTask> {
        let map = self.tasks.read().await;
        if let Some(tasks) = map.get(session_key) {
            return tasks.clone();
        }
        drop(map);

        // Hydrate from DB
        let json = self.store.get_session_work_tasks(session_id).unwrap_or_default();
        if json.is_empty() {
            return vec![];
        }
        let tasks: Vec<WorkTask> = serde_json::from_str(&json).unwrap_or_default();
        if !tasks.is_empty() {
            // Update counter to avoid ID collisions
            for wt in &tasks {
                if let Ok(id) = wt.id.parse::<i64>() {
                    WORK_TASK_COUNTER.fetch_max(id, Ordering::Relaxed);
                }
            }
            let mut map = self.tasks.write().await;
            map.insert(session_key.to_string(), tasks.clone());
        }
        tasks
    }

    pub async fn create(&self, session_key: &str, session_id: &str, subject: &str) -> WorkTask {
        let id = WORK_TASK_COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
        let task = WorkTask {
            id: id.to_string(),
            subject: subject.to_string(),
            status: "pending".to_string(),
            created_at: Utc::now(),
        };
        {
            let mut map = self.tasks.write().await;
            map.entry(session_key.to_string())
                .or_default()
                .push(task.clone());
        }
        self.persist(session_key, session_id).await;
        task
    }

    pub async fn update_status(
        &self,
        session_key: &str,
        session_id: &str,
        task_id: &str,
        status: &str,
    ) -> Result<(), String> {
        let mut map = self.tasks.write().await;
        let tasks = map.get_mut(session_key).ok_or("task not found")?;
        let task = tasks.iter_mut()
            .find(|t| t.id == task_id)
            .ok_or("task not found")?;
        task.status = status.to_string();
        drop(map);
        self.persist(session_key, session_id).await;
        Ok(())
    }

    pub async fn delete(
        &self,
        session_key: &str,
        session_id: &str,
        task_id: &str,
    ) -> Result<String, String> {
        let mut map = self.tasks.write().await;
        let tasks = map.get_mut(session_key).ok_or("task not found")?;
        let idx = tasks.iter()
            .position(|t| t.id == task_id)
            .ok_or("task not found")?;
        let removed = tasks.remove(idx);
        drop(map);
        self.persist(session_key, session_id).await;
        Ok(removed.subject)
    }

    pub async fn clear(&self, session_key: &str) {
        let mut map = self.tasks.write().await;
        map.remove(session_key);
    }

    async fn persist(&self, session_key: &str, session_id: &str) {
        let map = self.tasks.read().await;
        let tasks = map.get(session_key).cloned().unwrap_or_default();
        drop(map);
        if let Ok(json) = serde_json::to_string(&tasks) {
            let _ = self.store.set_session_work_tasks(session_id, &json);
        }
    }
}
```

---

## 5. Tier 3 -- Sub-agent Tasks

**File(s):** `internal/agent/orchestrator/orchestrator.go`, `internal/agent/recovery/recovery.go`, `internal/db/migrations/0022_pending_tasks.sql`

### 5.1 pending_tasks Schema

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

CREATE INDEX idx_pending_tasks_status ON pending_tasks(status);
CREATE INDEX idx_pending_tasks_lane ON pending_tasks(lane, status);
CREATE INDEX idx_pending_tasks_user ON pending_tasks(user_id, status);
CREATE INDEX idx_pending_tasks_parent ON pending_tasks(parent_task_id);
```

### 5.2 SubAgent Struct (Go)

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

### 5.3 SpawnRequest Struct

```go
type SpawnRequest struct {
    ParentSessionKey string        // Parent session for context inheritance
    ParentSessionID  string        // Parent session DB ID for result delivery (wait=false)
    Task             string        // Task description for the sub-agent
    Description      string        // Short description for tracking
    Lane             string        // Lane to run in (default: LaneSubagent)
    Wait             bool          // Wait for completion before returning
    Timeout          time.Duration // Context timeout for the agent
    SystemPrompt     string        // Optional custom system prompt
    ModelOverride    string        // Override model
}
```

### 5.4 Orchestrator Configuration

```go
type Orchestrator struct {
    mu            sync.RWMutex
    agents        map[string]*SubAgent    // All agents, keyed by agent ID
    sessions      *session.Manager
    providers     []ai.Provider
    tools         ToolExecutor
    config        *config.Config
    recovery      *recovery.Manager

    maxConcurrent int   // Default: 5
    maxPerParent  int   // Default: 0 (unlimited, NOT enforced)

    results chan AgentResult  // Buffered channel (capacity 100)
}
```

### 5.5 Spawn Flow (Three-Phase with Double-Checked Locking)

**Phase 1: First concurrency check (under lock)**
```go
o.mu.Lock()
runningCount := 0
for _, a := range o.agents {
    if a.Status == StatusRunning { runningCount++ }
}
if runningCount >= o.maxConcurrent {
    o.mu.Unlock()
    return nil, fmt.Errorf("max concurrent agents reached (%d)", o.maxConcurrent)
}
o.mu.Unlock()
```

**Phase 2: Generate IDs and persist (NO lock held)**
1. Generate agent ID: `fmt.Sprintf("agent-%d", time.Now().UnixNano())`
2. Create context with `WithTimeout` or `WithCancel`
3. Generate session key: `fmt.Sprintf("subagent-%s", agentID)`
4. Create SubAgent struct with `StatusPending`
5. Persist to `pending_tasks` BEFORE spawning (crash safety)

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
o.recovery.CreateTask(ctx, task)
```

**Phase 3: Register under lock (double-check)**
```go
o.mu.Lock()
// Re-count running agents (race detection)
if runningCount >= o.maxConcurrent {
    o.mu.Unlock()
    cancel()
    o.recovery.MarkFailed(ctx, taskID, "spawn limit race")
    return nil, err
}
agent.TaskID = taskID
o.agents[agentID] = agent
o.mu.Unlock()
```

**Launch:** `go o.runAgent(agentCtx, agent, req, sessionKey)`

**Wait behavior:**
- `wait=true`: calls `waitForAgent(context.Background(), agentID)` -- uses `context.Background()` so wait is decoupled from parent cancellation
- `wait=false`: returns immediately

### 5.6 Execution Loop

```go
func (o *Orchestrator) executeLoop(ctx context.Context, sessionID, systemPrompt, modelOverride string, agent *SubAgent) (string, error)
```

**Max iterations:** `config.MaxIterations` or 50 if not set.

**Loop:**
```
for iteration := 0; iteration < maxIterations; iteration++ {
    1. Check ctx.Done() -> StatusCancelled, return
    2. Get session messages (with MaxContext limit)
    3. Stream from providers[0] (always first provider, no fallback)
    4. Process events:
       - EventTypeText:     append to assistantContent + finalResult
       - EventTypeToolCall: collect tool calls
       - EventTypeError:    return with error
    5. Save assistant message (content + tool_calls JSON)
    6. If has tool calls:
       - Execute each via o.tools.Execute()
       - Save tool results as "tool" role message
       - continue (next iteration)
    7. If no tool calls: break (task complete)
}
return finalResult, nil
```

### 5.7 Result Delivery for wait=false

When a non-waited agent completes, its result is appended to the parent session as a user-role message:

```go
func (o *Orchestrator) deliverResultToParent(agent *SubAgent) {
    // Build message
    var sb strings.Builder
    sb.WriteString(fmt.Sprintf("[Sub-agent completed: %s]\n", agent.Description))
    sb.WriteString(fmt.Sprintf("Agent ID: %s\n", agent.ID))
    sb.WriteString(fmt.Sprintf("Status: %s\n", agent.Status))
    sb.WriteString(fmt.Sprintf("Duration: %s\n", agent.CompletedAt.Sub(agent.StartedAt).Round(time.Second)))
    if agent.Error != nil {
        sb.WriteString(fmt.Sprintf("Error: %v\n", agent.Error))
    }
    sb.WriteString("\nResult:\n")
    result := agent.Result
    if len(result) > 4000 {
        result = result[:4000] + "\n... (truncated)"
    }
    sb.WriteString(result)

    o.sessions.AppendMessage(agent.parentSessionID, session.Message{
        SessionID: agent.parentSessionID,
        Role:      "user",
        Content:   sb.String(),
    })
}
```

- Result truncation: if `len(result) > 4000`, truncates and appends `"\n... (truncated)"`
- Uses `agent.parentSessionID` as the session ID
- Only fires for agents where `!agent.waited && agent.parentSessionID != ""`

### 5.8 Tool API -- Spawn

```go
func (t *BotTool) taskSpawn(ctx context.Context, in BotInput) (*ToolResult, error) {
    if in.Prompt == "" {
        return &ToolResult{Content: "Error: 'prompt' is required for spawn action", IsError: true}, nil
    }
    if in.Description == "" {
        in.Description = truncateForDescription(in.Prompt)
    }
    wait := true
    if in.Wait != nil {
        wait = *in.Wait
    }
    timeout := 300
    if in.Timeout > 0 {
        timeout = in.Timeout
    }

    systemPrompt := buildAgentSystemPrompt(in.AgentType, in.Prompt)

    var subagentModel string
    if cfg := provider.GetModelsConfig(); cfg != nil && cfg.LaneRouting != nil && cfg.LaneRouting.Subagent != "" {
        subagentModel = cfg.LaneRouting.Subagent
    }

    agent, err := t.orchestrator.Spawn(ctx, &orchestrator.SpawnRequest{
        ParentSessionID: GetSessionID(ctx),
        Task:            in.Prompt,
        Description:     in.Description,
        Wait:            wait,
        Timeout:         time.Duration(timeout) * time.Second,
        SystemPrompt:    systemPrompt,
        ModelOverride:   subagentModel,
    })
    if err != nil {
        return &ToolResult{Content: fmt.Sprintf("Failed to spawn sub-agent: %v", err), IsError: true}, nil
    }

    if wait {
        var result strings.Builder
        result.WriteString(fmt.Sprintf("Sub-agent completed: %s\n", in.Description))
        result.WriteString(fmt.Sprintf("Status: %s\n", agent.Status))
        result.WriteString(fmt.Sprintf("Duration: %s\n\n", agent.CompletedAt.Sub(agent.StartedAt).Round(time.Second)))
        if agent.Error != nil {
            result.WriteString(fmt.Sprintf("Error: %v\n\n", agent.Error))
        }
        if agent.Result != "" {
            result.WriteString("Result:\n")
            result.WriteString(agent.Result)
        }
        return &ToolResult{Content: result.String(), IsError: agent.Status == orchestrator.StatusFailed}, nil
    }

    return &ToolResult{
        Content: fmt.Sprintf("Sub-agent spawned: %s\nAgent ID: %s\nDescription: %s\n\nThe agent is running in the background.",
            in.Description, agent.ID, in.Prompt),
    }, nil
}
```

### 5.9 Agent Type System Prompts

```go
func buildAgentSystemPrompt(agentType, task string) string {
    base := `You are a focused sub-agent working on a specific task. Complete the task efficiently and report your results.

Guidelines:
1. Focus ONLY on the assigned task
2. Use tools as needed to accomplish the task
3. Work independently - do not ask for clarification
4. When complete, report the result concisely -- key facts only, no preamble
5. If you encounter errors, try to resolve them

`

    switch agentType {
    case "explore":
        return base + `You are an EXPLORATION agent. Your job is to:
- Search through codebases to find relevant files and code
- Understand patterns and architecture
- Report findings clearly
- Do NOT modify any files - only read and analyze

Task: ` + task

    case "plan":
        return base + `You are a PLANNING agent. Your job is to:
- Analyze the task and break it into steps
- Identify files that need to be modified
- Consider edge cases and potential issues
- Create a clear, actionable plan
- Do NOT implement the plan - only create it

Task: ` + task

    default:
        return base + `Task: ` + task
    }
}
```

### 5.10 BotInput Fields for Task Operations

```go
type BotInput struct {
    Resource    string  `json:"resource"`      // "task"
    Action      string  `json:"action"`        // spawn/status/cancel/list/create/update/delete
    Description string  `json:"description"`   // Short description
    Prompt      string  `json:"prompt"`        // Full task text
    Wait        *bool   `json:"wait"`          // Default: true
    Timeout     int     `json:"timeout"`       // Seconds, default: 300
    AgentType   string  `json:"agent_type"`    // "explore", "plan", "general"
    AgentID     string  `json:"agent_id"`      // For status/cancel
    Subject     string  `json:"subject"`       // For work task create
    TaskID      string  `json:"task_id"`       // For work task update/delete
    Status      string  `json:"status"`        // For work task update
    // ... other fields for non-task resources
}
```

---

## 6. Crash Recovery

**File(s):** `internal/agent/recovery/recovery.go`, `internal/agent/orchestrator/orchestrator.go`

### 6.1 Recovery Manager

```go
type Manager struct {
    db *sql.DB
}

func NewManager(db *sql.DB) *Manager
```

### 6.2 Manager Methods

| Method | SQL | Behavior |
|---|---|---|
| `CreateTask` | INSERT | Auto-generates UUID if empty, defaults status=pending, lane=main, max_attempts=3 |
| `MarkRunning` | UPDATE SET status='running', started_at=?, attempts=attempts+1 | Atomically increments attempts |
| `MarkCompleted` | UPDATE SET status='completed', completed_at=? | Terminal state |
| `MarkFailed` | UPDATE SET status=CASE... | Auto-requeues if attempts < max_attempts |
| `MarkCancelled` | UPDATE SET status='cancelled', last_error='shutdown', completed_at=? | NEVER re-queues |
| `GetRecoverableTasks` | SELECT WHERE status IN ('pending','running') | Returns pending+running ordered by priority DESC, created_at ASC |
| `CheckTaskCompletion` | SELECT from chat_messages | Heuristic: checks session for evidence of completion |
| `CleanupOldTasks` | DELETE WHERE status IN ('completed','failed','cancelled') AND completed_at < 7d ago | 7-day TTL |

### 6.3 MarkFailed Auto-Requeue (CRITICAL)

This is the most subtle behavior. A SINGLE SQL statement handles both retry and permanent failure:

```sql
UPDATE pending_tasks
SET status = CASE WHEN attempts >= max_attempts THEN 'failed' ELSE 'pending' END,
    last_error = ?,
    completed_at = CASE WHEN attempts >= max_attempts THEN ? ELSE NULL END
WHERE id = ?
```

- If `attempts >= max_attempts`: permanently failed, `completed_at` set
- If `attempts < max_attempts`: reset to `pending` for retry, `completed_at` stays NULL

The Rust implementation already replicates this EXACT SQL:

```rust
pub fn update_task_failed(&self, id: &str, last_error: &str) -> Result<(), NeboError> {
    let conn = self.conn()?;
    conn.execute(
        "UPDATE pending_tasks SET
            status = CASE WHEN attempts >= max_attempts THEN 'failed' ELSE 'pending' END,
            last_error = ?2,
            completed_at = CASE WHEN attempts >= max_attempts THEN unixepoch() ELSE NULL END
         WHERE id = ?1",
        params![id, last_error],
    )
    .map_err(|e| NeboError::Database(e.to_string()))?;
    Ok(())
}
```

### 6.4 CheckTaskCompletion Heuristic

Examines the task's session in `chat_messages` to determine if a task was completed despite its DB status (crash recovery scenario):

```go
func (m *Manager) CheckTaskCompletion(ctx context.Context, task *PendingTask) (bool, error) {
    // Step 1: Count messages
    // SELECT COUNT(*) as total,
    //        SUM(CASE WHEN role = 'assistant' THEN 1 ELSE 0 END) as assistant_count,
    //        SUM(CASE WHEN tool_calls IS NOT NULL AND tool_calls != '' AND tool_calls != 'null' THEN 1 ELSE 0 END) as tool_count
    // FROM chat_messages WHERE chat_id = ?

    // Rule 1: No messages -> NOT completed
    if totalCount == 0 { return false, nil }

    // Rule 2: Has tool calls -> completed (avoid duplicate side effects)
    if toolCallCount > 0 { return true, nil }

    // Rule 3: Has assistant responses + multiple messages -> completed
    if assistantCount > 0 && totalCount > 2 { return true, nil }

    // Rule 4: Last message is assistant with >50 chars -> completed
    // SELECT role, content FROM chat_messages WHERE chat_id = ? ORDER BY created_at DESC LIMIT 1
    if lastMsg.Role == "assistant" && len(lastMsg.Content) > 50 { return true, nil }

    // Rule 5: Otherwise NOT completed
    return false, nil
}
```

**Design philosophy:** Deliberately generous -- better to skip a completed task than re-run it and duplicate side effects.

### 6.5 RecoverAgents Algorithm (Orchestrator)

```go
func (o *Orchestrator) RecoverAgents(ctx context.Context) (int, error) {
    const maxRecoveryAge = 2 * time.Hour

    tasks, _ := o.recovery.GetRecoverableTasks(ctx)

    for _, task := range tasks {
        // Skip non-subagent tasks
        if task.TaskType != TaskTypeSubagent { continue }

        // Rule 1: Too old -> mark failed
        if time.Since(task.CreatedAt) > maxRecoveryAge {
            o.recovery.MarkFailed(ctx, task.ID, "stale: exceeded max recovery age")
            continue
        }

        // Rule 2: Exhausted retries -> mark failed
        if task.Attempts >= task.MaxAttempts {
            o.recovery.MarkFailed(ctx, task.ID, "exhausted retry attempts")
            continue
        }

        // Rule 3: Check if actually completed despite status
        completed, err := o.recovery.CheckTaskCompletion(ctx, task)
        if err != nil {
            o.recovery.MarkFailed(ctx, task.ID, err.Error())
            continue
        }
        if completed {
            o.recovery.MarkCompleted(ctx, task.ID)
            continue
        }

        // Re-spawn with recovered ID and original session key
        agentID := fmt.Sprintf("agent-recovered-%s", task.ID[:8])
        agentCtx, cancel := context.WithCancel(ctx)

        agent := &SubAgent{
            ID:          agentID,
            TaskID:      task.ID,     // Original PendingTask ID
            Task:        task.Prompt,
            Description: task.Description,
            Lane:        task.Lane,
            Status:      StatusPending,
            StartedAt:   time.Now(),
            cancel:      cancel,
        }
        o.agents[agentID] = agent

        // Re-use original session key -- agent picks up where it left off
        go o.runAgent(agentCtx, agent, &SpawnRequest{
            Task:         task.Prompt,
            SystemPrompt: task.SystemPrompt,
            Wait:         false, // Recovered agents are always fire-and-forget
        }, task.SessionKey)
    }
}
```

**Key recovery behaviors:**
- Recovered agents always have `wait=false` (fire-and-forget)
- The `parentSessionID` is NOT preserved -- result delivery to parent is lost
- The original session key IS reused -- the agent sees its previous conversation
- Session reuse means the task prompt may be duplicated (original + recovery injection)

### 6.6 Shutdown

```go
func (o *Orchestrator) Shutdown(ctx context.Context) {
    o.mu.Lock()
    var toCancel []*SubAgent
    for _, a := range o.agents {
        if a.Status == StatusRunning || a.Status == StatusPending {
            toCancel = append(toCancel, a)
        }
    }
    o.mu.Unlock()

    for _, agent := range toCancel {
        agent.Status = StatusCancelled
        agent.CompletedAt = time.Now()
        if o.recovery != nil && agent.TaskID != "" {
            o.recovery.MarkCancelled(context.Background(), agent.TaskID)
        }
        if agent.cancel != nil {
            agent.cancel()
        }
    }
}
```

### 6.7 Cleanup

```go
func (o *Orchestrator) Cleanup(maxAge time.Duration) int {
    o.mu.Lock()
    defer o.mu.Unlock()

    var removed int
    for id, agent := range o.agents {
        if agent.Status == StatusRunning || agent.Status == StatusPending {
            continue
        }
        if !agent.CompletedAt.IsZero() && time.Since(agent.CompletedAt) > maxAge {
            delete(o.agents, id)
            removed++
        }
    }
    return removed
}
```

### 6.8 Startup Wiring Sequence

```
1. Create SQLite DB connection
2. session.New(sqlDB)
3. recovery.NewManager(sqlDB)
4. createProviders(cfg) -> providers
5. ... (policy, tool registry, runner setup) ...
6. runner.SetupSubagentPersistence(state.recovery)
   -> botTool.SetRecoveryManager(mgr)
     -> orchestrator.SetRecoveryManager(mgr)
7. runner.RecoverSubagents(ctx)
   -> botTool.RecoverSubagents(ctx)
     -> orchestrator.RecoverAgents(ctx)
```

### 6.9 Recovery Timing and TTLs

| Parameter | Value | Location |
|---|---|---|
| Max recovery age | 2 hours | `orchestrator.RecoverAgents()` |
| Max retry attempts | 3 (default) | `recovery.CreateTask()` default |
| Old task cleanup TTL | 7 days | `recovery.CleanupOldTasks()` |
| Completion check: tool calls | Any tool call = complete | `CheckTaskCompletion()` |
| Completion check: message threshold | assistant > 0 AND total > 2 | `CheckTaskCompletion()` |
| Completion check: content length | Last assistant message > 50 chars | `CheckTaskCompletion()` |

---

## 7. Steering Integration

**File(s):** `internal/agent/steering/generators.go`, `internal/agent/steering/templates.go`, `internal/agent/steering/pipeline.go`

### 7.1 Steering Context (Task-Relevant Fields)

```go
type Context struct {
    SessionID      string
    Messages       []session.Message
    UserPrompt     string
    ActiveTask     string            // Pinned active task (Tier 1)
    Channel        string
    AgentName      string
    Iteration      int               // 1-based
    RunStartTime   time.Time
    WorkTasks      []WorkTask        // In-memory work tasks (Tier 2)
    JanusRateLimit *ai.RateLimitInfo
}

type WorkTask struct {
    ID      string `json:"id"`
    Subject string `json:"subject"`
    Status  string `json:"status"` // pending, in_progress, completed
}
```

### 7.2 Generator 7: objectiveTaskNudge

Fires when the agent has an objective but has NOT created any work tasks after 2+ assistant turns. Nudges the agent to start working immediately.

```go
type objectiveTaskNudge struct{}

func (g *objectiveTaskNudge) Name() string { return "objective_task_nudge" }

func (g *objectiveTaskNudge) Generate(ctx *Context) []Message {
    if ctx.ActiveTask == "" {
        return nil // no objective
    }
    if len(ctx.WorkTasks) > 0 {
        return nil // already has tasks
    }
    if countAssistantTurns(ctx.Messages) < 2 {
        return nil // too early
    }
    return []Message{{
        Content:  wrapSteering(g.Name(), tmplObjectiveTaskNudge),
        Position: PositionEnd,
    }}
}
```

**Template:**
```
You have a clear objective. Start working on it immediately using your tools.
Do NOT create a task list or checklist. Just take the first concrete action toward the goal.
```

### 7.3 Generator 8: pendingTaskAction

Fires when the agent has an objective, is past iteration 1, and gave a text-only response (no tool calls) despite having work to do.

```go
type pendingTaskAction struct{}

func (g *pendingTaskAction) Name() string { return "pending_task_action" }

func (g *pendingTaskAction) Generate(ctx *Context) []Message {
    if ctx.ActiveTask == "" {
        return nil
    }
    if ctx.Iteration < 2 {
        return nil
    }
    // Don't fire if tools were used recently (model is actively working)
    if countTurnsSinceAnyToolUse(ctx.Messages) == 0 {
        return nil
    }

    content := tmplPendingTaskAction
    if ctx.ActiveTask != "" {
        content = fmt.Sprintf("Your objective: %s\n\n%s", ctx.ActiveTask, tmplPendingTaskAction)
    }
    return []Message{{
        Content:  wrapSteering(g.Name(), content),
        Position: PositionEnd,
    }}
}
```

**Template:**
```
You still have work to do -- your last response was text-only but the task is NOT complete.
Call a tool RIGHT NOW to continue. Do NOT respond with text explaining what you plan to do.
Do NOT narrate intent, summarize progress, or create task lists. Just make the next tool call.
```

### 7.4 Generator 9: taskProgress

Re-injects the full work task list every 4th iteration starting at iteration 4. Includes a concrete checklist when work tasks exist.

```go
type taskProgress struct{}

func (g *taskProgress) Name() string { return "task_progress" }

func (g *taskProgress) Generate(ctx *Context) []Message {
    if ctx.ActiveTask == "" {
        return nil
    }
    if ctx.Iteration < 4 || ctx.Iteration%4 != 0 {
        return nil
    }

    content := tmplTaskProgress
    if len(ctx.WorkTasks) > 0 {
        var sb strings.Builder
        sb.WriteString("Your objective: ")
        sb.WriteString(ctx.ActiveTask)
        sb.WriteString("\n\nInternal task state (do NOT reproduce this in your response):\n")
        for _, wt := range ctx.WorkTasks {
            icon := "[ ]"
            switch wt.Status {
            case "in_progress":
                icon = "[->]"
            case "completed":
                icon = "[x]"
            }
            sb.WriteString(fmt.Sprintf("  %s [%s] %s\n", icon, wt.ID, wt.Subject))
        }
        sb.WriteString("\nContinue working on the next incomplete task.")
        content = sb.String()
    }

    return []Message{{
        Content:  wrapSteering(g.Name(), content),
        Position: PositionEnd,
    }}
}
```

**Fallback template (no work tasks):**
```
You are still working toward your objective. Keep going -- use your tools to make progress.
If you've finished, report the outcome in one sentence and stop.
```

### 7.5 Generator 10: activeObjectiveReminder

Lightweight 1-line reminder every iteration when an objective is set. Skips iterations where `taskProgress` fires to avoid double-injection.

```go
type activeObjectiveReminder struct{}

func (g *activeObjectiveReminder) Name() string { return "active_objective_reminder" }

func (g *activeObjectiveReminder) Generate(ctx *Context) []Message {
    if ctx.ActiveTask == "" || ctx.Iteration < 2 {
        return nil
    }
    // Skip iterations where taskProgress fires (every 4 starting at 4)
    if ctx.Iteration >= 4 && ctx.Iteration%4 == 0 {
        return nil
    }
    content := fmt.Sprintf("Your active objective: %s -- keep working on it.", ctx.ActiveTask)
    return []Message{{
        Content:  wrapSteering(g.Name(), content),
        Position: PositionEnd,
    }}
}
```

### 7.6 Generator 11: loopDetector

Detects when the agent is stuck making the same tool call repeatedly. Two severity levels:

```go
type loopDetector struct{}

func (g *loopDetector) Name() string { return "loop_detector" }

func (g *loopDetector) Generate(ctx *Context) []Message {
    if ctx.ActiveTask == "" || ctx.Iteration < 4 {
        return nil
    }
    toolName, streak := countConsecutiveSameToolCalls(ctx.Messages)
    if streak < 4 {
        return nil
    }
    var content string
    if streak < 6 {
        content = fmt.Sprintf(tmplLoopSoftNudge, streak, toolName, toolName)
    } else {
        content = fmt.Sprintf(tmplLoopHardMandate, streak, toolName, toolName)
    }
    return []Message{{Content: wrapSteering(g.Name(), content), Position: PositionEnd}}
}
```

**Soft nudge template (4-5 consecutive):**
```
You have made %d consecutive %s calls without updating your task progress.
Before making another %s call, pause and:
1. State what you've found so far in one sentence
2. Update your work task status with bot(resource: task, action: update)
3. Identify the ONE specific gap you still need to fill
Then make ONE targeted call to fill that gap.
```

**Hard mandate template (6+):**
```
LOOP DETECTED: You have made %d consecutive %s calls without progress.
STOP. Do NOT make another %s call. You are stuck in a loop.
RIGHT NOW you must:
1. Give the user the key result in 2-3 sentences -- no preamble, no checklist
2. Update your work tasks: mark completed ones done, note what's left
3. If you need more information, state exactly what ONE thing you need and why
The user asked you to DO work, not endlessly search. Deliver what you have.
```

### 7.7 Firing Schedule Summary

| Generator | Fires when | Iteration constraint | Mutual exclusion |
|---|---|---|---|
| objectiveTaskNudge | ActiveTask + no WorkTasks + 2+ assistant turns | None | None |
| pendingTaskAction | ActiveTask + text-only response (no tools) | iter >= 2 | None |
| taskProgress | ActiveTask | iter >= 4, iter % 4 == 0 | None |
| activeObjectiveReminder | ActiveTask | iter >= 2, NOT (iter >= 4 AND iter % 4 == 0) | Skips taskProgress iterations |
| loopDetector | ActiveTask + 4+ consecutive same tool | iter >= 4 | streak < 6 = soft, streak >= 6 = hard |

### 7.8 Context Assembly in Runner

The runner reads active task and work tasks each iteration, builds the steering context, and injects the generated messages:

```go
// Build per-iteration dynamic suffix
activeTask, _ := r.sessions.GetActiveTask(sessionID)

// Mid-conversation steering
if r.steering != nil {
    var workTasks []steering.WorkTask
    if botTool := r.tools.GetBotTool(); botTool != nil {
        for _, wt := range botTool.ListWorkTasks(sessionKey) {
            workTasks = append(workTasks, steering.WorkTask{
                ID: wt.ID, Subject: wt.Subject, Status: wt.Status,
            })
        }
    }
    steeringCtx := &steering.Context{
        SessionID:      sessionID,
        Messages:       truncatedMessages,
        UserPrompt:     userPrompt,
        ActiveTask:     activeTask,
        Channel:        channel,
        AgentName:      agentName,
        Iteration:      iteration,
        RunStartTime:   startTime,
        WorkTasks:      workTasks,
        JanusRateLimit: r.latestRateLimit(provider),
    }
    if steeringMsgs := r.steering.Generate(steeringCtx); len(steeringMsgs) > 0 {
        truncatedMessages = steering.Inject(truncatedMessages, steeringMsgs)
    }
}
```

---

## 8. Runner Integration

**File(s):** `internal/agent/runner/runner.go`

### 8.1 Integration Summary Table

| Phase | What Happens | Code Location |
|---|---|---|
| Pre-loop (Run) | Objective detection fires async | runner.go:384-396 |
| On "set" | Clear work tasks, set objective in DB | runner.go:2131-2141 |
| On "update" | Set objective in DB, keep work tasks | runner.go:2143-2149 |
| On "clear" | Clear objective + work tasks | runner.go:2150-2155 |
| Each iteration | Read active task for dynamic suffix | runner.go:793 |
| Each iteration | Read work tasks for steering context | runner.go:826-833 |
| Each iteration | Inject objective into dynamic suffix | prompt.go:964-971 |
| Each iteration | Generate + inject steering messages | runner.go:846-848 |
| On compaction | Build state checkpoint with objective + work tasks + original request | runner.go:679-693 |
| On summary | Include objective in rolling summary | runner.go:1791, 1833-1834 |

### 8.2 Run() Method -- Objective Detection Entry Point

```go
// In Run(), after saving user message, before starting runLoop:
if req.Prompt != "" && len(req.Prompt) >= 20 && !req.SkipMemoryExtract {
    if r.backgroundFunc != nil {
        sessID, sessKey, prompt := sess.ID, req.SessionKey, req.Prompt
        r.backgroundFunc(func(ctx context.Context) error {
            r.detectAndSetObjective(sessID, sessKey, prompt)
            return nil
        }, "objective-detect:"+req.SessionKey)
    } else {
        go r.detectAndSetObjective(sess.ID, req.SessionKey, req.Prompt)
    }
}

resultCh := make(chan ai.StreamEvent, 100)
go r.runLoop(ctx, rs, sess.ID, req.SessionKey, ...)
```

The objective detection runs concurrently with the first iteration of `runLoop`. By iteration 2+, the objective is typically available in the database.

### 8.3 runLoop() -- Per-Iteration Task Integration

Each iteration of the agentic loop performs these task-related operations in order:

1. **Read active task** from DB for dynamic suffix:
```go
activeTask, _ := r.sessions.GetActiveTask(sessionID)
```

2. **Build dynamic suffix** with objective:
```go
dynamicSuffix := BuildDynamicSuffix(DynamicContext{
    ProviderID: provider.ID(),
    ModelName:  modelName,
    ActiveTask: activeTask,
    Summary:    summaryText,
})
```

3. **Read work tasks** from BotTool for steering:
```go
var workTasks []steering.WorkTask
if botTool := r.tools.GetBotTool(); botTool != nil {
    for _, wt := range botTool.ListWorkTasks(sessionKey) {
        workTasks = append(workTasks, steering.WorkTask{
            ID: wt.ID, Subject: wt.Subject, Status: wt.Status,
        })
    }
}
```

4. **Generate steering messages** using task context:
```go
steeringCtx := &steering.Context{
    ActiveTask: activeTask,
    WorkTasks:  workTasks,
    Iteration:  iteration,
    // ...
}
steeringMsgs := r.steering.Generate(steeringCtx)
```

5. **Inject steering** into message list before sending to provider

### 8.4 Compaction -- State Checkpoint

When `estimatedTokens > thresholds.AutoCompact`:

```go
compactActiveTask, _ := r.sessions.GetActiveTask(sessionID)

// Reduce window
if currentRunStart < len(allMessages) {
    messages = allMessages[currentRunStart:]
}

// Inject checkpoint
if checkpoint := r.buildStateCheckpoint(sessionKey, compactActiveTask, allMessages); checkpoint != nil {
    messages = append([]session.Message{*checkpoint}, messages...)
}
```

### 8.5 Rolling Summary -- Objective Inclusion

```go
func (r *Runner) buildRollingSummary(sessionID string, outsideWindow []session.Message, userID string) string {
    // ...
    activeObjective, _ := r.sessions.GetActiveTask(sessionID)

    rollingSummary := existingSummary
    if rollingSummary == "" && len(outsideWindow) > 0 {
        rollingSummary = buildQuickFallbackSummary(outsideWindow, activeObjective)
    }

    // Background: re-read for freshness
    go func() {
        obj, _ := r.sessions.GetActiveTask(sessionID)
        combined := r.buildCumulativeSummary(sessionID, newSummary, obj)
        _ = r.sessions.UpdateSummary(sessionID, combined)
    }()

    return rollingSummary
}
```

---

## 9. Rust Implementation Status

| Feature | Go | Rust | Status |
|---|---|---|---|
| Cron/scheduler system | Y | Y | Y |
| Active objective detection | Y | Y | Y |
| Active objective persistence | Y | Y | Y |
| Active objective in dynamic suffix | Y | Y | Y |
| Work tasks (in-memory) | Y | N | N |
| Work tasks (DB backup) | Y | P | P |
| Work task tool API (create/update/delete/list) | Y | P | P |
| Work task hydration from DB | Y | N | N |
| Work task clear on objective set/clear | Y | N | N |
| Orchestrator (sub-agent spawning) | Y | Y | Y |
| Sub-agent execution loop | Y | Y | Y |
| Sub-agent DAG decomposition | N | Y | Y |
| Crash recovery (pending_tasks) | Y | P | P |
| Recovery: CheckTaskCompletion heuristic | Y | N | N |
| Recovery: RecoverAgents on startup | Y | N | N |
| Recovery: age/retry exhaustion checks | Y | N | N |
| Result delivery (wait=false) | Y | N | N |
| Double-checked locking on spawn | Y | N | N |
| State checkpoint on compaction | Y | N | N |
| Steering: objectiveTaskNudge | Y | Y | Y |
| Steering: pendingTaskAction | Y | Y | Y |
| Steering: taskProgress | Y | Y | Y |
| Steering: activeObjectiveReminder | Y | Y | Y |
| Steering: loopDetector | Y | N | N |
| Compaction state checkpoint | Y | N | N |

**Legend:** Y = implemented, N = not started, P = partially implemented

### 9.1 Key Gaps for Reimplementation Priority

1. **In-memory WorkTaskStore** -- The Rust bot_tool routes task create/update/delete to `pending_tasks` DB table instead of an in-memory store with DB backup. This is semantically wrong: work tasks should be lightweight, session-scoped, numeric-ID items that live in memory with JSON serialization to `sessions.work_tasks`. The current implementation conflates them with sub-agent pending tasks.

2. **Work task clear on objective change** -- The Rust `detect_objective` does NOT call any work task clearing when the action is `set` or `clear`. This will cause stale work tasks from previous objectives to persist.

3. **State checkpoint on compaction** -- The Rust runner has no equivalent of `buildStateCheckpoint`. When context is compacted, the agent loses its objective and work task context.

4. **Loop detector steering** -- Missing from the Rust steering pipeline. Without it, the agent can get stuck in infinite tool-call loops.

5. **Crash recovery** -- The Rust codebase has the `pending_tasks` schema and CRUD queries, but no `RecoverAgents` logic at startup. No `CheckTaskCompletion` heuristic. No age/retry exhaustion checks.

6. **Result delivery** -- When Rust sub-agents complete with `wait=false`, the result is NOT appended to the parent session. The parent model never sees the result.

---

*Generated: 2026-03-04*
