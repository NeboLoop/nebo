# Agentic Loop — SME Deep Dive

> Last updated: 2026-02-25

This document covers the complete lifecycle of the Nebo agentic loop — from user message receipt to response delivery. Read this file to become an agentic loop SME.

---

## Architecture Overview

The agentic loop is the core execution engine of Nebo's agent. It receives a user message, iterates through LLM calls and tool executions until the task is complete, and streams results back in real-time.

**Key principle:** Nebo has ONE eternal conversation per session — it must always be able to continue. Context overflow is handled by compaction, never by failure.

### Component Map

| Component | File | Lines | Responsibility |
|-----------|------|-------|----------------|
| **Runner** | `internal/agent/runner/runner.go` | ~2050 | Main agentic loop, context mgmt, compaction, tool execution |
| **Prompt Builder** | `internal/agent/runner/prompt.go` | ~550 | Two-tier prompt assembly, STRAP docs, platform sections |
| **Agent Hub** | `internal/agenthub/hub.go` | ~620 | WebSocket agent connections, frame routing, sync requests |
| **Lane Manager** | `internal/agenthub/lane.go` | ~480 | Work queues with concurrency limits |
| **Agent Cmd** | `cmd/nebo/agent.go` | ~1000+ | Glue connecting hub to runner via lanes |
| **Model Selector** | `internal/agent/ai/selector.go` | ~300+ | Task classification, model routing, cooldown |
| **Tool Registry** | `internal/agent/tools/registry.go` | ~300+ | Tool registration, execution, approval checking |
| **Orchestrator** | `internal/agent/orchestrator/orchestrator.go` | ~200+ | Sub-agent spawning, recovery persistence |
| **Chat Context** | `internal/realtime/chat.go` | ~400+ | Event relay, streaming, fence buffering, approval routing |
| **Steering** | `internal/agent/steering/generators.go` | ~270 | 10 steering generators for mid-conversation guidance |
| **Session Manager** | `internal/agent/session/` | — | SQLite conversation persistence, compaction |
| **Memory** | `internal/agent/memory/` | — | DB context loading, memory extraction |

---

## Complete Message Lifecycle

### Phase 1: Entry — `Run()` (runner.go:264-336)

When a user sends a message, the flow begins:

```
User message → HTTP handler or WS frame
  → Lane Manager: Enqueue(ctx, LaneMain, task)
  → Runner.Run(ctx, &RunRequest{...})
```

**RunRequest fields:**
```go
type RunRequest struct {
    SessionKey       string       // Session namespace ("default", "companion-default", "dm-{id}", "subagent-{id}")
    Prompt           string       // User message text
    System           string       // Override system prompt (optional)
    ModelOverride    string       // e.g. "anthropic/claude-opus-4-6"
    UserID           string       // For user-scoped operations
    SkipMemoryExtract bool        // True for heartbeats, system tasks
    Origin           tools.Origin // user, comm, app, skill, system
    Channel          string       // web, cli, telegram, discord, slack
    ForceSkill       string       // Pre-load a specific skill
}
```

**Run() does:**
1. **Inject origin into context** (268-271) — tools check `GetOrigin(ctx)` for access control
2. **Reload providers if empty** (276-278) — handles mid-session onboarding
3. **Set session key in context** (288) — tools scope state per-session
4. **Bridge MCP context** (291-292) — CLI providers cross HTTP boundary, losing context values
5. **Get or create session** (296) — user-scoped SQLite persistence
6. **Append user message to session** (309-318)
7. **Trigger background objective detection** (328-330) — async classification of intent
8. **Launch `runLoop()` in goroutine** (333) — returns buffered channel of `StreamEvent`s

### Phase 2: Setup — `runLoop()` top (runner.go:339-448)

