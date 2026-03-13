# Chat System — SME Reference

Comprehensive Subject Matter Expert document covering the full chat pipeline from
frontend WebSocket through agent runner and back, including all data structures,
streaming events, session management, lane concurrency, DB schema, codes system,
NeboLoop comm integration, and frontend rendering.

**Status:** Current (Rust implementation) | **Last updated:** 2026-03-12

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [WebSocket Handler](#2-websocket-handler)
3. [Unified Chat Dispatch](#3-unified-chat-dispatch)
4. [Runner (Agentic Loop)](#4-runner-agentic-loop)
5. [Session Management](#5-session-management)
6. [Key Parser](#6-key-parser)
7. [DB Schema](#7-db-schema)
8. [Lane System](#8-lane-system)
9. [Streaming Events](#9-streaming-events)
10. [REST Chat Endpoints](#10-rest-chat-endpoints)
11. [AppState](#11-appstate)
12. [Comm Integration](#12-comm-integration)
13. [Codes System](#13-codes-system)
14. [Frontend Components](#14-frontend-components)
15. [End-to-End Event Flow](#15-end-to-end-event-flow)
16. [Known Issues and Fixes](#16-known-issues-and-fixes)

---

## 1. Architecture Overview

```
Frontend (Svelte)                 Server (Axum)                   Agent (Runner)
================                 ==============                  ===============
WebSocketClient  ──WS──>  handle_client_ws()
  .send("chat",{...})           │
                                ├─ detect_code()?  ─> codes.rs (intercept)
                                │
                                ├─ dispatch_chat()
                                │     builds ChatConfig
                                │     calls run_chat()
                                │
                            run_chat()  (chat_dispatch.rs)
                                │  ├─ wraps in LaneTask
                                │  ├─ enqueues on LaneManager
                                │  └─ lane pump spawns task
                                │
                                │  RunRequest ──> Runner.run()
                                │                    │
                                │                    ├─ get_or_create session
                                │                    ├─ append user message
                                │                    ├─ spawn run_loop() task
                                │                    └─ return mpsc::Receiver<StreamEvent>
                                │
                                │  run_loop() (agentic loop, up to 100 iterations)
                                │     ├─ load & sanitize messages
                                │     ├─ sliding window + pruning
                                │     ├─ build system prompt (static + STRAP + dynamic)
                                │     ├─ select model via ModelSelector
                                │     ├─ acquire LLM permit (ConcurrencyController)
                                │     ├─ provider.stream() ──> EventReceiver
                                │     ├─ process stream events
                                │     ├─ save assistant message
                                │     ├─ execute tool calls in parallel
                                │     ├─ save tool results
                                │     └─ loop (if tool calls) or break
                                │
                            event loop in run_chat():
                                │  reads StreamEvents from rx
                                │  broadcasts each to ClientHub
                                │
ClientHub.broadcast() ──> all connected WS clients
  │
  ├─ "chat_stream"      (text chunks)
  ├─ "thinking"          (thinking blocks)
  ├─ "tool_start"        (tool invocation)
  ├─ "tool_result"       (tool output)
  ├─ "chat_error"        (errors)
  ├─ "usage"             (token counts)
  ├─ "approval_request"  (tool approval gate)
  ├─ "ask_request"       (interactive question)
  ├─ "chat_complete"     (terminal event)
  └─ "chat_cancelled"    (user cancel)
```

### Design Principle

**ONE entry point for all chat.** WebSocket, REST, and NeboLoop comm messages all
build a `ChatConfig` and call `run_chat()`. No separate code paths. This is
CODE_AUDITOR Rule 8.1 compliant — no competing pathways.

---

## 2. WebSocket Handler

**File:** `crates/server/src/handlers/ws.rs`

### Data Structures

```rust
pub struct ClientHub {
    tx: broadcast::Sender<HubEvent>,  // capacity: 256
}

pub struct HubEvent {
    pub event_type: String,
    pub payload: serde_json::Value,
}
```

### Three WebSocket Endpoints

| Endpoint | Handler | Purpose |
|----------|---------|---------|
| `GET /ws` | `client_ws_handler` | Main client (frontend) WebSocket |
| `GET /api/v1/agent/ws` | `agent_ws_handler` | Agent-to-server communication (forwards events to hub) |
| `GET /ws/extension` | `extension_ws_handler` | Chrome extension bridge (native messaging relay) |

### Client WS Message Types (Inbound)

| Type | Fields | Behavior |
|------|--------|----------|
| `"chat"` | `session_id`, `prompt`, `system`, `user_id`, `channel`, `role_id` | Dispatches to `dispatch_chat()` |
| `"cancel"` | `session_id` | Cancels active run via CancellationToken |
| `"auth"` / `"connect"` | optional `token` | Responds with `auth_ok` |
| `"ping"` | — | Responds with `pong` |
| `"session_reset"` | `session_id` | Clears session messages and counters |
| `"check_stream"` | `session_id` | Returns `stream_status` (running/idle) |
| `"approval_response"` | `request_id`, `approved` | Resolves pending tool approval oneshot |
| `"ask_response"` | `request_id`, `value` | Resolves pending ask request oneshot |
| `"request_introduction"` | `session_id` | Stub: sends `chat_complete` immediately |

### Connection Lifecycle

1. Client connects to `/ws`
2. Server sends `{"type": "connected", "version": "..."}`
3. Client sends `{"type": "auth", "data": {"token": "..."}}` or `{"type": "connect"}`
4. Server replies `{"type": "auth_ok"}`
5. Bidirectional: server broadcasts HubEvents, client sends chat/cancel/etc.
6. On close: cleanup, log disconnect

### Extension Bridge WS

- First message is `{"type": "hello", "browser": "chrome"}`
- Server registers connection via `bridge.connect(browser)`
- Forwards `execute_tool` requests to extension, receives `tool_response` results
- Split into two async tasks (send + recv), select! for completion

### ActiveRuns

```rust
pub type ActiveRuns = Arc<Mutex<HashMap<String, CancellationToken>>>;
```
Tracks which session_ids have active agent runs. Used by `cancel` WS message to
cooperatively stop the agentic loop.

### dispatch_chat()

Thin wrapper that extracts fields from WS JSON, intercepts marketplace codes,
builds `ChatConfig`, and calls `run_chat()`:

```rust
async fn dispatch_chat(state: &AppState, msg: &serde_json::Value, active_runs: ActiveRuns) {
    // 1. Extract session_id, prompt, system, user_id, channel, role_id from data
    // 2. Intercept marketplace codes (NEBO/SKIL/WORK/ROLE/LOOP-XXXX-XXXX)
    // 3. Reject empty prompts
    // 4. Build session_key: if role_id set, use build_role_session_key()
    // 5. Build ChatConfig with lane=MAIN, origin=User
    // 6. Call run_chat(state, config, Some(active_runs))
}
```

---

## 3. Unified Chat Dispatch

**File:** `crates/server/src/chat_dispatch.rs`

### Data Structures

```rust
pub struct ChatConfig {
    pub session_key: String,      // hierarchical session key
    pub prompt: String,           // user message text
    pub system: String,           // custom system prompt (empty = modular default)
    pub user_id: String,          // owner for scoping
    pub channel: String,          // "web", "neboloop", etc.
    pub origin: Origin,           // Origin::User, Origin::Comm, etc.
    pub role_id: String,          // role isolation (empty = main agent)
    pub cancel_token: CancellationToken,
    pub lane: String,             // which lane to enqueue on
    pub comm_reply: Option<CommReplyConfig>,  // reply-back config for NeboLoop
}

pub struct CommReplyConfig {
    pub topic: String,            // "chat" or "dm"
    pub conversation_id: String,  // NeboLoop conversation thread
}
```

### Three Entry Points (Same Function)

| Source | session_key | lane | origin | comm_reply |
|--------|-------------|------|--------|------------|
| WebSocket (companion) | companion chat UUID | `MAIN` | `User` | `None` |
| WebSocket (role) | `role:<roleId>:web` | `MAIN` | `User` | `None` |
| REST `/roles/:id/chat` | `role:<roleId>:web` | `MAIN` | `User` | `None` |
| NeboLoop comm | `neboloop:<type>:<convId>` | `COMM` | `Comm` | `Some(...)` |

### run_chat() Flow

1. Clone hub, runner, janus_usage from AppState
2. Track cancel token in ActiveRuns (if provided)
3. Broadcast `"chat_created"` event
4. Build `LaneTask` via `make_task()` containing:
   a. Construct `RunRequest` from ChatConfig fields
   b. Call `runner.run(req)` → get `mpsc::Receiver<StreamEvent>`
   c. Loop receiving StreamEvents, broadcasting each:
      - `Text` → `"chat_stream"` + accumulate `full_response`
      - `Thinking` → `"thinking"`
      - `ToolCall` → `"tool_start"`
      - `ToolResult` → `"tool_result"`
      - `Error` → `"chat_error"`
      - `Usage` → `"usage"`
      - `ApprovalRequest` → `"approval_request"`
      - `AskRequest` → `"ask_request"` (with optional widgets)
      - `RateLimit` → update `janus_usage` in-memory
      - `Done` → no-op
   d. If `comm_reply` configured: send `full_response` back via `comm_manager.send()`
   e. Always broadcast `"chat_complete"` at end
   f. On error: broadcast `"chat_error"` then `"chat_complete"`
   g. Clean up ActiveRuns entry
5. Enqueue task via `state.lanes.enqueue_async(&lane, lane_task)`

---

## 4. Runner (Agentic Loop)

**File:** `crates/agent/src/runner.rs`

### Constants

```rust
const DEFAULT_MAX_ITERATIONS: usize = 100;
const DEFAULT_CONTEXT_TOKEN_LIMIT: usize = 80_000;
const MAX_TRANSIENT_RETRIES: usize = 10;
const MAX_RETRYABLE_RETRIES: usize = 5;
const TOOL_EXECUTION_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_AUTO_CONTINUATIONS: usize = 3;
```

### Data Structures

```rust
pub struct RunRequest {
    pub session_key: String,
    pub prompt: String,
    pub system: String,
    pub model_override: String,
    pub user_id: String,
    pub skip_memory_extract: bool,
    pub origin: Origin,
    pub channel: String,
    pub force_skill: String,
    pub max_iterations: usize,
    pub cancel_token: CancellationToken,
    pub role_id: String,
}

struct RunState {
    prompt_overhead: usize,
    last_input_tokens: usize,
    thresholds: Option<ContextThresholds>,
}

pub struct Runner {
    sessions: SessionManager,
    providers: Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    tools: Arc<Registry>,
    store: Arc<Store>,
    selector: Arc<ModelSelector>,
    concurrency: Arc<ConcurrencyController>,
    hooks: Arc<napp::HookDispatcher>,
    mcp_context: Option<Arc<tokio::sync::Mutex<ToolContext>>>,
    role_registry: tools::RoleRegistry,
}
```

### Public Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `new()` | `(store, tools, providers, selector, concurrency, hooks, mcp_context, role_registry) -> Self` | Constructor |
| `run()` | `(&self, RunRequest) -> Result<mpsc::Receiver<StreamEvent>, ProviderError>` | Main entry: spawns agentic loop, returns event stream |
| `chat()` | `(&self, &str) -> Result<String, ProviderError>` | One-shot convenience (no tools, no session) |
| `reload_providers()` | `(&self, Vec<Arc<dyn Provider>>)` | Hot-swap provider list |
| `sessions()` | `(&self) -> &SessionManager` | Accessor |
| `store()` | `(&self) -> &Arc<Store>` | Accessor |
| `provider_count()` | `(&self) -> usize` | Non-blocking count |

### run() Method Flow

1. Validate providers exist (else error: "No AI providers configured")
2. Get or create session via `SessionManager`
3. Append user message to session (propagates error if fails)
4. Create `mpsc::channel(100)` for streaming events
5. Clone all shared state for spawned task
6. Resolve fuzzy model override (e.g., "sonnet" → "anthropic/claude-sonnet-4")
7. Derive channel from session key via keyparser
8. Set MCP context for CLI provider tool calls
9. `tokio::spawn` the agentic loop (`run_loop()`)
10. Return `rx` receiver to caller

### run_loop() — Agentic Loop

Per-iteration:

1. **Cancellation check** — bail if token cancelled
2. **Hook: `agent.should_continue`** — apps can dynamically stop
3. **Load messages** — `sessions.get_messages()`, then `sanitize_message_order()`
4. **Sliding window** — `pruning::apply_sliding_window()`, evicts old messages
5. **Rolling summary** — build if messages evicted
6. **Prompt overhead** — computed on first iteration (system tokens + tool schema tokens + 4000 buffer)
7. **Context thresholds** — `ContextThresholds::from_context_window()`
8. **Micro-compact** — shrink tool results if near threshold
9. **Tool filtering** — `tool_filter::filter_tools_with_context()` returns filtered tools + active contexts
10. **Steering** — generate steering messages + hook: `steering.generate`
11. **Build system prompt** — `static_system + STRAP section + tools_list + dynamic_suffix`
12. **Hook: `message.pre_send`** — apps can modify system prompt
13. **Model selection** — override or `selector.select()` + thinking mode
14. **Build ChatRequest** — messages + tools + system + model
15. **Acquire LLM permit** — `concurrency.acquire_llm_permit()`
16. **Provider selection** — match by ID or round-robin with fallback
17. **`provider.stream()`** — returns EventReceiver

#### Stream Processing

- `Text` — accumulate content, track block order, forward to tx
- `Thinking` — forward thinking block
- `ToolCall` — collect tool calls, track block order
- `Error` — capture stream_error (don't forward yet)
- `Usage` — track input tokens, forward
- `RateLimit` — report to concurrency controller
- `Done` / `ToolResult` / `ApprovalRequest` / `AskRequest` — handled at runner level only

#### Error Handling (3 layers)

1. **Transient errors** (connection reset, timeout, EOF) — retry up to 10 times, rotate providers
2. **Retryable errors** (rate_limit, billing, provider) — retry up to 5 times, rotate providers
3. **Non-retryable** — send error to user, break

#### After Stream

- **Hook: `message.post_receive`** — apps modify response text
- **Save assistant message** with tool_calls JSON + content block order metadata
- **Hook: `session.message_append`** — notification
- **CLI providers** — if `provider.handles_tools()`, skip runner tool loop
- **Tool execution** — parallel via `FuturesUnordered`:
  - Acquire tool permit per call
  - 300s timeout per tool
  - Results collected and forwarded as `StreamEventType::ToolResult`
  - Sidecar vision verification for browser screenshots
  - Results saved in deterministic order
- **Hook: `agent.turn`** — notification after tool execution
- If tool calls present: `continue` loop (LLM needs to respond to results)

#### Auto-Continuation

If no tool calls but `active_task` is set and response `looks_like_continuation_pause()`:
- Inject synthetic user message: `<system>Continue with your current objective...</system>`
- Up to `MAX_AUTO_CONTINUATIONS` (3) times

#### Post-Loop

- Debounced memory extraction (5s idle per session)
- Extract facts via LLM, store in memory DB

### Helper Functions

| Function | Description |
|----------|-------------|
| `looks_like_continuation_pause()` | Detects 25+ "should I continue?" patterns |
| `convert_messages()` | `ChatMessage` → `ai::Message` conversion |
| `sanitize_message_order()` | Reorders tool results after their assistant, strips orphans |
| `build_system_prompt()` | Combines custom system with DB context |
| `detect_objective()` | Background task classification via LLM |

---

## 5. Session Management

**File:** `crates/agent/src/session.rs`

### Data Structure

```rust
pub struct SessionManager {
    store: Arc<Store>,
    session_keys: Arc<RwLock<HashMap<String, String>>>,  // session_id -> session_key
}
```

The `session_keys` cache maps internal `session_id` (UUID) to the `session_key`
(the frontend-visible identifier like `"companion-default"`, `"role:researcher:web"`).
The `session_key` IS the `chat_id` used for message storage.

### Public Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `new()` | `(Arc<Store>) -> Self` | Constructor |
| `get_or_create()` | `(&self, session_key, user_id) -> Result<Session>` | Upsert session by key+scope |
| `resolve_session_key()` | `(&self, session_id) -> Result<String>` | Cache-first lookup: session_id → key |
| `get_messages()` | `(&self, session_id) -> Result<Vec<ChatMessage>>` | Load + sanitize messages |
| `append_message()` | `(&self, session_id, role, content, tool_calls, tool_results, metadata) -> Result<ChatMessage>` | Create message with token estimate |
| `get_summary()` / `update_summary()` | — | Rolling compaction summary |
| `get_active_task()` / `set_active_task()` / `clear_active_task()` | — | Pinned objective tracking |
| `get_work_tasks()` / `set_work_tasks()` | — | Work tasks JSON for steering |
| `reset()` | `(&self, session_id)` | Clear messages + counters |
| `list_sessions()` | `(&self, scope) -> Result<Vec<Session>>` | List by scope |
| `delete_session()` | `(&self, session_id)` | Delete session + messages |
| `store()` | `(&self) -> &Arc<Store>` | Accessor |

### Key Behaviors

- **Token estimation**: `chars / 4` heuristic for content + tool_calls + tool_results
- **Message sanitization**: `sanitize_messages()` removes orphaned tool results
  (tool messages whose `tool_call_id` doesn't match any assistant's tool calls)
- **Empty message rejection**: Skips messages where content, tool_calls, and tool_results are all empty/null
- **Chat ID resolution**: `resolve_chat_id()` uses session_key as chat_id; fallback `"chat-{session_id}"`

### Session-to-Chat Relationship

```
sessions table              chats table                chat_messages table
==============              ===========                ===================
id (UUID)          ──┐
name (session_key) ──┼──>   id = session_key    <──── chat_id (FK)
scope, scope_id      │      title
                     └──>   (auto-created by
                             create_chat_message_for_runner)
```

The `session_key` serves as BOTH:
- The session `name` (in sessions table)
- The `chat_id` (in chats + chat_messages tables)

The `create_chat_message_for_runner` function auto-creates the parent `chats` row
via `INSERT OR IGNORE` before inserting the message, satisfying the FK constraint.

---

## 6. Key Parser

**File:** `crates/agent/src/keyparser.rs`

### Session Key Formats

```
role:<roleId>:<channel>          — Role-scoped session
agent:<agentId>:<rest>           — Agent-scoped session
subagent:<parentId>:<childId>    — Sub-agent session
acp:<sessionId>                  — ACP session
<channel>:group:<id>             — Group chat
<channel>:channel:<id>           — Channel session
<channel>:dm:<id>                — Direct message
<parent>:thread:<threadId>       — Threaded conversation
<parent>:topic:<topicId>         — Topic-grouped conversation
```

### Parsed Structure

```rust
pub struct SessionKeyInfo {
    pub raw: String,
    pub channel: String,
    pub chat_type: String,      // "group", "channel", "dm"
    pub chat_id: String,
    pub agent_id: String,
    pub is_subagent: bool,
    pub is_acp: bool,
    pub is_thread: bool,
    pub is_topic: bool,
    pub parent_key: String,
    pub rest: String,
    pub role_id: String,
}
```

### Build Functions

| Function | Output Format |
|----------|---------------|
| `build_session_key(channel, type, id)` | `"discord:group:123"` |
| `build_agent_session_key(agent_id, name)` | `"agent:bot1:main"` |
| `build_subagent_session_key(parent, child)` | `"subagent:parent:child"` |
| `build_thread_session_key(parent, thread)` | `"discord:group:123:thread:t1"` |
| `build_topic_session_key(parent, topic)` | `"slack:channel:abc:topic:t2"` |
| `build_role_session_key(role_id, channel)` | `"role:researcher:web"` |

### Predicate Functions

`is_subagent_key()`, `is_acp_key()`, `is_agent_key()`, `is_role_key()`

### Extraction Functions

`extract_agent_id()`, `extract_role_id()`, `resolve_thread_parent_key()`

---

## 7. DB Schema

### Sessions Table (migration 0010 + additions)

```sql
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    name TEXT,                         -- session_key (used as chat_id)
    scope TEXT DEFAULT 'global',       -- global, user, agent, channel
    scope_id TEXT,                     -- user/channel ID if scoped
    summary TEXT,                      -- rolling compaction summary
    token_count INTEGER DEFAULT 0,
    message_count INTEGER DEFAULT 0,
    last_compacted_at INTEGER,
    metadata TEXT,                     -- JSON
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    -- Added by later migrations:
    compaction_count INTEGER DEFAULT 0,              -- 0023
    memory_flush_at INTEGER,                         -- 0023
    memory_flush_compaction_count INTEGER,            -- 0023
    send_policy TEXT DEFAULT 'allow',                -- 0024
    model_override TEXT,                             -- 0024
    provider_override TEXT,                          -- 0024
    auth_profile_override TEXT,                      -- 0024
    auth_profile_override_source TEXT,               -- 0024
    verbose_level TEXT,                              -- 0024
    custom_label TEXT,                               -- 0024
    last_embedded_message_id INTEGER DEFAULT 0,      -- 0039
    active_task TEXT,                                -- 0040
    last_summarized_count INTEGER DEFAULT 0,         -- 0043
    work_tasks TEXT,                                 -- 0046
);
UNIQUE INDEX ON sessions(name, scope, scope_id);  -- Upsert target
```

### Chats Table (migration 0008)

```sql
CREATE TABLE chats (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL DEFAULT 'New Chat',
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    user_id TEXT                                    -- 0009: companion mode
);
INDEX idx_chats_updated_at ON chats(updated_at DESC);
UNIQUE ON user_id (for companion chat upsert)
```

### Chat Messages Table (migration 0008 + 0045 + 0048)

```sql
CREATE TABLE chat_messages (
    id TEXT PRIMARY KEY,
    chat_id TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'system', 'tool')),
    content TEXT NOT NULL,
    metadata TEXT,                    -- JSON: contentBlocks, toolCalls
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    day_marker TEXT,                  -- 0009: date string for day grouping
    tool_calls TEXT,                  -- 0045: JSON array of ToolCall
    tool_results TEXT,                -- 0045: JSON array of tool results
    token_estimate INTEGER,           -- 0045: chars/4 heuristic
    is_compacted INTEGER DEFAULT 0,   -- 0048: compaction flag
    FOREIGN KEY (chat_id) REFERENCES chats(id) ON DELETE CASCADE
);
```

**Critical:** The FK constraint means a `chats` row MUST exist before inserting messages.
`create_chat_message_for_runner()` handles this automatically with `INSERT OR IGNORE`.

### Rust Models

```rust
pub struct Session {
    pub id: String,
    pub name: Option<String>,          // = session_key = chat_id
    pub scope: Option<String>,
    pub scope_id: Option<String>,
    pub summary: Option<String>,
    pub token_count: Option<i64>,
    pub message_count: Option<i64>,
    // ... (see db/src/models.rs for all 20+ fields)
}

pub struct Chat {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub user_id: Option<String>,
}

pub struct ChatMessage {
    pub id: String,
    pub chat_id: String,
    pub role: String,                   // "user", "assistant", "system", "tool"
    pub content: String,
    pub metadata: Option<String>,       // JSON
    pub created_at: i64,
    pub day_marker: Option<String>,
    pub tool_calls: Option<String>,     // JSON array
    pub tool_results: Option<String>,   // JSON array
    pub token_estimate: Option<i64>,
}
```

### Key DB Query Methods

**Sessions** (`crates/db/src/queries/sessions.rs`):
- `get_or_create_scoped_session()` — upsert: `ON CONFLICT(name, scope, scope_id) DO UPDATE`
- `create_session()`, `get_session()`, `get_session_by_name()`, `get_session_by_scope()`
- `list_sessions()`, `list_sessions_by_scope()`
- `update_session_summary()`, `update_session_stats()`, `increment_session_message_count()`
- `reset_session()`, `delete_session()`
- `set_session_model_override()`, `set_session_auth_profile_override()`, `clear_session_overrides()`
- `get_session_active_task()`, `set_session_active_task()`, `clear_session_active_task()`
- `get_session_work_tasks()`, `set_session_work_tasks()`

**Chats** (`crates/db/src/queries/chats.rs`):
- `create_chat()`, `get_chat()`, `list_chats()`, `count_chats()`
- `create_chat_message_for_runner()` — auto-creates parent chat row, inserts message with all fields
- `create_chat_message()` — basic (REST endpoints)
- `get_chat_messages()`, `get_recent_chat_messages()`, `get_recent_chat_messages_with_tools()`
- `find_tool_output()` — search role='tool' messages for a specific tool_call_id
- `get_or_create_companion_chat()` — upsert by user_id
- `list_chat_days()` — GROUP BY day_marker
- `get_chat_messages_by_day()`
- `search_chat_messages()` — LIKE search on content

---

## 8. Lane System

**File:** `crates/agent/src/lanes.rs`

### Data Structures

```rust
pub struct LaneTask {
    pub id: String,              // "lane-nanosecond_timestamp"
    pub lane: String,
    pub description: String,
    pub task: Pin<Box<dyn Future<Output = Result<(), String>> + Send>>,
    pub enqueued_at: Instant,
    pub warn_after_ms: u64,      // default: 2000ms
    pub completion_tx: Option<oneshot::Sender<Result<(), String>>>,
}

pub struct LaneManager {
    lanes: HashMap<String, (Arc<Mutex<LaneState>>, Arc<Notify>)>,
    cancel: CancellationToken,
}
```

### Lane Configurations

| Lane | Constant | Max Concurrent | Rationale |
|------|----------|---------------|-----------|
| `main` | `lanes::MAIN` | 0 (unlimited) | Primary chat |
| `events` | `lanes::EVENTS` | 0 (unlimited) | Event-triggered |
| `subagent` | `lanes::SUBAGENT` | 0 (unlimited) | Sub-agent tasks |
| `nested` | `lanes::NESTED` | 0 (unlimited) | Nested calls |
| `heartbeat` | `lanes::HEARTBEAT` | 1 | Sequential proactive ticks |
| `comm` | `lanes::COMM` | 0 (unlimited) | NeboLoop messages |
| `dev` | `lanes::DEV` | 0 (unlimited) | Developer assistant |
| `desktop` | `lanes::DESKTOP` | 1 | One screen, one mouse |

`0 = unlimited` means the adaptive `ConcurrencyController` governs concurrency
globally based on machine resources and LLM rate limits.

### Methods

| Method | Description |
|--------|-------------|
| `new()` | Creates all 8 lanes |
| `start_pumps()` | Spawns per-lane pump tasks (Notify-driven) |
| `enqueue()` | Enqueue with completion handle (returns `oneshot::Receiver`) |
| `enqueue_async()` | Fire-and-forget enqueue |
| `status()` | Get (name, active, queued, max_concurrent) for all lanes |
| `shutdown()` | Cancel all pumps |

### Pump Mechanism

Each lane has a `Notify`-driven pump loop:
1. Wait for `notify.notified()` (or cancel)
2. Lock lane state, check capacity
3. Pop task from FIFO queue, increment `active` count
4. Spawn task, on completion: decrement active, re-notify pump

### make_task() Helper

```rust
pub fn make_task(lane: &str, description: impl Into<String>, future: impl Future<...>) -> LaneTask
```
Creates a LaneTask with auto-generated ID (`"lane-nanosecond"`), 2000ms warn threshold.

---

## 9. Streaming Events

**File:** `crates/ai/src/types.rs`

### StreamEventType Enum

```rust
pub enum StreamEventType {
    Text,             // Incremental text content
    ToolCall,         // Tool invocation from LLM
    ToolResult,       // Tool execution output (runner-generated)
    Error,            // Error during streaming
    Done,             // Stream complete
    Thinking,         // Extended thinking block
    Usage,            // Token usage stats
    RateLimit,        // Rate limit headers from provider
    ApprovalRequest,  // Tool needs user approval (runner-generated)
    AskRequest,       // Interactive question for user (runner-generated)
}
```

### StreamEvent Structure

```rust
pub struct StreamEvent {
    pub event_type: StreamEventType,
    pub text: String,
    pub tool_call: Option<ToolCall>,
    pub error: Option<String>,
    pub usage: Option<UsageInfo>,
    pub rate_limit: Option<RateLimitMeta>,
    pub widgets: Option<serde_json::Value>,  // AskRequest UI widgets
}
```

Factory methods: `StreamEvent::text()`, `thinking()`, `tool_call()`, `error()`,
`done()`, `usage()`, `rate_limit_info()`, `approval_request()`, `ask_request()`

### Supporting Types

```rust
pub struct ToolCall { pub id: String, pub name: String, pub input: serde_json::Value }
pub struct UsageInfo { pub input_tokens: i32, pub output_tokens: i32, pub cache_creation_input_tokens: i32, pub cache_read_input_tokens: i32 }
pub struct RateLimitMeta { remaining_requests, remaining_tokens, reset_after_secs, retry_after_secs, session_limit_tokens, session_remaining_tokens, session_reset_at, weekly_limit_tokens, weekly_remaining_tokens, weekly_reset_at }
pub struct ToolDefinition { pub name: String, pub description: String, pub input_schema: serde_json::Value }
pub struct Message { pub role: String, pub content: String, pub tool_calls: Option<Value>, pub tool_results: Option<Value>, pub images: Option<Vec<ImageContent>> }
pub struct ChatRequest { pub messages: Vec<Message>, pub tools: Vec<ToolDefinition>, pub max_tokens: i32, pub temperature: f64, pub system: String, pub static_system: String, pub model: String, pub enable_thinking: bool }
```

### Provider Trait

```rust
pub trait Provider: Send + Sync {
    fn id(&self) -> &str;
    fn profile_id(&self) -> &str { "" }
    fn handles_tools(&self) -> bool { false }
    async fn stream(&self, req: &ChatRequest) -> Result<EventReceiver, ProviderError>;
}
```

### ProviderError

```rust
pub enum ProviderError {
    Api { code: String, message: String, retryable: bool },
    ContextOverflow,
    RateLimit,
    Auth(String),
    Request(String),
    Stream(String),
}
```

Error classification: `is_context_overflow()`, `is_transient_error()`,
`is_role_ordering_error()`, `classify_error_reason()` → "rate_limit", "auth",
"billing", "timeout", "provider", "other"

---

## 10. REST Chat Endpoints

**File:** `crates/server/src/handlers/chat.rs`

| Method | Path | Handler | Description |
|--------|------|---------|-------------|
| GET | `/api/v1/chats` | `list_chats` | Paginated chat list (limit/offset) |
| POST | `/api/v1/chats` | `create_chat` | Create new chat with title |
| GET | `/api/v1/chats/companion` | `get_companion_chat` | Get/create companion chat + messages |
| GET | `/api/v1/chats/search` | `search_messages` | LIKE search on messages |
| GET | `/api/v1/chats/days` | `list_chat_days` | Day grouping for history |
| POST | `/api/v1/chats/message` | `send_message` | Create message (chatId, content, role) |
| GET | `/api/v1/chats/history/:day` | `get_chat_history_by_day` | Messages for specific day |
| GET | `/api/v1/chats/:id` | `get_chat` | Get single chat |
| PUT | `/api/v1/chats/:id` | `update_chat` | Update chat title |
| DELETE | `/api/v1/chats/:id` | `delete_chat` | Delete chat + messages |
| GET | `/api/v1/chats/:id/messages` | `get_chat_messages` | All messages for chat |
| GET | `/api/v1/chats/:chat_id/tool-output/:tool_call_id` | `get_tool_output` | Lazy-fetch single tool output |

### Companion Chat

- Uses `COMPANION_USER_ID = "companion-default"` as stable user_id
- `get_or_create_companion_chat()` upserts by user_id
- Returns chat + recent messages (default 20) + total count
- Messages include reconstructed metadata via `build_message_metadata()`

### Metadata Reconstruction (`build_message_metadata()`)

Two-phase process:
1. **Phase 1**: Collect tool result statuses (error/success) from role='tool' messages
2. **Phase 2**: For each assistant message:
   - **Case 1**: Old metadata already has `toolCalls` — strip `output` field, done
   - **Case 2**: Metadata has persisted `contentBlocks` — build toolCalls from column, use persisted block order
   - **Case 3**: No metadata — build everything, default text-then-tools order

Tool outputs are NOT included in list responses (lazy-loaded via `get_tool_output`).

---

## 11. AppState

**File:** `crates/server/src/state.rs`

### Chat-Relevant Fields

```rust
pub struct AppState {
    pub hub: Arc<ClientHub>,                    // WebSocket broadcast hub
    pub runner: Arc<Runner>,                    // Agent runner (sessions, providers, tools)
    pub tools: Arc<Registry>,                   // Tool registry
    pub lanes: Arc<LaneManager>,                // Per-lane task queuing
    pub comm_manager: Arc<PluginManager>,        // NeboLoop comm plugin
    pub approval_channels: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,
    pub ask_channels: Arc<Mutex<HashMap<String, oneshot::Sender<String>>>>,
    pub extension_bridge: Arc<browser::ExtensionBridge>,
    pub hooks: Arc<napp::HookDispatcher>,
    pub mcp_context: Arc<tokio::sync::Mutex<tools::ToolContext>>,
    pub event_bus: tools::EventBus,
    pub event_dispatcher: Arc<workflow::events::EventDispatcher>,
    pub role_registry: tools::RoleRegistry,
    pub janus_usage: Arc<tokio::sync::RwLock<Option<JanusUsage>>>,
    pub store: Arc<Store>,
    pub config: Config,
    // ... other non-chat fields
}

pub struct JanusUsage {
    pub session_limit_tokens: u64,
    pub session_remaining_tokens: u64,
    pub session_reset_at: String,
    pub weekly_limit_tokens: u64,
    pub weekly_remaining_tokens: u64,
    pub weekly_reset_at: String,
}
```

### Approval/Ask Flow

1. Runner creates a `oneshot::channel()` for an approval or ask request
2. Sender stored in `approval_channels` or `ask_channels` (keyed by request_id)
3. StreamEvent::ApprovalRequest / AskRequest sent to frontend via hub
4. Frontend shows modal/form, user responds
5. WS message `"approval_response"` / `"ask_response"` arrives
6. WS handler looks up and resolves the oneshot sender
7. Runner receives the response and continues

---

## 12. Comm Integration

**File:** `crates/server/src/lib.rs`

### Message Flow: NeboLoop → Agent

```
NeboLoop Gateway ──WS──> NeboLoopPlugin ──> PluginManager.message_handler
                                                      |
                                              handle_comm_message()
                                                      |
                            ┌─── topic == "installs" ──> napp registry
                            ├─── topic == "chat"/"dm" ──> chat pipeline
                            └─── other ──> event bus + hub broadcast
```

### handle_comm_message()

1. **Install events** (`topic == "installs"`): Route to napp registry
2. **Chat/DM** (`topic == "chat"` or `"dm"`):
   - Extract text from content (JSON `.text` field or plain text)
   - Build session key: `"neboloop:chat:<conversation_id>"` or `"neboloop:dm:<conversation_id>"`
   - Build `ChatConfig` with:
     - `origin: Origin::Comm`
     - `lane: lanes::COMM`
     - `comm_reply: Some(CommReplyConfig { topic, conversation_id })`
   - Call `run_chat(&state, config, None)`
   - Emit into event bus for role triggers
3. **Other topics**: Emit into event bus + broadcast to frontend as `"comm_message"`

### Reply Path

When `comm_reply` is set in `ChatConfig`, `run_chat()` accumulates `full_response`
and after completion sends it back via `comm_manager.send()` as a `CommMessage`.

---

## 13. Codes System

**File:** `crates/server/src/codes.rs`

### Code Format

```
PREFIX-XXXX-XXXX
```
Where PREFIX is NEBO/SKIL/WORK/ROLE/LOOP and XXXX is 4 Crockford Base32 characters.

### Code Types

```rust
pub enum CodeType { Nebo, Skill, Work, Role, Loop }
```

### Detection

`detect_code(&prompt)` — checks if prompt is exactly a code (trimmed, case-insensitive).
Returns `Option<(CodeType, &str)>`.

### Interception Point

In `dispatch_chat()` (ws.rs), before the prompt reaches the agent:
```rust
if let Some((code_type, code)) = crate::codes::detect_code(&prompt) {
    crate::codes::handle_code(state, code_type, code, &session_id).await;
    return;
}
```

### Handler Flow

1. Broadcast `"code_processing"` with status message
2. Dispatch to per-type handler:
   - **NEBO**: `redeem_nebo_code()` → store bot_id + token → activate NeboLoop
   - **SKILL**: `install_skill()` → persist to filesystem → reload skill loader → cascade deps
   - **WORK**: `install_workflow()` → persist to DB + filesystem → cascade deps
   - **ROLE**: `install_role()` → persist to DB + filesystem → auto-activate → cascade deps
   - **LOOP**: `join_loop()` → register membership
3. Broadcast `"code_result"` with success/error + artifact_name + checkout_url
4. Always broadcast `"chat_complete"` (resets frontend loading state)

### Payment Support

If API returns `status == "payment_required"`, the result includes `checkout_url`
for Stripe checkout redirect.

### REST Endpoint

`POST /api/v1/codes` — submit a code via REST (alternative to chat interception).
Body: `{"code": "SKIL-RFBM-XCYT"}`

---

## 14. Frontend Components

### WebSocket Client (`app/src/lib/websocket/client.ts`)

Singleton `WebSocketClient` class:

```typescript
class WebSocketClient {
    private ws: WebSocket | null;
    private listeners: Map<string, Set<MessageHandler>>;
    private statusListeners: Set<(status: ConnectionStatus) => void>;
    private messageQueue: string[];  // queued while disconnected
    private reconnectAttempts: number;
    private authToken: string | null;
}
```

**Connection flow**:
1. Create WebSocket to `ws://localhost:PORT/ws`
2. On open: send `{"type": "auth", "data": {"token": "..."}}` or `{"type": "connect"}`
3. Wait for `auth_ok` → set status "connected", flush queue
4. Auto-reconnect on disconnect (exponential backoff: 2s, 4s, 8s... max 30s)

**Message format**: `{"type": "...", "data": {...}, "timestamp": "..."}`

### Chat.svelte (`app/src/lib/components/chat/Chat.svelte`)

Multi-mode component supporting three modes:

```typescript
interface ChatMode {
    type: 'companion' | 'channel' | 'role';
    channelId?: string;
    channelName?: string;
    loopName?: string;
    roleId?: string;
    roleName?: string;
}
```

**Internal message model**:
```typescript
interface Message {
    id: string;
    role: 'user' | 'assistant' | 'system';
    content: string;
    contentHtml?: string;
    timestamp: Date;
    toolCalls?: ToolCall[];
    streaming?: boolean;
    thinking?: string;
    contentBlocks?: ContentBlock[];
    senderName?: string;  // channel mode
}

interface ContentBlock {
    type: 'text' | 'tool' | 'image' | 'ask';
    text?: string;
    toolCallIndex?: number;
    imageData?: string;
    askRequestId?: string;
    askPrompt?: string;
    askWidgets?: Array<{type, label?, options?, default?}>;
    askResponse?: string;
}
```

**WebSocket event subscriptions (companion/role)**:
- `chat_stream` — append text to streaming message
- `chat_complete` — finalize message, drain queue, extract memories
- `chat_response` — single complete response
- `tool_start` — add running tool card
- `tool_result` — update tool card status
- `image` — inline image block
- `thinking` — thinking block content
- `error` — error display
- `approval_request` — show ApprovalModal
- `stream_status` — idle/running check
- `chat_cancelled` — cancel loading state
- `ask_request` — interactive question with widgets
- `code_processing` — marketplace code status
- `code_result` — code install result
- `dep_installed` / `dep_cascade_complete` — dependency installation

**Sending a message (companion/role)**:
```typescript
ws.send('chat', {
    session_id: chatId,
    prompt: text,
    user_id: '',
    channel: 'web',
    role_id: mode.roleId || ''  // role isolation
});
```

**Features**:
- Virtual scroll (top-truncation, 20-message window, load-more)
- Message grouping by role + tool presence
- Draft persistence in localStorage
- Message queue during loading (queued messages shown as pills)
- Stream staleness detection (10min timeout)
- Voice: TTS output, full-duplex voice sessions, wake word
- File drag-and-drop (inserts paths into input)
- Code processing UI (marketplace code status messages)

### ChatInput.svelte

- Autoresizing textarea (max 200px)
- Enter to send, Shift+Enter for newline
- Up arrow recalls last queued message
- File attachment via native dialog or HTML input
- Voice conversation toggle (full-duplex waveform visualizer)
- Send/Stop button (switches based on isLoading)
- New session button
- Queued message pills with cancel

### Frontend API (`app/src/lib/api/nebo.ts`)

| Function | Endpoint |
|----------|----------|
| `listChats(params)` | `GET /api/v1/chats` |
| `createChat(req)` | `POST /api/v1/chats` |
| `getCompanionChat()` | `GET /api/v1/chats/companion` |
| `getToolOutput(chatId, toolCallId)` | `GET /api/v1/chats/{chatId}/tool-output/{toolCallId}` |
| `listChatDays(params)` | `GET /api/v1/chats/days` |
| `getHistoryByDay(day)` | `GET /api/v1/chats/history/{day}` |
| `sendMessage(req)` | `POST /api/v1/chats/message` |
| `searchChatMessages(params)` | `GET /api/v1/chats/search` |
| `deleteChat(id)` | `DELETE /api/v1/chats/{id}` |
| `getChat(id)` | `GET /api/v1/chats/{id}` |
| `getChatMessages(chatId)` | `GET /api/v1/chats/{chatId}/messages` |
| `updateChat(req, id)` | `PUT /api/v1/chats/{id}` |
| `listAgentSessions()` | `GET /api/v1/agent/sessions` |
| `deleteAgentSession(id)` | `DELETE /api/v1/agent/sessions/{id}` |
| `getAgentSessionMessages(id)` | `GET /api/v1/agent/sessions/{id}/messages` |
| `chatWithRole(roleId, prompt)` | `POST /api/v1/roles/{roleId}/chat` |

---

## 15. End-to-End Event Flow

### User types "Hello" in companion chat:

1. **Frontend**: `Chat.svelte` calls `ws.send('chat', {session_id: chatId, prompt: 'Hello', channel: 'web'})`
2. **WS Handler**: `handle_client_ws()` receives `type: "chat"`, calls `dispatch_chat()`
3. **dispatch_chat()**: Checks `detect_code("Hello")` → None. Builds `ChatConfig`, calls `run_chat()`
4. **run_chat()**: Tracks in ActiveRuns, broadcasts `"chat_created"`, creates LaneTask, enqueues on `"main"` lane
5. **Lane pump**: Picks up task, spawns it
6. **Runner.run()**: Creates session, appends user message (auto-creates `chats` row), spawns `run_loop()`
7. **run_loop() iteration 1**:
   - Loads messages, applies sliding window
   - Builds system prompt (identity + STRAP + tools)
   - Selects model, acquires LLM permit
   - `provider.stream()` → receives Text events
   - Forwards each Text event to tx channel
8. **run_chat() event loop**: Receives Text events, broadcasts `"chat_stream"` to hub
9. **Frontend**: `handleChatStream()` appends text to streaming message, scrolls
10. **run_loop()**: Stream ends, saves assistant message, no tool calls → breaks
11. **run_chat()**: Broadcasts `"chat_complete"`, cleans up ActiveRuns
12. **Frontend**: `handleChatComplete()` finalizes message, sets `isLoading = false`

### User sends marketplace code "SKIL-RFBM-XCYT":

1. **Frontend**: Same WS send as above
2. **dispatch_chat()**: `detect_code("SKIL-RFBM-XCYT")` → `Some((Skill, "SKIL-RFBM-XCYT"))`
3. **codes::handle_code()**: Broadcasts `"code_processing"`, calls `handle_skill_code()`
4. **handle_skill_code()**: API call to NeboLoop, persists skill, reloads skill loader, cascades deps
5. Broadcasts `"code_result"` with success + artifact name
6. Broadcasts `"chat_complete"`
7. **Frontend**: Shows code processing/result UI, resets loading state

### User sends message to role "Researcher":

1. **Frontend**: `ws.send('chat', {session_id: 'role:35672fb4:web', prompt: 'Hello', role_id: '35672fb4', channel: 'web'})`
2. **dispatch_chat()**: role_id is set → `session_key = build_role_session_key(role_id, "web")` = `"role:35672fb4:web"`
3. **run_chat()**: Same as companion but `session_key = "role:35672fb4:web"`, `role_id = "35672fb4"`
4. **Runner.run()**: Creates session with `name = "role:35672fb4:web"`, auto-creates `chats` row with same ID
5. **run_loop()**: Resolves role from `role_registry`, injects ROLE.md into system prompt, uses role's declared tools
6. Rest of flow identical to companion chat

---

## 16. Known Issues and Fixes

### FK Constraint on chat_messages (Fixed 2026-03-12)

**Problem:** `chat_messages.chat_id` has a `FOREIGN KEY` referencing `chats.id`, and
`PRAGMA foreign_keys = ON` is set on every connection. Companion chat works because
`get_or_create_companion_chat()` creates the `chats` row. Role/channel sessions had
no equivalent, causing `FOREIGN KEY constraint failed` on `INSERT INTO chat_messages`.

**Root cause:** `runner.run()` only warned on `append_message` failure and continued,
leading to `run_loop()` finding zero messages and returning "No messages in session".

**Fix:**
1. `create_chat_message_for_runner()` now does `INSERT OR IGNORE INTO chats` before inserting the message
2. `runner.run()` now propagates `append_message` errors via `?` instead of just warning

### Session Key = Chat ID Coupling

The system uses `session_key` as both the session name AND the `chat_id` for message
storage. This coupling means:
- Changing a session key format requires migrating existing messages
- The session key must be a valid identifier (no special characters beyond what's already used)
- Frontend must know the exact session key format to load history (e.g., `role:<id>:web`)
