# Lane System Specification

The lane system provides supervisor-pattern concurrency for the Nebo agent. Lanes are work queues that organize different types of work with configurable concurrency limits.

## Lane Types

| Lane | Purpose | Default Concurrency | Description |
|------|---------|---------------------|-------------|
| `main` | User conversations | 1 (serialized) | Primary user interactions via chat, CLI, or channels |
| `events` | Scheduled tasks | 2 | Event-driven tasks including cron jobs and triggers |
| `subagent` | Sub-agent parallel work | 0 (unlimited) | Spawned sub-agents for concurrent task execution |
| `nested` | Nested tool calls | 3 (hard cap) | Recursive tool executions within a single task |
| `heartbeat` | Proactive heartbeat ticks | 1 (sequential) | Periodic heartbeat checks for proactive behavior |

## Architecture

```
                    ┌─────────────────────────────────────────────────────────┐
                    │                      Nebo Agent                         │
                    │                                                         │
                    │  ┌───────────────────────────────────────────────────┐  │
                    │  │  Lane Manager                                     │  │
                    │  │                                                   │  │
                    │  │    main      ──── [1] ─── User Conversations     │  │
                    │  │    events    ──── [2] ─── Scheduled Tasks        │  │
                    │  │    subagent  ──── [∞] ─── Parallel Sub-agents    │  │
                    │  │    nested    ──── [3] ─── Tool Recursion         │  │
                    │  │    heartbeat ──── [1] ─── Proactive Heartbeats   │  │
                    │  │                                                   │  │
                    │  └───────────────────────────────────────────────────┘  │
                    │                                                         │
                    │  Single Runner instance shared across all lanes         │
                    │  All lanes have full memory access                      │
                    └─────────────────────────────────────────────────────────┘
```

## Configuration

Lane concurrency can be configured in `~/.nebo/config.yaml`:

```yaml
lanes:
  main: 1        # User conversations (serialized by default)
  events: 2      # Scheduled/triggered tasks
  subagent: 0    # 0 = unlimited sub-agents
  nested: 3      # Hard cap on nested tool calls
  heartbeat: 1   # Sequential heartbeat processing
```

**Concurrency Values:**
- `0` = unlimited (no concurrency limit)
- Any positive integer = maximum concurrent tasks
- Hard caps (like `nested: 3`) cannot be exceeded even if configured higher

## Routing Rules

### Request Type → Lane Mapping

| Request Type | Detected By | Target Lane |
|--------------|-------------|-------------|
| User message | Default | `main` |
| Heartbeat | Session key prefix `heartbeat-` | `heartbeat` |
| Cron job | Session key prefix `cron-` | `events` |
| Sub-agent spawn | Orchestrator spawn | `subagent` |
| Nested tool call | Tool recursion | `nested` |

### Routing Code Example

```go
isHeartbeat := strings.HasPrefix(sessionKey, "heartbeat-")
isCronJob := strings.HasPrefix(sessionKey, "cron-")

lane := agenthub.LaneMain
if isHeartbeat {
    lane = agenthub.LaneHeartbeat
} else if isCronJob {
    lane = agenthub.LaneEvents
}
```

## Concurrency Semantics

### Lane Independence

Each lane operates independently:
- Tasks in different lanes can run concurrently
- A busy `main` lane does NOT block `heartbeat` or `events` lanes
- This enables proactive agent behavior during user conversations

### Within-Lane Serialization

Within a single lane, tasks respect concurrency limits:
- `main` lane with concurrency 1 serializes all user conversations
- `heartbeat` lane with concurrency 1 ensures sequential heartbeat processing
- Higher concurrency allows parallel execution within the lane

### Queue Behavior

1. **Enqueue**: Tasks are added to the lane's queue
2. **Drain**: The lane manager processes tasks up to the concurrency limit
3. **Completion**: When a task completes, the next queued task starts
4. **Blocking**: `Enqueue()` blocks until the task completes
5. **Async**: `EnqueueAsync()` returns immediately, task runs in background

## Memory Access

**All lanes have full memory access:**

| Lane | Memory Access | Can Store | Can Recall |
|------|---------------|-----------|------------|
| main | Full | Yes | Yes |
| events | Full | Yes | Yes |
| subagent | Full | Yes | Yes |
| nested | Full | Yes | Yes |
| heartbeat | Full | Yes | Yes |

This ensures:
- Scheduled tasks can access user preferences
- Heartbeats can recall context for proactive actions
- Sub-agents can store facts they discover

## Key Files

| File | Responsibility |
|------|----------------|
| `internal/agenthub/lane.go` | Lane constants, LaneManager, queue implementation |
| `internal/agent/config/config.go` | LaneConfig struct for YAML configuration |
| `cmd/nebo/agent.go` | Request routing to lanes |
| `internal/agent/orchestrator/orchestrator.go` | Sub-agent lane management |

## Lane Constants

```go
const (
    LaneMain      = "main"      // User conversations
    LaneEvents    = "events"    // Scheduled/triggered tasks
    LaneSubagent  = "subagent"  // Sub-agent operations
    LaneNested    = "nested"    // Nested tool calls
    LaneHeartbeat = "heartbeat" // Proactive heartbeat ticks
)

var DefaultLaneConcurrency = map[string]int{
    LaneMain:      1,  // Serialized user conversations
    LaneEvents:    2,  // Parallel scheduled tasks
    LaneSubagent:  0,  // Unlimited sub-agents
    LaneNested:    3,  // Hard cap on tool recursion
    LaneHeartbeat: 1,  // Sequential heartbeats
}
```

## Design Rationale

### Why Separate Heartbeat Lane?

Previously, heartbeats ran in the `cron` lane and were **skipped entirely** when the main lane was busy. This prevented proactive agent behavior during user conversations.

With a dedicated `heartbeat` lane:
- Heartbeats run independently of user conversations
- The agent can proactively check tasks while chatting
- No more "skipping heartbeat - main lane busy" messages

### Why Rename Cron → Events?

The original "cron" name was a misnomer:
- The lane handles more than just time-based cron jobs
- It processes triggers, webhooks, and other event-driven tasks
- "Events" better describes the lane's purpose

### Why Hard Cap on Nested?

The `nested` lane has a hard cap (default: 3) to prevent:
- Runaway recursive tool calls
- Resource exhaustion from deep call stacks
- Infinite loops in tool execution

Even if configured higher, the hard cap is enforced.