**One-time per run:**
1. Create per-run `FenceStore` for AFV (Arithmetic Fence Verification)
2. Set user ID on memory tool
3. **Load DB context** (374-387) — identity, persona, memories from SQLite. Falls back to file-based (AGENTS.md, MEMORY.md, SOUL.md)
4. **Resolve agent name** (393-396) — from DB context or default "Nebo"
5. **Collect tool definitions** (399-403) — all registered tools
6. **Skills handling** (411-430):
   - Force-load skill if explicitly requested or user needs onboarding
   - Auto-match skills against user prompt (trigger keywords)
   - Get active skill content for prompt injection
7. **Build static system prompt** (445-447) — `BuildStaticPrompt(pctx)` — cached by Anthropic for 5 min

### Phase 3: Main Loop — Iteration Cycle (runner.go:458-992)

```
for iteration < maxIterations (default 100) {
    1. Load messages from session
    2. Check context thresholds → compact if needed
    3. Select provider and model
    4. Build enriched prompt (static + dynamic suffix)
    5. Apply context pruning
    6. Generate steering messages
    7. AFV pre-send verification
    8. Stream to LLM provider
    9. Process streaming events
    10. Save assistant message
    11. Execute tool calls → continue loop
    12. Or: no tools → complete, extract memory
}
```

#### Step 1: Load Messages (463-467)

```go
messages, err := r.sessions.GetMessages(sessionID, r.config.MaxContext)
```

Returns last N messages from SQLite. `MaxContext` limits history window.

#### Step 2: Context Threshold Evaluation (471-541)

**Three graduated tiers:**

| Tier | Trigger | Action |
|------|---------|--------|
| Warning | ~20k below effective window | Micro-compact: strip old tool results + images |
| Error | Above error threshold | Log warning |
| AutoCompact | Above auto-compact threshold | Full LLM-based summarization + progressive compaction |

**Compaction flow (when AutoCompact triggered):**
1. Flush memories synchronously before compacting (first time only)
2. `generateSummary()` — uses cheapest available model
3. Extract active task from summary → pin to session via `SetActiveTask()`
4. Build cumulative summary (compress previous summary + prepend new)
5. Progressive compaction — try keeping 10, then 3, then 1 messages:
   ```go
   for _, keep := range []int{10, 3, 1} {
       r.sessions.Compact(sessionID, summary, keep)
       // Index compacted messages for semantic search (async)
       // Reload messages, check if under threshold
   }
   ```
6. Re-inject recently accessed files via `FileAccessTracker` to recover working context
7. **Never block** — proceed with whatever context remains

#### Step 3: Provider Selection (543-603)

**Priority chain:**
1. **User model switch** (544-547) — fuzzy match "use claude" → `anthropic/claude-opus-4-6`
2. **Model override** from RunRequest (555-561)
3. **Selector** (562-571) — task-based routing:
   - Classify task type: Vision, Audio, Reasoning, Code, General
   - Route to best available model per type
   - Respect cooldown (failed models get exponential backoff: 5s→10s→20s...→1hr)
4. **First provider fallback** (576-579) — handles clean installs with only Janus
5. **Friendly error** if no provider at all (581-603) — persisted to session

**Provider map:** `providerMap[providerID]` → pre-built during `ReloadProviders()`. Runtime providers (Janus, gateway apps) bypass `models.yaml` entries.

#### Step 4: Prompt Assembly (605-634)

**Two-tier caching strategy:**

```
┌─────────────────────────────────────────┐
│         STATIC PROMPT (cached 5min)      │
│                                          │
│  DB Context (identity/persona/memories)  │
│  9 section constants:                    │
│    sectionIdentityAndPrime               │
│    sectionCapabilities                   │
│    sectionToolsDeclaration               │
│    sectionCommStyle                      │
│    sectionSTRAPHeader + tool docs        │
│    sectionMediaGuidance                  │
│    sectionMemoryGuidance                 │
│    sectionBehavior                       │
│    sectionAgentName                      │
│  Platform capabilities                   │
│  Tool list (reinforced)                  │
│  Skill hints + active skills             │
│  App catalog                             │
│  Model aliases                           │
│  AFV security guides                     │
├──────────────────────────────────────────┤
│       DYNAMIC SUFFIX (per iteration)     │
│                                          │
│  Date/time (current exact moment)        │
│  System: model name, hostname, OS        │
│  Active task pin                         │
│  Compaction summary                      │
└──────────────────────────────────────────┘
```

**Key insight:** Date/time in dynamic suffix (not static) was the #1 cache optimization — the static prefix can be reused across iterations and across 5-minute Anthropic cache windows.

**Per-iteration:**
```go
dynamicSuffix := BuildDynamicSuffix(DynamicContext{
    ProviderID: provider.ID(),
    ModelName:  modelName,
    ActiveTask: activeTask,
    Summary:    summaryText,
})
enrichedPrompt := systemPrompt + dynamicSuffix
```

**Skill refresh** (616-622): If skill content changed mid-run (model invoked a skill), rebuild static prompt.

**Micro-compact** (628): Silently trims old tool results + strips images. Only activates above warning threshold.

**Two-stage pruning** (630-634):
1. Soft trim: head + tail of long messages
2. Hard clear: replace with placeholder

#### Step 5: Steering Pipeline (636-663)

10 generators inject ephemeral guidance messages. **Never persisted, never shown to user.**

| # | Generator | Trigger | Position | Purpose |
|---|-----------|---------|----------|---------|
| 1 | `identityGuard` | Every 8 assistant turns | End | Re-anchor identity, prevent drift |
| 2 | `channelAdapter` | Non-web channels | End | Channel-specific behavior (Telegram: short replies, etc.) |
| 3 | `toolNudge` | 5+ turns without tool use + active task | End | "Use your tools, don't just chat" |
| 4 | `compactionRecovery` | Just compacted | End | "Don't ask what we were doing" |
| 5 | `dateTimeRefresh` | 30+ min since run start | End | Refresh stale date/time reference |
| 6 | `memoryNudge` | Conditions TBD | End | Remind to store user facts |
| 7 | `objectiveTaskNudge` | Active objective, no work tasks | End | Break objective into work tasks |
| 8 | `pendingTaskAction` | Pending work tasks | End | "Take action, don't narrate" |
| 9 | `taskProgress` | Every 8 iterations + work tasks | End | Re-inject work task list |
| 10 | `janusQuotaWarning` | >80% Janus usage, once/session | End | Warn about budget |

**Injection:** `steering.Inject(truncatedMessages, steeringMsgs)` inserts at `PositionEnd` (before last user message).

#### Step 6: AFV Pre-Send Verification (665-702)

**Arithmetic Fence Verification** — defense against prompt injection in tool results:

1. Check if any fences exist (`fenceStore.Count() > 0`)
2. Build context record from enriched prompt + messages
3. `afv.Verify(fenceStore, contextRecord)` — checks all fence markers intact
4. **If violated:**
   - Log violation details
   - Quarantine response (in-memory `QuarantineStore`)
   - Save sanitized placeholder to session
   - Return "prompt injection detected" to user
   - **Exit loop** — do NOT send to LLM
5. **If passed:** Strip fence markers from messages before sending (prevents LLM echoing them)

#### Step 7: Stream to Provider (707-860)

```go
chatReq := &ai.ChatRequest{
    Messages: truncatedMessages,
    Tools:    chatTools,      // Always all registered tools
    System:   enrichedPrompt,
    Model:    modelName,
}
// Auto-enable thinking for reasoning tasks
if taskType == ai.TaskTypeReasoning && selector.SupportsThinking(model) {
    chatReq.EnableThinking = true
}
events, err := provider.Stream(ctx, chatReq)
```

**Error handling on Stream() failure:**

| Error Type | Handler |
|---|---|
| `IsContextOverflow` | Progressive compaction (try keeping 10→3→1), then `continue` loop |
| `IsRateLimitOrAuth` | Record profile error, mark model failed, `continue` (try different provider) |
| `IsRoleOrderingError` | Retry silently (user doesn't need to know) |
| Generic error | Extract user-friendly message, send to user, `return` |

**Event processing loop:**
```go
for event := range events {
    resultCh <- event  // Forward ALL events immediately (real-time streaming)

    switch event.Type {
    case EventTypeText:    → accumulate assistantContent
    case EventTypeToolCall → validate JSON, append to toolCalls
    case EventTypeError    → send error to user, return
    case EventTypeMessage  → save intermediate messages (CLI provider's internal loop)
    }
}
```

**Tool call JSON validation** (819-822): Invalid JSON input (e.g., concatenated chunks `{...}{...}`) is silently skipped to prevent session poisoning.

#### Step 8: Save Assistant Message (866-895)

- Skip if provider handled tools (CLI via MCP already saved intermediate messages)
- Validate tool calls JSON via round-trip: marshal → unmarshal → check (876-883)
- Strip AFV fence markers from content before saving
- Save to session DB

#### Step 9: Tool Execution (897-952)

**Only runs if runner is responsible** (not CLI providers that handle tools via MCP):

```go
for _, tc := range toolCalls {
    result := r.tools.Execute(ctx, &ai.ToolCall{...})

    // Wrap in AFV fences if origin/tool requires it
    if afv.ShouldFence(origin, tc.Name) {
        fence := fenceStore.Generate("tool_" + tc.Name + "_" + tc.ID)
        guide := afv.BuildToolResultGuide(fenceStore, tc.Name)
        fencedContent = guide.Format() + "\n" + fence.Wrap(content)
    }

    // Send tool result event (real-time)
    resultCh <- ai.StreamEvent{Type: EventTypeToolResult, ...}
}
// Save all tool results to session
// continue — let LLM respond to results
```

**Tool Registry execution** (`registry.go:145-230`):
1. MCP prefix handling: check `mcp__` prefixed name exists as-is (external MCP proxy), strip as fallback
2. Unknown tool → error with available tool list + correction hint
3. Origin check: `policy.IsDeniedForOrigin(origin, toolName)`
4. Approval check: `tool.RequiresApproval()` → `policy.RequestApproval(ctx, name, input)`
5. Execute tool

#### Step 10: Completion (973-992)

When no tool calls remain:
1. Record successful profile usage for tracking
2. **Schedule debounced memory extraction** (981): 5-second idle timer — each new message resets it. Extraction only runs when conversation pauses.
3. Send `EventTypeDone` to caller
4. Return from loop

---

## Lane-Based Concurrency System

### Lane Configuration

| Lane | Default Max | Hard Cap | Purpose |
|------|------------|----------|---------|
| `main` | 1 | — | User conversations (strictly serialized) |
| `events` | 0 (unlimited) | — | Scheduled/triggered tasks |
| `subagent` | 5 | 10 | Sub-agent goroutines |
| `nested` | 3 | 3 | Tool recursion/callbacks |
| `heartbeat` | 1 | — | Proactive heartbeat ticks |
| `comm` | 5 | — | Inter-agent communication |
| `dev` | 1 | — | Developer assistant |

### Execution Model

```
Enqueue(ctx, lane, task)
  → getLaneState(lane) — create if needed, apply defaults
  → append to Queue
  → drain(lane)
    → pump(state):
        while queue not empty AND active < MaxConcurrent:
          dequeue entry
          go func():
            entry.task.Task(ctx)
            resolve <- err
            pump(state)  // recursive: process next after completion
```

**Key behaviors:**
- `Enqueue()` — blocks until task completes (returns error)
- `EnqueueAsync()` — fire-and-forget wrapper around Enqueue
- `CancelActive(lane)` — cancels all active tasks via context cancellation
- `ClearLane(lane)` — removes all queued (not active) tasks
- Panic recovery: caught per-task, logged via `crashlog.LogPanic()`
- Events emitted: `task_enqueued`, `task_started`, `task_completed`, `task_cancelled`

---

## Real-Time Event Pipeline

### Event Flow

```
Runner (runLoop sends events to resultCh)
  → Agent Cmd (reads resultCh, calls sendFrame)
  → Agent Hub (readPump processes frames)
    → Frame router:
        "stream"/"res" → ChatContext.handleAgentResponse() → specific client
        "event"        → ChatContext.handleAgentEvent()    → ALL clients
        "req"          → handleRequest()                   → agent-initiated requests
  → ChatContext (internal/realtime/chat.go)
    → ClientHub.Broadcast()
      → Client.send channel (buffered 256)
        → writePump → WebSocket → Browser
```

### Event Types

| Type | Direction | Purpose |
|------|-----------|---------|
| `chat_stream` | Agent → Client | Text streaming token |
| `chat_complete` | Agent → Client | Response finished |
| `tool_start` | Agent → Client | Tool execution beginning |
| `tool_result` | Agent → Client | Tool execution result |
| `image` | Agent → Client | Image produced by tool |
| `thinking` | Agent → Client | Extended thinking content |
| `error` | Agent → Client | Error message |
| `approval_request` | Agent → Client | Tool needs user approval |
| `stream_status` | Agent → Client | Streaming state change |
| `chat_cancelled` | Agent → Client | Response cancelled |
| `chat_response` | Agent → Client | Full response (non-streaming) |
| `reminder_complete` | Agent → Client | Scheduled reminder fired |
| `dm_user_message` | Agent → Client | Owner DM message for web UI sync |

### Streaming Safety

- **Fence marker buffering:** 20-char holdback buffer prevents partial fence markers from reaching the client
- **UTF-8 rune boundary:** backs up to valid rune boundary before flushing (prevents split emoji)
- **Barge-in:** User sends while loading → cancel current context → sends new immediately

---

## Sub-Agent Spawning

### Orchestrator (`internal/agent/orchestrator/orchestrator.go`)

```go
type Orchestrator struct {
    agents        map[string]*SubAgent
    sessions      *session.Manager
    providers     []ai.Provider
    tools         ToolExecutor
    config        *config.Config
    recovery      *recovery.Manager
    maxConcurrent int  // default 5
    results       chan AgentResult
}
```

**Spawn lifecycle:**
1. Check concurrency limit (max 5 running, hard cap 10 via lane)
2. Generate unique ID: `agent-{unixnano}-{count}`
3. Create context with timeout if specified
4. Create sub-session: `subagent-{agentID}`
5. Persist to `pending_tasks` table (crash recovery)
6. Run full `Runner.Run()` in goroutine
7. Announce result via callback

**Sub-agents get:**
- Own session (isolated from parent)
- Own agentic loop (full Runner.Run())
- Optional model override
- Lane assignment (default: LaneSubagent)
- Crash recovery via `pending_tasks` table

**Recovery on restart:** `RecoverSubagents()` restores pending tasks from DB.

---

## Memory System

### Three-Tier Memory

| Layer | Purpose | Key Pattern |
|-------|---------|-------------|
| `tacit` | Long-term preferences, learned behaviors | `style/`, `preference/`, `workflow/` |
| `daily` | Day-specific facts (auto-keyed by date) | `daily/2026-02-25/` |
| `entity` | People, places, things | `entity/john_smith/`, `entity/project_x/` |

### Debounced Extraction (runner.go:1501-1514)

```go
func (r *Runner) scheduleMemoryExtraction(sessionID, userID string) {
    // Cancel existing timer for this session
    // Create new 5-second timer
    // On fire: extract memories from latest ~6 messages
}
```

- Each new message resets the idle timer
- Prevents background API calls from competing with chat bandwidth
- Uses cheapest available model
- Deduplication: skip if identical value already stored
- Reinforcement tracking: increment count on style duplicates
- Auto-synthesizes personality directives from repeated style observations

### Pre-Compaction Memory Flush (runner.go:1678-1735)

Triggers at 75% of compaction limit to ensure memories are captured before context is discarded. Synchronous threshold check, async LLM extraction.

---

## Objective Detection (runner.go:1358-1495)

Runs in background goroutine on every user message (>= 20 chars):

1. Classify user message relative to current active task
2. Classification actions: `set` (new objective), `update`, `clear`, `keep`
3. On `set`/`update`: pin active task to session via `SetActiveTask()`
4. On `set`/`clear`: clear work tasks
5. Prevents overlaps via `sync.Map` guard

The active task is then:
- Included in the dynamic prompt suffix every iteration
- Used by steering generators (objectiveTaskNudge, pendingTaskAction, taskProgress)
- Extracted and re-pinned during compaction

---

## Error Recovery & Resilience

### Provider Fallback Chain

```
Model override → Selector → First provider → Error message
```

On rate limit or auth error:
1. Record error for profile cooldown tracking
2. Mark model as failed in selector (exponential backoff)
3. Continue loop — selector picks next best model

### Context Overflow Recovery

```
Overflow detected → flush memory → generate summary → pin active task
  → progressive compaction: keep 10 → keep 3 → keep 1
  → re-inject recently accessed files
  → continue loop (always retry)
```

### Empty Response Guard (959-971)

If model returns nothing (0 text, 0 tool calls):
- Iteration 1: retry silently
- Iteration 2+: show error, return

### Max Iterations (987-992)

Hard cap at `config.MaxIterations` (default 100). Exhaustion sends error event.

---

## Prompt Sections (prompt.go)

### Static Sections

| Section | Content |
|---------|---------|
| `sectionIdentityAndPrime` | Identity declaration + PRIME DIRECTIVE ("JUST DO IT") + banned phrases |
| `sectionCapabilities` | What the agent can do (filesystem, shell, browser, apps, memory) |
| `sectionToolsDeclaration` | "Your ONLY tools are..." — prevents hallucinating tools from training |
| `sectionCommStyle` | Don't narrate routine calls, don't create deliverable files |
| `sectionSTRAPHeader` | STRAP pattern explanation + per-tool docs (only included tools) |
| `sectionMediaGuidance` | Image/audio handling guidance |
| `sectionMemoryGuidance` | When and how to store memories |
| `sectionBehavior` | General behavioral rules |
| `sectionAgentName` | Agent name anchoring |

### STRAP Tool Docs

Per-tool documentation injected only for tools present in the registry:
- `file` — read, write, edit, glob, grep actions
- `shell` — bash/process/session resources and actions
- `web` — fetch, search, navigate, click, type, screenshot actions
- `agent` — task, cron, memory, message, session, comm resources
- `screenshot` — capture, see actions
- `skill` — invoke, list, status actions

### Dynamic Suffix

Built per-iteration:
```go
type DynamicContext struct {
    ProviderID  string  // e.g. "anthropic"
    ModelName   string  // e.g. "claude-opus-4-6"
    ActiveTask  string  // pinned objective
    Summary     string  // compaction context
}
```

Output includes:
- Current date/time (exact moment)
- System context: model identity, hostname, OS
- Active task pin: "You are working on: {task}"
- Compaction summary: "Previous conversation context: {summary}"

---

## Key Design Decisions

1. **One Agent + Sub-Agent Goroutines:** NOT multi-agent — one persistent agent spawns goroutines for parallel work
2. **Serialized Main Lane (max=1):** User conversation is strictly sequential, preventing race conditions
3. **Streaming-First:** All events forwarded immediately via buffered channel, no batching
4. **Debounced Memory (5s idle):** Prevents API thrashing during active conversation
5. **AFV Pre-Send:** Fence verification BEFORE sending to LLM, quarantine on failure
6. **File Re-injection:** Post-compaction recovery of recently accessed files (maintains working context)
7. **Graduated Thresholds:** Warning → Error → AutoCompact prevents cascade failures
8. **Ephemeral Steering:** Mid-conversation guidance is never persisted, never shown to user
9. **Progressive Compaction:** Keep 10 → 3 → 1 ensures the agent can always continue
10. **Cumulative Summaries:** Previous summary compressed and prepended, not discarded
11. **Two-Tier Prompt Cache:** Static portion reused across iterations + 5-min Anthropic cache
12. **Tool JSON Validation:** Round-trip marshal/unmarshal prevents session poisoning from corrupted tool calls
