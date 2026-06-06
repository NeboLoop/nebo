# Chat System — SME Reference

Comprehensive Subject Matter Expert document covering the full chat pipeline from
frontend WebSocket through agent runner and back, including all data structures,
streaming events, session management, lane concurrency, DB schema, codes system,
NeboAI comm integration, and frontend rendering.

**Status:** Current (Rust implementation) | **Last updated:** 2026-06-05

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
15. [Per-Entity Config System](#15-per-entity-config-system)
16. [End-to-End Event Flow](#16-end-to-end-event-flow)
17. [Slash Commands](#17-slash-commands)
18. [File Attachment & Drag-and-Drop](#18-file-attachment--drag-and-drop)
19. [Known Issues and Fixes](#19-known-issues-and-fixes)
20. [Ask Widget System](#20-ask-widget-system)
21. [RunRegistry](#21-runregistry)
22. [ConcurrencyController](#22-concurrencycontroller)
23. [Orchestrator (Sub-Agents)](#23-orchestrator-sub-agents)
24. [@Mention Routing](#24-mention-routing)
25. [Redaction System](#25-redaction-system)
26. [Ghost Text / Inline Completion](#26-ghost-text--inline-completion)
27. [Plan Mode Approval](#27-plan-mode-approval)
28. [Tool Scope Isolation](#28-tool-scope-isolation)
29. [Unified Chat Controller (Frontend)](#29-unified-chat-controller-frontend)
30. [App Sidecar Restore](#30-app-sidecar-restore)

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
                                │     resolves entity_config (per-entity overrides)
                                │     calls run_chat()
                                │
                            run_chat()  (chat_dispatch.rs)
                                │  ├─ registers in RunRegistry (global visibility)
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
                                │  run_loop() (agentic loop, up to 200 iterations)
                                │     ├─ load & sanitize messages
                                │     ├─ sliding window + pruning
                                │     ├─ build system prompt (static + STRAP + dynamic + model identity)
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
                                │  broadcasts each to ClientHub (50ms text coalescing)
                                │  chat_stream sends content only (no HTML); chat_complete includes HTML
                                │
ClientHub.broadcast() ──> all connected WS clients
  │
  ├─ "chat_stream"      (text chunks, 50ms coalesced)
  ├─ "thinking"          (thinking blocks)
  ├─ "tool_start"        (tool invocation)
  ├─ "tool_result"       (tool output)
  ├─ "tool_summary"      (brief tool execution summary)
  ├─ "chat_error"        (errors)
  ├─ "usage"             (token counts + cache stats)
  ├─ "approval_request"  (tool approval gate)
  ├─ "ask_request"       (interactive question)
  ├─ "plan_approval"     (agent plan for user approval, see §27)
  ├─ "followup_suggestions" (chat continuation chips)
  ├─ "ghost_text"        (inline completion suggestion, see §26)
  ├─ "chat_complete"     (terminal event, includes HTML)
  ├─ "chat_cancelled"    (user cancel)
  │
  │  Additional lifecycle/status events (not from runner):
  ├─ "connected"         (WS handshake welcome)
  ├─ "chat_ack"          (message accepted)
  ├─ "chat_created"      (run started)
  ├─ "chat_title_updated" (auto-generated title)
  ├─ "quota_warning"     (Janus usage >80%)
  ├─ "stream_status"     (running/idle probe reply)
  ├─ "session_reset"     (session reset result)
  ├─ "session_compact"   (compact result)
  ├─ "code_processing"   (marketplace code handling / artifact install progress)
  ├─ "code_result"       (marketplace code outcome: success, message, artifact_name, payment_required, checkout_url, needsAuth)
  ├─ "dep_installed"     (dependency cascade step)
  ├─ "dep_cascade_complete" (dependency cascade done)
  ├─ "subagent_start"    (sub-agent spawned)
  ├─ "subagent_progress" (sub-agent update)
  ├─ "subagent_complete" (sub-agent done)
  ├─ "tool_quarantined"  (tool disabled at runtime)
  └─ "tool_error"        (tool registration error)
```

### Design Principle

**ONE entry point for all chat.** WebSocket, REST, and NeboAI comm messages all
build a `ChatConfig` and call `run_chat()`. No separate code paths.

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
| `GET /agent/ws` | `agent_ws_handler` | Agent-to-server communication (forwards events to hub) |
| `GET /ws/extension` | `extension_ws_handler` | Chrome extension bridge (native messaging relay) |

### Client WS Message Types (Inbound)

| Type | Fields | Behavior |
|------|--------|----------|
| `"chat"` | `session_id`, `prompt`, `system`, `user_id`, `channel`, `agent_id`, `message_id` | Dispatches to `dispatch_chat()` |
| `"cancel"` | `run_id`, `entity_id`, `session_id` | Cancels by run_id, entity_id, or session_id; if no match, cancels ALL runs |
| `"cancel_all"` | — | Emergency stop all active runs |
| `"auth"` / `"connect"` | optional `token` | Responds with `auth_ok` |
| `"ping"` | — | Responds with `pong` |
| `"session_reset"` | `session_id` | Rotates chat (creates new conversation), broadcasts new chat ID |
| `"session_compact"` | `session_id` | Spawns async summarization task (calls LLM, clears old messages) |
| `"approval_response"` | `request_id`, `approved` | Resolves pending tool approval oneshot |
| `"ask_response"` | `request_id`, `value` | Resolves pending ask request oneshot |
| `"plan_response"` | `request_id`, `approved` (bool) | Approve/reject agent plan (routes through `ask_channels`, see §27) |
| `"ghost_text"` | `partial_text`, `session_id`, `agent_id`, `request_id` | Request inline completion (see §26) |

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

### Stale Run Cleanup

A background task (spawned per-connection) polls every 60s and cancels runs in the
RunRegistry older than **600 seconds (10 minutes)**. This prevents stale entries from
accumulating if a run completes without proper cleanup.

### Message Idempotency

Chat messages support optional `message_id` for deduplication. When present, the WS
handler checks against an in-memory `HashSet<String>` (per-connection, max 1000 entries).
Duplicate `message_id` values are silently dropped. The set is cleared entirely when it
exceeds 1000 entries. This is **per-connection only** — reconnecting resets the dedup set.

### Image Extraction from Prompts

Before dispatch, `extract_images_from_prompt()` scans the prompt for whitespace-separated
file paths with image extensions (png, jpg, jpeg, gif, webp, bmp, tiff). Matching files
are read, base64-encoded into `ai::ImageContent` structs, and removed from the prompt text.
If the entire prompt consisted of image paths, the cleaned text defaults to `"What's in
this image?"`. The extracted images are passed via `ChatConfig.images` → `RunRequest.images`.

### dispatch_chat()

Thin wrapper that extracts fields from WS JSON, intercepts marketplace codes,
builds `ChatConfig`, and calls `run_chat()`. Also handles @mention routing (see §24):

```rust
async fn dispatch_chat(state: &AppState, msg: &serde_json::Value) {
    // 1. Extract session_id, prompt, system, user_id, channel, agent_id from data
    // 2. Intercept marketplace codes (NEBO/SKIL/WORK/AGNT/LOOP/PLUG-XXXX-XXXX)
    // 3. Reject empty prompts
    // 4. Redact sensitive slash command args: redact::redact_sensitive_args(&prompt) (see §25)
    // 5. Build session_key: if agent_id set, use build_agent_session_key();
    //    preserves client-provided session_id if it has correct agent prefix
    // 6. Resolve entity_config via resolve_for_chat() (per-entity overrides)
    // 7. Parse @mention tokens: parse_mention_tokens(&prompt, &agent_id)
    // 8. Build mention_context if mentions found (invisible system msg)
    // 9. Merge app context: data["context"] + mention_context combined into system message
    // 10. Parse tool_scope from data["scope"] (plan_mode hardcoded false — not yet client-settable)
    // 11. Build ChatConfig with lane=MAIN, origin=User, entity_config, mention_context,
    //     tool_scope, plan_mode
    // 12. Call run_chat(state, config) for primary agent
    // 13. Fork async chats for each mentioned agent via tokio::spawn
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
    pub channel: String,          // "web", "neboai", etc.
    pub origin: Origin,           // Origin::User, Origin::Comm, etc.
    pub agent_id: String,         // agent isolation (empty = main agent)
    pub cancel_token: CancellationToken,
    pub lane: String,             // which lane to enqueue on
    pub comm_reply: Option<CommReplyConfig>,  // reply-back config for NeboAI
    pub entity_config: Option<ResolvedEntityConfig>,  // per-entity overrides (see §15)
    pub images: Vec<ai::ImageContent>,  // base64-encoded image attachments
    pub entity_name: String,      // display name for RunRegistry
    pub origin_agent_id: Option<String>,  // for @mention routing (see §24)
    pub mention_context: Option<String>,  // invisible system msg for primary agent (see §24)
    pub tool_scope: Option<String>,       // restrict tools to named scope (see §28)
    pub plan_mode: bool,                  // present plan before executing tools (see §27)
}

pub struct CommReplyConfig {
    pub provider: String,         // "neboai" or future: "slack"
    pub topic: String,            // "chat" or "dm"
    pub conversation_id: String,  // NeboAI conversation thread
}
```

### Three Entry Points (Same Function)

| Source | session_key | lane | origin | comm_reply |
|--------|-------------|------|--------|------------|
| WebSocket (companion) | companion chat UUID | `MAIN` | `User` | `None` |
| WebSocket (agent) | `agent:<agentId>:web` | `MAIN` | `User` | `None` |
| REST `/agents/:id/chat` | `agent:<agentId>:web` | `MAIN` | `User` | `None` |
| NeboAI comm | `neboai:<type>:<convId>` | `COMM` | `Comm` | `Some(...)` |

### run_chat() Flow

1. Resolve agent display name from registry or DB
2. Register in global `RunRegistry` (for visibility/cancellation)
3. Broadcast `"chat_created"` event
4. Build `LaneTask` via `make_task()` containing:
   a. Construct `RunRequest` from ChatConfig fields
   b. Extract per-entity overrides from `entity_config` into RunRequest (permissions, resource_grants, model_preference, personality_snippet)
   c. Set `progress` counters (RunProgress with run_id, AtomicU32 for iterations/tools, current_tool Mutex)
   d. Call `runner.run(req)` → get `mpsc::Receiver<StreamEvent>`
   e. Loop receiving StreamEvents, broadcasting each:
      - `Text` → coalesced into 50ms batches, broadcast `"chat_stream"` with `content` only (no HTML; frontend renders MD-to-HTML client-side) + accumulate `full_response`
      - `Thinking` → `"thinking"` (immediate)
      - `ToolCall` → flush pending text, broadcast `"tool_start"`
      - `ToolResult` → `"tool_result"` (with error flag)
      - `Error` → `"chat_error"`
      - `Usage` → `"usage"` (includes `cache_read_input_tokens`, `cache_creation_input_tokens`)
      - `ApprovalRequest` → `"approval_request"`
      - `AskRequest` → `"ask_request"` (with optional widgets)
      - `RateLimit` → update `janus_usage` in-memory + broadcast `"quota_warning"` if text present (once per run)
      - `PlanApproval` → `"plan_approval"` (request_id, plan text, tool widgets; see §27)
      - `FollowupSuggestions` → `"followup_suggestions"` (session_id, suggestions array)
      - `ToolSummary` → `"tool_summary"` (session_id, summary text)
      - `Done` → no-op (handled by loop exit)
   f. If `comm_reply` configured: stream chunks during reception (500ms coalesced), then send final message via `send_to_channel()` (dedup: skips final if chunks were already streamed)
   g. Always broadcast `"chat_complete"` with rendered HTML at end
   h. On error: broadcast `"chat_error"` then `"chat_complete"`
5. Enqueue task via `state.lanes.enqueue_async(&lane, lane_task)`
6. Background: auto-generates chat title if default ("New Chat", etc.) via cheapest provider

### Helper Functions

- `md_to_html(md: &str) -> String` — converts markdown to HTML via pulldown-cmark (tables + strikethrough)
- `send_to_channel(provider, comm_manager, channel_providers, msg) -> Result` — routes CommMessage to correct provider (neboai fast path, or lookup in registry)
- `generate_chat_title_if_needed()` — spawned async, non-fatal on error, calls cheapest provider with transcript snippet

---

## 4. Runner (Agentic Loop)

**File:** `crates/agent/src/runner.rs`

### Constants

```rust
const DEFAULT_MAX_ITERATIONS: usize = 100;
const EXTENDED_MAX_ITERATIONS: usize = 200;     // with genuine progress
const DEFAULT_CONTEXT_TOKEN_LIMIT: usize = 80_000;
const MAX_TRANSIENT_RETRIES: usize = 10;
const MAX_RETRYABLE_RETRIES: usize = 5;
const TOOL_EXECUTION_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_AUTO_CONTINUATIONS_DEFAULT: usize = 5;
const MAX_AUTO_CONTINUATIONS_CEILING: usize = 50;
const MAX_OUTPUT_RECOVERY_ATTEMPTS: usize = 3;
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
    pub agent_id: String,
    // Per-entity overrides (from entity_config system, see §15)
    pub permissions: Option<HashMap<String, bool>>,      // tool category allow/deny
    pub resource_grants: Option<HashMap<String, String>>, // screen/browser access
    pub model_preference: Option<String>,                 // fuzzy model name
    pub personality_snippet: Option<String>,               // prepended to system prompt
    pub images: Vec<ai::ImageContent>,                    // base64-encoded image attachments
    pub allowed_paths: Vec<String>,                       // restrict file writes/shell (empty = unrestricted)
    pub presence_tracker: Option<Arc<PresenceTracker>>,
    pub proactive_inbox: Option<Arc<ProactiveInbox>>,
    pub min_iterations: usize,
    pub prompt_mode: PromptMode,
    pub progress: Option<RunProgress>,
}

pub struct RunProgress {
    pub run_id: String,
    pub iteration_count: Arc<AtomicU32>,
    pub tool_call_count: Arc<AtomicU32>,
    pub current_tool: Arc<Mutex<String>>,
}

struct RunState {
    prompt_overhead: usize,
    last_input_tokens: usize,
    thresholds: Option<ContextThresholds>,
    quota_warning: Option<String>,     // Janus >80% usage warning string
    quota_warning_sent: bool,          // fire once per run
}

pub struct Runner {
    sessions: SessionManager,
    providers: Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    tools: Arc<Registry>,
    store: Arc<Store>,
    selector: Arc<ModelSelector>,
    _steering: steering::Pipeline,
    concurrency: Arc<ConcurrencyController>,
    hooks: Arc<napp::HookDispatcher>,
    mcp_context: Option<Arc<tokio::sync::Mutex<ToolContext>>>,
    agent_registry: tools::AgentRegistry,
    skill_loader: Option<Arc<tools::skills::Loader>>,
    ask_channels: Option<AskChannels>,
}
```

### Public Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `new()` | `(store, tools, providers, selector, concurrency, hooks, mcp_context, agent_registry, skill_loader) -> Self` | Constructor |
| `run()` | `(&self, RunRequest) -> Result<mpsc::Receiver<StreamEvent>, ProviderError>` | Main entry: spawns agentic loop, returns event stream |
| `chat()` | `(&self, &str) -> Result<String, ProviderError>` | One-shot convenience (no tools, no session) |
| `reload_providers()` | `(&self, Vec<Arc<dyn Provider>>)` | Hot-swap provider list |
| `set_ask_channels()` | `(self, AskChannels) -> Self` | Builder method |
| `sessions()` | `(&self) -> &SessionManager` | Accessor |
| `store()` | `(&self) -> &Arc<Store>` | Accessor |
| `selector()` | `(&self) -> &ModelSelector` | Accessor |
| `providers()` | `(&self) -> Arc<RwLock<Vec<Arc<dyn Provider>>>>` | Clones Arc |
| `provider_count()` | `(&self) -> usize` | Non-blocking count |

### run() Method Flow

1. Validate providers exist (else error: "No AI providers configured")
2. Get or create session via `SessionManager`
3. Handle large input (>threshold) via sidecar summarization:
   - Saves full content to disk (`<data_dir>/large_inputs/`)
   - Calls cheapest provider to summarize
   - Replaces prompt with summary + file reference in metadata
4. Append user message to session (propagates error if fails)
5. Create `mpsc::channel(100)` for streaming events
6. Resolve fuzzy model override (e.g., "sonnet" → "anthropic/claude-sonnet-4")
7. Derive channel from session key via keyparser
8. Set MCP context for CLI provider tool calls
9. `tokio::spawn` the agentic loop (`run_loop()`)
10. Return `rx` receiver to caller

### run_loop() — Agentic Loop

Per-iteration:

1. **Cancellation check** — bail if token cancelled
2. **Hook: `agent.should_continue`** — plugins can dynamically stop
3. **Load messages** — `sessions.get_messages()`, then `sanitize_message_order()`
4. **Sliding window** — `pruning::apply_sliding_window()`, evicts old messages
5. **Rolling summary** — build LLM summary if messages evicted (first eviction only)
6. **Prompt overhead** — computed on first iteration (system tokens + tool schema tokens + 4000 buffer)
7. **Context thresholds** — `ContextThresholds::from_context_window()`
8. **Micro-compact** — shrink tool results if near threshold
9. **Time-based micro-compact** — clear stale results after 5+ min inactivity
10. **Tool filtering** — `tool_filter::filter_tools_with_context()` returns tools + active contexts
11. **Steering** — 13 generators (see §4.2) + hook: `steering.generate`
12. **Build system prompt** — `static_system + STRAP section + tools_list + dynamic_suffix + model_identity`
    - **Model identity branding**: Janus/nebo-* models get "you are Nebo, NOT Claude/GPT/Gemini" directive
13. **Hook: `message.pre_send`** — plugins can modify system prompt
14. **Model selection** — override or `selector.select()` + thinking mode
15. **Build ChatRequest** — messages + tools + system + model
16. **Acquire LLM permit** — `concurrency.acquire_llm_permit()`
17. **Provider selection** — match by ID or round-robin with fallback
18. **`provider.stream()`** — returns EventReceiver

#### Stream Processing

- `Text` — accumulate content, track block order, forward to tx
- `Thinking` — forward thinking block
- `ToolCall` — collect tool calls, track block order
- `Error` — capture stream_error (don't forward yet)
- `Usage` — track input tokens, forward
- `RateLimit` — report to concurrency controller

#### Error Handling (3 layers)

1. **Transient errors** (connection reset, timeout, EOF) — retry up to 10 times, rotate providers
2. **Retryable errors** (rate_limit, billing, provider) — retry up to 5 times, rotate providers
3. **Non-retryable** — send error to user, break

#### After Stream

- **Hook: `message.post_receive`** — plugins modify response text
- **Save assistant message** with tool_calls JSON + content block order metadata
- **Hook: `session.message_append`** — notification
- **CLI providers** — if `provider.handles_tools()`, skip runner tool loop
- **Tool execution** — parallel via `FuturesUnordered`:
  - Acquire tool permit per call (max 8 parallel tools via ConcurrencyController)
  - 300s timeout per tool
  - Hook: `tool.pre_execute` (can block/modify input)
  - Results collected and forwarded as `StreamEventType::ToolResult`
  - Hook: `tool.post_execute` (notification)
  - Results saved in deterministic order
- **Hook: `agent.turn`** — notification after tool execution
- **Update RunProgress** — increment iteration_count, tool_call_count atomics
- If tool calls present: `continue` loop (LLM needs to respond to results)

#### Auto-Continuation

If no tool calls but `active_task` is set and response `looks_like_continuation_pause()`:
- Inject synthetic user message: `<system>Continue with your current objective...</system>`
- Up to `MAX_AUTO_CONTINUATIONS_DEFAULT` (5) times, ceiling of 50 for work tasks

#### Output Recovery

If response is truncated due to max_tokens (detected via usage):
- Up to `MAX_OUTPUT_RECOVERY_ATTEMPTS` (3) retries
- Injects "continue from where you left off" as synthetic user message

#### Extended Iterations

Default max iterations is 100, but extends to 200 if genuine progress is detected
(tool calls being made, not stuck in a loop).

#### Post-Loop

- Debounced memory extraction (5s idle per session)
- Extract facts via LLM, store in memory DB

### 4.2 Steering System

**File:** `crates/agent/src/steering.rs`

13 generators run each iteration, producing `SteeringDirective` structs (label, content, priority 0–10):

| # | Generator | Condition | Behavior |
|---|-----------|-----------|----------|
| 1 | **IdentityGuard** | `iteration >= 8 && iteration % 8 == 0` | Re-affirm agent identity every 8 turns |
| 2 | **ChannelAdapter** | Always | Adapt output format: DM (concise), CLI (plain text), Voice (1–2 sentences) |
| 3 | **ToolNudge** | `iteration >= 5 && turns_since_tool_use >= 5 && active_task` | Nudge toward tool use when idle |
| 4 | **PendingTaskAction** | `iteration >= 2 && active_task && last_response_text_only` | Force action on active task |
| 5 | **OutputDiscipline** | `iteration >= 1 && last_response > 300 chars` | Suppress verbose narration (non-Claude) |
| 6 | **NarrationSuppressor** | `iteration >= 1 && any narrating_turn >= 1` | "STOP narrating tool calls" (non-Claude) |
| 7 | **RepetitionDetector** | `iteration >= 3 && recent_texts_share_40%_trigrams` | Catch restating same info (non-Claude) |
| 8 | **LoopDetector** | Budget-based only | 70%: caution, 90%: critical warning, 100%: must stop |
| 9 | **ErrorRecovery** | `consecutive_errors >= 3` | Soft advisory: "try a different approach" |
| 10 | **PresenceAwareness** | `iteration >= 2 && user_presence set` | Adapt to user focus: unfocused="work autonomously", returned="summarize progress" |
| 11 | **ContextPressure** | `iteration >= 15 && iteration % 15 == 0` | "Context window filling; summarize instead of quoting" |
| 12 | **JanusQuotaWarning** | `quota_warning set && !is_ollama` | Warn about budget consumption |
| 13 | **AskToolNudge** | `last_response_has_question? && !is_claude` | "Use ask tool for user input, not plain text" |

**Provider-aware skipping:**
- Claude (direct Anthropic): Skips NarrationSuppressor, OutputDiscipline, RepetitionDetector, AskToolNudge
- Ollama: Skips JanusQuotaWarning

**Force break logic:** Only hard-stops on `user_requested_stop && iteration > 2`. Everything
else (errors, loops) handled by iteration budget only — aggressive circuit breakers kill
legitimate work.

### 4.3 Prompt Assembly

**File:** `crates/agent/src/prompt.rs`

Two-phase build: static prefix (cacheable) + dynamic suffix (per-iteration).

**Static prefix (8 sections):**
1. Identity & Personality — "You are {agent_name}, a personal AI companion..."
2. Capabilities Overview — "Your tools: {tool_list}"
3. Core Behavior — "You are a goal-driven agent... Use tools to make progress..."
4. STRAP Documentation — Context-injected docs for active tools
5. Tool Usage Discipline — "Always validate file paths. Never guess..."
6. Channel Adaptation — Channel-specific guidance
7. Communication Etiquette — "Be concise. No narration..."
8. Model-Specific Guidance — Provider-specific instructions

**Dynamic suffix (per-iteration):**
- Provider guidance, active task, work tasks
- Formatted steering directives
- Proactive context (inbox results)
- Tool docs cache (survives eviction)
- User timezone

**Cache boundary:** `<!-- CACHE_BOUNDARY -->` marker separates static from semi-dynamic
content, enabling prompt caching providers to cache the prefix separately.

```rust
pub enum PromptMode {
    Full,      // All sections: memory, steering, STRAP docs, etiquette
    Minimal,   // Identity + capabilities + behavior core only (for sub-agents)
}
```

### 4.4 Pruning & Compaction

**File:** `crates/agent/src/pruning.rs`

Three-tier compaction strategy:

| Tier | Function | Trigger | Behavior |
|------|----------|---------|----------|
| Primary | `apply_sliding_window()` | Every iteration | Walk backwards, evict when over 40k tokens or 80 messages; never evict current-run messages |
| Secondary | `micro_compact()` | Near warning threshold | Trim old tool results to `[trimmed: {tool} result]`; preserve 3 most recent |
| Tertiary | `time_based_micro_compact()` | 5+ min inactivity gap | Clear stale results (provider cache expired); keep only 1 recent |
| Fallback | `build_llm_summary()` | First eviction | Generate structured 10-section summary via sidecar LLM |

**Key constants:**
```rust
const CHARS_PER_TOKEN: usize = 4;
const IMAGE_CHAR_ESTIMATE: usize = 8000;
const MICRO_COMPACT_KEEP_RECENT: usize = 3;
const MICRO_COMPACT_COUNT_TRIGGER: usize = 4;
const TIME_BASED_GAP_THRESHOLD_SECS: i64 = 300;  // 5 min
const MAX_MESSAGE_COUNT: usize = 80;
pub const DEFAULT_WINDOW_MAX_TOKENS: usize = 40_000;
```

### 4.5 Model Selection

**File:** `crates/agent/src/selector.rs`

**10-step fallback chain:**
1. Task routing (check `task_routing["vision"]`)
2. Task fallbacks (try ordered list)
3. General routing (`task_routing["general"]`)
4. Default model
5. First non-gateway active model
6. CLI preferred (claude-code, codex-cli, gemini-cli)
7. Last resort gateway (Janus)
8. Config default

**Task classification keywords:**
- Vision: `data:image/`, `"type":"image"`
- Audio: `data:audio/`, `"type":"audio"`
- Reasoning: "think through", "analyze", "step by step"
- Code: "code", "function", "python", "react"
- General: default

**Failure tracking:** Exponential backoff 5s → 1h per model. Cleared on `clear_failed()`.

### 4.6 Tool Filtering

**File:** `crates/agent/src/tool_filter.rs`

All registered tools are ALWAYS included in the tool definition list. Filtering controls
which STRAP docs are injected and which deferred tools are activated.

**Context groups** (13): web, event, loop, work, desktop, app, organizer, music, settings, keychain, spotlight, execute, emit

**Deferred tools** (5): loop, work, execute, plugin, publisher — activate on keyword match or explicit call

**Activation logic:** Scans last 5 messages + called_tools for context keywords.

### Helper Functions

| Function | Description |
|----------|-------------|
| `looks_like_continuation_pause()` | Detects 25+ "should I continue?" patterns |
| `convert_messages()` | `ChatMessage` → `ai::Message` conversion |
| `sanitize_message_order()` | Reorders tool results after their assistant, strips orphans |
| `build_system_prompt()` | Combines custom system with DB context + model identity |
| `detect_objective()` | Background task classification via LLM |

---

## 5. Session Management

**File:** `crates/agent/src/session.rs`

### Data Structure

```rust
pub struct SessionManager {
    store: Arc<Store>,
    chat_ids: Arc<RwLock<HashMap<String, String>>>,      // session_id -> active_chat_id
    session_keys: Arc<RwLock<HashMap<String, String>>>,   // session_id -> session_key (name)
}
```

Two caches:
- `chat_ids`: maps `session_id` (UUID) → `active_chat_id` (the current conversation's chat_id)
- `session_keys`: maps `session_id` (UUID) → `session_key` (the frontend-visible identifier)

Sessions and chats are **decoupled**: a session can hold multiple conversations over time.
`session.active_chat_id` points to the current conversation. `rotate_chat()` creates a new
conversation under the same session, preserving old messages and session-level preferences.

### Public Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `new()` | `(Arc<Store>) -> Self` | Constructor |
| `get_or_create()` | `(&self, session_key, user_id) -> Result<Session>` | Upsert session by key+scope; ensures `active_chat_id` is set |
| `resolve_session_key()` | `(&self, session_id) -> Result<String>` | Cache-first lookup: session_id → key |
| `active_chat_id()` | `(&self, session_id) -> String` | Public accessor for resolved chat_id |
| `get_messages()` | `(&self, session_id) -> Result<Vec<ChatMessage>>` | Load + sanitize messages for active conversation |
| `append_message()` | `(&self, session_id, role, content, tool_calls, tool_results, metadata) -> Result<ChatMessage>` | Create message with token estimate |
| `get_summary()` / `update_summary()` | — | Rolling compaction summary |
| `get_active_task()` / `set_active_task()` / `clear_active_task()` | — | Pinned objective tracking |
| `get_work_tasks()` / `set_work_tasks()` | — | Work tasks JSON for steering |
| `rotate_chat()` | `(&self, session_id, user_id) -> Result<String>` | Create new conversation under same session; returns new chat_id |
| `reset()` | `(&self, session_id) -> Result<String>` | Alias for `rotate_chat(session_id, None)` |
| `clear_current_messages()` | `(&self, session_id) -> Result<()>` | Delete messages within current conversation (used by compact) |
| `list_sessions()` | `(&self, scope) -> Result<Vec<Session>>` | List by scope |
| `delete_session()` | `(&self, session_id)` | Delete session + messages |
| `store()` | `(&self) -> &Arc<Store>` | Accessor |

### Key Behaviors

- **Token estimation**: `chars / 4` heuristic for content + tool_calls + tool_results
- **Message sanitization**: `sanitize_messages()` removes orphaned tool results
  (tool messages whose `tool_call_id` doesn't match any assistant's tool calls)
- **Empty message rejection**: Skips messages where content, tool_calls, and tool_results are all empty/null
- **Chat ID resolution**: `resolve_chat_id()` prefers `session.active_chat_id`, falls back to `session.name` (legacy compat), then `"chat-{session_id}"`
- **Non-destructive reset**: `rotate_chat()` creates a new chat_id, updates `active_chat_id`, resets conversation-scoped counters (message_count, summary, active_task) but preserves session-level preferences (model_override, provider_override)

### Session-to-Chat Relationship

```
sessions table              chats table                chat_messages table
==============              ===========                ===================
id (UUID)
name (session_key)
active_chat_id ────────>    id (UUID)           <──── chat_id (FK)
scope, scope_id             session_name ──┐          role, content, ...
model_override              title          │
provider_override           user_id        │
                                           └── links chat back to session
```

Sessions are **decoupled** from chats:
- `session.active_chat_id` points to the current conversation
- `chat.session_name` links a chat back to its parent session (for history queries)
- `rotate_chat()` creates a new chat under the same session; old messages are preserved
- `create_chat_message_for_runner` auto-creates the parent `chats` row via `INSERT OR IGNORE`

**Migration:** `0075_session_conversations.sql` adds `active_chat_id` to sessions and
`session_name` to chats, with backfill: `active_chat_id = session.name` for existing sessions.
Runtime fallback in `get_or_create()` handles sessions missed by the migration.

---

## 6. Key Parser

**File:** `crates/agent/src/keyparser.rs`

### Session Key Formats

```
agent:<agentId>:<channel>      — Agent-scoped session
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
}
```

### Build Functions

| Function | Output Format |
|----------|---------------|
| `build_session_key(channel, type, id)` | `"discord:group:123"` |
| `build_agent_session_key(agent_id, channel)` | `"agent:bot1:web"` |
| `build_subagent_session_key(parent, child)` | `"subagent:parent:child"` |
| `build_thread_session_key(parent, thread)` | `"discord:group:123:thread:t1"` |
| `build_topic_session_key(parent, topic)` | `"slack:channel:abc:topic:t2"` |

### Predicate Functions

`is_subagent_key()`, `is_acp_key()`, `is_agent_key()`

### Extraction Functions

`extract_agent_id()`, `resolve_thread_parent_key()`

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
    active_chat_id TEXT,                             -- 0075 (decoupled from chat_id)
);
UNIQUE INDEX ON sessions(name, scope, scope_id);  -- Upsert target
```

### Chats Table (migration 0008 + 0075)

```sql
CREATE TABLE chats (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL DEFAULT 'New Chat',
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    user_id TEXT,                                    -- 0009: companion mode
    session_name TEXT                                -- 0075: links chat to parent session
);
INDEX idx_chats_updated_at ON chats(updated_at DESC);
INDEX idx_chats_session_name ON chats(session_name, updated_at DESC);
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
INDEX idx_chat_messages_chat_id ON chat_messages(chat_id);
INDEX idx_chat_messages_day ON chat_messages(chat_id, day_marker);
```

**Critical:** The FK constraint means a `chats` row MUST exist before inserting messages.
`create_chat_message_for_runner()` handles this automatically with `INSERT OR IGNORE`.

### Rust Models

```rust
pub struct Session {
    pub id: String,
    pub name: Option<String>,          // = session_key
    pub scope: Option<String>,
    pub scope_id: Option<String>,
    pub summary: Option<String>,
    pub token_count: Option<i64>,
    pub message_count: Option<i64>,
    pub active_chat_id: Option<String>,
    // ... (see db/src/models.rs for all 20+ fields)
}

pub struct Chat {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub user_id: Option<String>,
    pub session_name: Option<String>,
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
    // NOTE: is_compacted column exists in DB (migration 0048) but is NOT mapped in this struct
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
- `set_session_active_chat_id()`, `list_chats_by_session()`

**Chats** (`crates/db/src/queries/chats.rs`):
- `create_chat()`, `get_chat()`, `list_chats()`, `count_chats()`
- `create_chat_for_session()` — creates chat with session_name link
- `create_chat_message_for_runner()` — auto-creates parent chat row, inserts message with all fields
- `create_chat_message()` — basic (REST endpoints)
- `get_chat_messages()`, `get_recent_chat_messages()`, `get_recent_chat_messages_with_tools()`
- `get_chat_messages_paginated()` — cursor-based (created_at < ?)
- `get_chat_messages_budgeted()` — fetch by character budget (for context windowing)
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

struct LaneState {
    queue: VecDeque<LaneTask>,   // FIFO queue
    active: usize,               // Currently executing tasks
    max_concurrent: usize,       // Cap (0 = unlimited)
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
| `heartbeat` | `lanes::HEARTBEAT` | 0 (unlimited) | Proactive ticks |
| `comm` | `lanes::COMM` | 0 (unlimited) | NeboAI messages |
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
5. Log warning for stale tasks (waited > `warn_after_ms`)

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
    Text,                // Incremental text content
    ToolCall,            // Tool invocation from LLM
    ToolResult,          // Tool execution output (runner-generated)
    Error,               // Error during streaming
    Done,                // Stream complete
    Thinking,            // Extended thinking block
    Usage,               // Token usage stats
    RateLimit,           // Rate limit headers from provider
    ApprovalRequest,     // Tool needs user approval (runner-generated)
    AskRequest,          // Interactive question for user (runner-generated)
    PlanApproval,        // Agent plan for user approval (see §27)
    FollowupSuggestions, // Chat continuation chips
    SubagentStart,       // Sub-agent spawned
    SubagentProgress,    // Sub-agent update
    SubagentComplete,    // Sub-agent done
    ToolSummary,         // Brief tool execution summary
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
    pub provider_metadata: Option<HashMap<String, String>>,
}
```

Factory methods: `StreamEvent::text()`, `thinking()`, `tool_call()`, `error()`,
`done()`, `usage()`, `rate_limit_info()`, `approval_request()`, `ask_request()`,
`plan_approval()`, `followup_suggestions()`, `tool_summary()`

### Supporting Types

```rust
pub struct ToolCall { pub id: String, pub name: String, pub input: serde_json::Value }
pub struct UsageInfo { pub input_tokens: i32, pub output_tokens: i32, pub cache_creation_input_tokens: i32, pub cache_read_input_tokens: i32 }
pub struct RateLimitMeta { remaining_requests, remaining_tokens, reset_after_secs, retry_after_secs, session_limit_tokens, session_remaining_tokens, session_reset_at, weekly_limit_tokens, weekly_remaining_tokens, weekly_reset_at }
pub struct ToolDefinition { pub name: String, pub description: String, pub input_schema: serde_json::Value }
pub struct Message { pub role: String, pub content: String, pub tool_calls: Option<Value>, pub tool_results: Option<Value>, pub images: Option<Vec<ImageContent>> }
pub struct ChatRequest { pub messages: Vec<Message>, pub tools: Vec<ToolDefinition>, pub max_tokens: i32, pub temperature: f64, pub system: String, pub static_system: String, pub model: String, pub enable_thinking: bool, pub metadata: Option<HashMap<String, String>> }
```

### Provider Trait

```rust
pub trait Provider: Send + Sync {
    fn id(&self) -> &str;
    fn profile_id(&self) -> &str { "" }
    fn handles_tools(&self) -> bool { false }
    fn supports_tool_result_images(&self) -> bool { false }
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
    pub run_registry: RunRegistry,              // Global run tracking
    pub comm_manager: Arc<PluginManager>,       // NeboAI comm plugin
    pub channel_providers: Arc<RwLock<HashMap<String, Arc<dyn ChannelProvider>>>>,
    pub approval_channels: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,
    pub ask_channels: Arc<Mutex<HashMap<String, oneshot::Sender<String>>>>,
    pub extension_bridge: Arc<browser::ExtensionBridge>,
    pub hooks: Arc<napp::HookDispatcher>,
    pub mcp_context: Arc<tokio::sync::Mutex<tools::ToolContext>>,
    pub event_bus: tools::EventBus,
    pub event_dispatcher: Arc<workflow::events::EventDispatcher>,
    pub agent_registry: Arc<RwLock<AgentRegistry>>,
    pub janus_usage: Arc<tokio::sync::RwLock<Option<JanusUsage>>>,
    pub store: Arc<Store>,
    pub config: Config,
    pub skill_loader: Arc<Loader>,
    pub plugin_store: Arc<PluginStore>,
    pub presence: Arc<PresenceTracker>,
    pub proactive_inbox: Arc<ProactiveInbox>,
    pub concurrency: Arc<ConcurrencyController>,
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

### Message Flow: NeboAI → Agent

```
NeboAI Gateway ──WS──> NeboAIPlugin ──> PluginManager.message_handler
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
   - Build session key: `"neboai:chat:<conversation_id>"` or `"neboai:dm:<conversation_id>"`
   - Build `ChatConfig` with:
     - `origin: Origin::Comm`
     - `lane: lanes::COMM`
     - `comm_reply: Some(CommReplyConfig { provider: "neboai", topic, conversation_id })`
   - Call `run_chat(&state, config)`
   - Emit into event bus for agent triggers
3. **Other topics**: Emit into event bus + broadcast to frontend as `"comm_message"`

### Reply Path

When `comm_reply` is set in `ChatConfig`, `run_chat()` accumulates `full_response`
and streams chunks during reception. Final send via `send_to_channel()`
with dedup: skips final complete message if chunks were already streamed.

### Loop/Comm Dispatch Rendering

**File:** `crates/server/src/chat_dispatch.rs`

When `comm_reply` is set, the `run_chat()` event loop mirrors the run to the loop so a
reply renders the same collapsed "Used N tools" timeline as the local app — not just a
flat text blob.

**Text streaming** (chat_dispatch.rs:391–418): each `Text` event is appended to
`comm_buffer` and sent via `send_comm_msg()` as a `CommMessageType::Stream` chunk. The
FIRST chunk flushes immediately (so the loop paints opening text the instant the model
produces it); subsequent chunks coalesce at `COMM_COALESCE_MS` (chat_dispatch.rs:330,
aliased to `COALESCE_MS` = 25ms — the same cadence as the local-chat WS coalescing). All
comm messages carry `senderName` metadata for per-agent attribution.

**Tool activity** (chat_dispatch.rs:466–540): `send_comm_tool_activity()` mirrors tool
events to the loop as `CommMessageType::ToolActivity` messages tagged with the response's
`stream_id` so the loop groups them under the reply:
- `ToolCall` → `phase: "start"`, carries the tool input as `request` (capped 2000 chars)
- `ToolResult` → `phase: "result"`, carries the trimmed output (capped 4000 chars) + `is_error`

**Heartbeat filtering** (chat_dispatch.rs:64–67, 391): orchestrator `_Working on: ..._`
progress lines (the 30s "still alive" signal sub-agent runs emit, see §23) are detected by
`is_progress_heartbeat()` (`starts_with("_Working")` && `ends_with('_')`) and excluded from
the streamed comm chunks — the loop instead gets a live `send_typing()` activity signal
(chat_dispatch.rs:430, 496). Thinking blocks and tool starts also emit `send_typing()` with
a phase label.

**Finalized response** (chat_dispatch.rs:832): the final `CommMessageType::Message` runs
`full_response` through `strip_progress_heartbeats()` (chat_dispatch.rs:73) so any heartbeat
lines that slipped into the accumulated text never land in the reply that replaces the
streamed bubble.

---

## 13. Codes System

**File:** `crates/server/src/codes.rs`

### Code Format

```
PREFIX-XXXX-XXXX
```
Where PREFIX is NEBO/SKIL/WORK/AGNT/LOOP/PLUG/APPX and XXXX is 4 Crockford Base32 characters
(charset: 0-9A-Z excluding I/L/O/U to reduce confusion).

### Code Types

```rust
pub enum CodeType { Nebo, Skill, Work, Agent, Loop, Plugin, App }
```

### Detection — Dual Layer

**Backend** (`crates/server/src/codes.rs`): `detect_code(&prompt)` — checks if prompt is
exactly a code (trimmed, case-insensitive). Returns `Option<(CodeType, &str)>`.

**Frontend** (`app/src/lib/chat/controller.svelte.ts`): Regex `CODE_RE` matches the same
pattern in the chat controller's `send()` function for **instant modal feedback** before the
WS round-trip. Also duplicated in the new thread page (`threads/+page.svelte`) which has its
own `handleSend` that bypasses the controller.

**Marketplace sidebar** (`app/src/routes/marketplace/+layout.svelte`): Same regex in the
"Install code" input form — dispatches `nebo:code_processing` and sends via WS.

### Interception Point

In `dispatch_chat()` (ws.rs), before the prompt reaches the agent:
```rust
if let Some((code_type, code)) = crate::codes::detect_code(&prompt) {
    crate::codes::handle_code(state, code_type, code, &session_id).await;
    return;
}
```

### Handler Flow

1. Broadcast `"code_processing"` with `{code, code_type, status_message}`
2. Dispatch to per-type handler
3. For agents: broadcast `"agent_installed"` immediately after persist (before cascade)
4. Cascade dep resolution runs in **background** (`tokio::spawn`) — does not block result
5. Broadcast `"code_result"` with `{success, artifact_name, artifact_id, checkout_url, needsAuth}`
6. Always broadcast `"chat_complete"` (resets frontend loading state)

Per-type handlers:
- **NEBO**: `redeem_nebo_code()` → store bot_id + token → activate NeboAI
- **SKILL**: `install_skill()` → persist to filesystem → reload skill loader → cascade deps
- **WORK**: `install_workflow()` → persist to DB + filesystem → cascade deps
- **AGNT**: `install_agent()` → clean reinstall → persist → broadcast `agent_installed` → background cascade → auth sweep → workflow bindings → auto-activate
- **LOOP**: `join_loop()` → register membership
- **PLUG**: `install_plugin()` → download .napp → install via plugin_store → register structured tools → cascade deps
- **APP**: `install_app()` → same as AGNT with app flag

### Payment Support

If API returns `status == "payment_required"`, the result includes `checkout_url`
for Stripe checkout redirect. Frontend shows payment phase in CodeInstallModal.

### Agent Install Special Logic

- Clean reinstall: stops agent worker, removes from registry, unregisters workflows, deletes from DB
- Cascade resolution for plugins/skills runs in **background** (`tokio::spawn`) — does not block `code_result`
- Auth sweep: checks plugins for pending OAuth (broadcasts `agent_auth_required` if needed)
- Workflow binding processing from typeConfig or DB frontmatter
- Auto-activation only if no auth required (wizard completes first)
- NeboAI registration for owner's personal loop

### Frontend — CodeInstallModal

**File:** `app/src/lib/components/chat/CodeInstallModal.svelte`

Self-contained modal driven by WS events via `window.addEventListener`. Phases:

| Phase | Trigger | UI |
|-------|---------|-----|
| `installing` | `nebo:code_processing` | Spinner + code display + Cancel |
| `auth` | `nebo:agent_auth_required` | Multi-plugin OAuth queue (Connect/Skip) |
| `done` | `nebo:code_result` (success) | Green check, auto-closes 1.5s |
| `error` | `nebo:code_result` (failure) | Error message + Close |
| `payment` | `nebo:code_result` (payment_required) | Checkout link |

**Safety net**: 30-second timeout — if `code_result` never arrives, transitions to `done`
with "finalizing dependencies..." message.

**Sidebar refresh**: Dispatches `nebo:agent_installed` on completion so the agent roster
reloads immediately.

**Auth flow**: Uses `authLogin(slug)` API → backend broadcasts `plugin_auth_url` →
`window.open()` → backend broadcasts `plugin_auth_complete` → advance to next plugin
in queue or finish.

**Mounted in**: `[agentId]/+layout.svelte`, `app/[agentId]/+page.svelte`,
`marketplace/+layout.svelte`.

### WebSocket Events

| Event | Direction | Payload |
|-------|-----------|---------|
| `code_processing` | Server → Client | `{code, code_type, status_message}` |
| `code_result` | Server → Client | `{success, artifact_name, artifact_id, checkout_url, needsAuth, error}` |
| `agent_installed` | Server → Client | `{agentId, name}` |
| `agent_activated` | Server → Client | `{agentId, name}` |
| `agent_auth_required` | Server → Client | `{agentId, plugins: [{slug, label, description}]}` |
| `plugin_auth_url` | Server → Client | `{url}` |
| `plugin_auth_complete` | Server → Client | `{}` |
| `plugin_auth_error` | Server → Client | `{error}` |
| `plugin_installing` | Server → Client | `{plugin}` |
| `plugin_installed` | Server → Client | `{plugin}` |
| `dep_pending` | Server → Client | `{reference, depType}` |
| `dep_installed` | Server → Client | `{reference, depType}` |
| `dep_failed` | Server → Client | `{reference, depType, error}` |
| `dep_cascade_complete` | Server → Client | `{installed, pending, failed}` |
| `chat_complete` | Server → Client | `{session_id}` |

### Entry Points

1. **Chat** — user pastes code in chat composer → controller detects → WS send → backend intercepts
2. **Marketplace sidebar** — user types code in "Install code" input → Go button → WS send
3. **REST** — `POST /api/v1/codes` with body `{"code": "SKIL-RFBM-XCYT"}`

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
    private currentPresence: 'focused' | 'unfocused' | 'away';
}
```

**Connection flow**:
1. Create WebSocket to `ws://localhost:PORT/ws`
2. On open: send `{"type": "auth", "data": {"token": "..."}}` or `{"type": "connect"}`
3. Wait for `auth_ok` → set status "connected", flush queue
4. Auto-reconnect on disconnect (exponential backoff: 2s, 4s, 8s... max 30s)

**Message format**: `{"type": "...", "data": {...}, "timestamp": "..."}`

**Presence tracking**: Attaches `visibilitychange`, `focus`, `blur` listeners. Sends
`ws.send('presence', { status })` on change. 5-min timer from `unfocused` → `away`.

### Event Dispatcher Bridge (`app/src/lib/websocket/listeners.ts`)

Routes all WebSocket events to window custom events so chat components can listen:
- WebSocket `chat_stream` → `window.dispatchEvent(new CustomEvent('nebo:chat_stream', {detail: data}))`
- Similarly for: `tool_start`, `tool_result`, `thinking`, `chat_complete`, `chat_message`, `approval_request`, `ask_request`, `subagent_*`, etc.

### ChatComposer.svelte (`app/src/lib/components/chat/ChatComposer.svelte`)

Rich message input built on **TipTap** (`@tiptap/core`) with these extensions:

- **StarterKit** — basic text editing (headings/codeBlock/horizontalRule/blockquote disabled)
- **Mention** (`@tiptap/extension-mention`) — `@` autocomplete driven by `suggestion` API, renders styled agent chips via `renderHTML`, produces `<@id>` tokens on serialize
- **SlashDetector** — custom `Extension.create()`, fires on `onUpdate`, shows `SlashCommandMenu` when text starts with `/` and has no spaces
- **DictationMark** — custom `Mark.create()`, renders `<span data-dictation class="bg-primary/20 border-b-2 border-primary/60 rounded-sm">`, highlights live-transcribed text
- **Placeholder** — (via StarterKit) placeholder text when editor is empty

**Dictation integration**:
- `dictationStore` + `$combinedTranscript` from `$lib/stores/dictation`
- `composerOwnerId` (UUID) prevents cross-composer interference
- `buildDictationDoc(before, dictation, after)` — constructs TipTap JSON doc with frozen cursor segments (text before/after cursor preserved, dictation text marked)
- Live rebuild: `$effect` watches `$combinedTranscript`, rebuilds doc + repositions cursor on every transcript update
- On dictation stop: strips dictation marks, preserves cursor position
- **Cmd+D** hotkey toggles dictation (document-level `keydown` listener)
- **VoiceButton** in toolbar — starts dictation or opens voice conversation overlay
- **VoiceModeOverlay** — full-screen voice conversation mode (separate from in-editor dictation)

**Draft persistence**:
- Saves TipTap JSON to `localStorage` key `nebo:draft:{agentId}` on every edit (debounced 300ms)
- Restores on mount (`restoreDraft()`), clears on send (`clearDraft()`)
- Flushes pending save on destroy

**Other features**:
- **File attachments**: Image thumbnails with remove button, file chips with size
- **Drag-and-drop**: Sets `_composerHandled` flag so ChatPane overlay doesn't interfere
- **Serialization**: `serializeContent()` walks TipTap JSON tree (`editor.getJSON()`), extracts text + `<@id>` mention tokens + `\n` for paragraphs
- **IME handling**: `isComposing` state prevents Enter-to-send during CJK composition
- **Send**: Enter to send (Shift+Enter for newline, suppressed during IME), calls `onsend(text, files, mentions)`

**Props**: `agentName`, `agentId`, `placeholder`, `allAgents`, `onsend`, `onstop`, `isLoading`

### ChatPane.svelte (`app/src/lib/components/chat/ChatPane.svelte`)

Chat display and layout:

- **Message rendering**: Renders user (right-aligned), assistant (left-aligned), thinking (collapsible details), tool (expandable JSON), tool-group (connector lines), ask (interactive widgets), delegate agent identity chips
- **Message grouping**: `groupedMessages` derived groups consecutive tool messages into `{ type: 'tool-group', tools: [...] }`
- **Scroll management**: Auto-scroll on new messages (disabled on manual scroll-up), scroll-to-bottom button
- **Edit/Copy/Redo**: Inline edit for user messages (Ctrl+Enter to save), copy to clipboard, redo from message
- **Drag-drop overlay**: "Drop files here" visual when dragging files over pane
- **Mention chips**: Renders `<@id>` in messages as styled span chips with agent avatar/name
- **Ask widgets**: Renders `AskWidget.svelte` for `type: 'ask'` messages (buttons, select, radio, checkbox, confirm)
- **Delegate identity**: Renders agent avatar chip above messages from @mentioned agents (`delegateAgentId`/`delegateAgentName`)
- **Client-side markdown rendering**: `chat_stream` events no longer include server-rendered HTML; frontend renders MD-to-HTML client-side via `marked.parse()`. Server HTML is only used in `chat_complete` for the final message

**Props**: `messages`, `agentName`, `agentId`, `headerTitle`, `emptyIcon/Title/Desc`, `allAgents`, `onsend`, `onstop`, `onedit`, `onredo`, `onasksubmit`, `isLoading`

### Thread-Based UI (Routes)

**`[agentId]/threads/+layout.svelte`** — 3-column layout:
- Col 2 (260px): Thread list sidebar with AgentTabBar, New Thread link, thread rows
- Col 3 (flex-1): Child route content

**`[agentId]/threads/+page.svelte`** — New thread empty state:
- ChatPane with empty state, on first send: creates chat via API, sends prompt via WS, navigates to new thread

**`[agentId]/threads/[threadId]/+page.svelte`** — Single thread page:
- Loads messages via REST API on mount (filters to `user`/`assistant` roles only, hiding `system`)
- Listens to window custom events (`nebo:chat_stream`, `nebo:thinking`, `nebo:tool_start`, `nebo:tool_result`, `nebo:chat_complete`, `nebo:ask_request`)
- `streamingContent` — `Record<agentId, rawMarkdown>` for raw text accumulation (per-agent buffers for @mention support); client renders to HTML via `marked.parse()`
- `pendingTools` — `Map<tool_id, { idx, startTime }>` to match `tool_start` with `tool_result`
- `isMyEvent(data)` — validates `data.agentId === agentId || data.originAgentId === agentId`
- `handleAskRequest` — appends `{ type: 'ask', requestId, prompt, widgets }` message on `nebo:ask_request`
- `handleAskSubmit` — sends WS `ask_response`, updates ask message with chosen response
- Optimistic UI: user messages added locally before WebSocket send completes

### Message Flow (Frontend)

```
User types in ChatComposer (TipTap editor)
  → serializeContent() walks editor.getJSON() → { text, mentions }
  → onsend(text, files, mentions)
  ↓
[+page.svelte].handleSend(text)
  → optimistically add user message
  → ws.send('chat', { prompt, agent_id, session_id })
  ↓
Backend processes → streams response
  ↓
WS messages arrive → listeners.ts → window custom events
  ↓
[+page.svelte] event listeners:
  → nebo:chat_stream → accumulate raw in streamingContent[agentId] (no server HTML; client renders)
  → nebo:thinking → add thinking message
  → nebo:tool_start → flush pending text, add tool message, track in pendingTools
  → nebo:tool_result → update tool status/duration/response
  → nebo:ask_request → add { type: 'ask', requestId, prompt, widgets } message
  → nebo:chat_complete → finalize streamed content, clear loading
  ↓
displayMessages derived: [...messages, ...streaming extra]
  → ChatPane renders (client-side marked.parse() for streaming, server HTML for finalized messages)
  → Note: new surfaces should use the Unified Chat Controller (§29) instead of manual event wiring
```

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
| `chatWithAgent(agentId, prompt)` | `POST /api/v1/agents/{agentId}/chat` |
| `getEntityConfig(type, id)` | `GET /api/v1/entity-config/{type}/{id}` |
| `updateEntityConfig(type, id, patch)` | `PUT /api/v1/entity-config/{type}/{id}` |
| `deleteEntityConfig(type, id)` | `DELETE /api/v1/entity-config/{type}/{id}` |

---

## 15. Per-Entity Config System

**Files:** `crates/server/src/entity_config.rs`, `crates/db/src/queries/entity_config.rs`

### Purpose

Allows per-agent and per-channel overrides of global agent settings. An entity is
identified by `(entity_type, entity_id)` where type is `"main"`, `"agent"`, or
`"channel"` and id is the agent ID, channel name, or `"main"`.

### Data Structure

```rust
pub struct ResolvedEntityConfig {
    pub entity_type: String,
    pub entity_id: String,
    pub heartbeat_enabled: bool,
    pub heartbeat_interval_minutes: i32,
    pub heartbeat_content: String,
    pub heartbeat_window: Option<(String, String)>,  // (start, end) HH:MM
    pub permissions: HashMap<String, bool>,            // tool category allow/deny
    pub resource_grants: HashMap<String, String>,      // "allow"/"deny"/"inherit"
    pub model_preference: Option<String>,
    pub personality_snippet: Option<String>,
    pub overrides: HashMap<String, bool>,              // which fields are customized (UI hint)
    pub allowed_paths: Vec<String>,                    // restrict file/shell to these dirs
}
```

### Resolution (`resolve()`)

1. Loads global defaults from `settings` table + `user_profiles.tool_permissions`
2. Loads entity-specific row from `entity_config` table
3. Layers entity values on top of globals — NULL fields inherit defaults
4. Returns `ResolvedEntityConfig` with `overrides` map showing which fields are customized
5. `resolve_for_chat()` convenience function: loads defaults + resolves in one call (best-effort, returns None on error)

### DB Schema (migration 0057 + 0065)

```sql
CREATE TABLE entity_config (
    entity_type TEXT NOT NULL,        -- "main" | "agent" | "channel"
    entity_id TEXT NOT NULL,          -- agent ID, channel name, or "main"
    heartbeat_enabled INTEGER,        -- 0/1/NULL (NULL = inherit)
    heartbeat_interval_minutes INTEGER,
    heartbeat_content TEXT,
    heartbeat_window_start TEXT,      -- HH:MM
    heartbeat_window_end TEXT,        -- HH:MM
    permissions TEXT,                 -- JSON: {"web": true, "desktop": false, ...}
    resource_grants TEXT,             -- JSON: {"screen": "allow", "browser": "deny", ...}
    model_preference TEXT,
    personality_snippet TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    allowed_paths TEXT,               -- 0065: JSON array of allowed file/shell paths
    PRIMARY KEY (entity_type, entity_id)
);
```

### Integration with Chat Pipeline

1. `dispatch_chat()` calls `resolve_for_chat()` → sets `ChatConfig.entity_config`
2. `run_chat()` extracts overrides into `RunRequest` fields (permissions, resource_grants, model_preference, personality_snippet)
3. Runner uses `model_preference` for fuzzy model resolution, `personality_snippet` prepended to system prompt
4. Permission/resource enforcement happens at tool execution time

### REST API

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/v1/entity-config/{type}/{id}` | Get resolved config (with inheritance) |
| `PUT` | `/api/v1/entity-config/{type}/{id}` | Patch-update entity overrides |
| `DELETE` | `/api/v1/entity-config/{type}/{id}` | Reset entity to inherited defaults |

---

## 16. End-to-End Event Flow

### User types "Hello" in agent thread:

1. **Frontend**: `[threadId]/+page.svelte` calls `ws.send('chat', {session_id: threadId, prompt: 'Hello', agent_id, channel: 'web'})`
2. **WS Handler**: `handle_client_ws()` receives `type: "chat"`, calls `dispatch_chat()`
3. **dispatch_chat()**: Checks `detect_code("Hello")` → None. Builds `ChatConfig`, calls `run_chat()`
4. **run_chat()**: Registers in RunRegistry, broadcasts `"chat_created"`, creates LaneTask, enqueues on `"main"` lane
5. **Lane pump**: Picks up task, spawns it
6. **Runner.run()**: Creates session, appends user message (auto-creates `chats` row), spawns `run_loop()`
7. **run_loop() iteration 1**:
   - Loads messages, applies sliding window
   - Builds system prompt (identity + STRAP + tools)
   - Selects model, acquires LLM permit
   - `provider.stream()` → receives Text events
   - Forwards each Text event to tx channel
8. **run_chat() event loop**: Receives Text events, coalesces into 75ms batches, broadcasts `"chat_stream"` to hub
9. **Frontend**: listeners.ts dispatches `nebo:chat_stream` custom event → `[threadId]/+page.svelte` accumulates in `streamingMap`
10. **run_loop()**: Stream ends, saves assistant message, no tool calls → breaks
11. **run_chat()**: Renders markdown→HTML, broadcasts `"chat_complete"`, RunHandle drops (auto-unregisters)
12. **Frontend**: `nebo:chat_complete` handler finalizes message, sets `isLoading = false`

### User sends marketplace code "SKIL-RFBM-XCYT":

1. **Frontend**: Same WS send as above
2. **dispatch_chat()**: `detect_code("SKIL-RFBM-XCYT")` → `Some((Skill, "SKIL-RFBM-XCYT"))`
3. **codes::handle_code()**: Broadcasts `"code_processing"`, calls `handle_skill_code()`
4. **handle_skill_code()**: API call to NeboAI, persists skill, reloads skill loader, cascades deps
5. Broadcasts `"code_result"` with success + artifact name
6. Broadcasts `"chat_complete"`
7. **Frontend**: Shows code processing/result UI, resets loading state

### User sends message to agent with tool calls:

1. Same flow through steps 1–7
2. **run_loop() iteration 1**: Stream returns text + tool_calls
   - Saves assistant message with tool_calls JSON
   - Executes tools in parallel (max 8 via tool_semaphore)
   - Saves tool results
   - Tool calls present → continues to iteration 2
3. **run_loop() iteration 2**: Loads updated messages (includes tool results)
   - LLM generates final response (text only, no more tool calls)
   - Saves assistant message
   - No tool calls → breaks
4. **run_chat()**: Final `"chat_complete"` broadcast

---

## 17. Slash Commands

**Files:** `app/src/lib/components/chat/slash-commands.ts`, `app/src/lib/components/chat/slash-command-executor.ts`, `app/src/lib/components/chat/SlashCommandMenu.svelte`

### Architecture

Slash commands are intercepted **before** a message reaches the agent. When the user
types `/` in the chat input, a floating autocomplete menu appears above the textarea.

```
User types "/"
     │
     ├─ ChatComposer.svelte: detects prefix, shows SlashCommandMenu
     │   └─ Arrow keys navigate, Tab/Enter selects, Escape closes
     │
User submits (Enter)
     │
     ├─ parseSlashCommand(prompt) → { command, args } or null
     │
     ├─ executeSlashCommand(command, args, ctx)
     │   ├─ returns true  → handled locally (system message shown)
     │   └─ returns false → sent to agent as normal chat message
```

### Command Reference

#### Session Commands

| Command | Args | Execution | Description |
|---------|------|-----------|-------------|
| `/new` | — | Local | Start a new chat session |
| `/reset` | — | Local | Reset current session (rotates chat) |
| `/clear` | — | Local | Clear chat display only (messages still in DB) |
| `/stop` | — | Local | Cancel active generation |
| `/focus` | — | Local | Toggle sidebar visibility |
| `/compact` | — | Local | Force context compaction |

#### Model Commands

| Command | Args | Execution | Description |
|---------|------|-----------|-------------|
| `/model` | — | Local | List all available models |
| `/model` | `<name>` | Agent | Switch model (fuzzy resolution) |
| `/think` | `off\|low\|medium\|high` | Local | Set extended thinking level |
| `/verbose` | `on\|off` | Local | Toggle verbose tool output |

#### Info Commands

| Command | Args | Execution | Description |
|---------|------|-----------|-------------|
| `/help` | — | Local | Show all slash commands |
| `/status` | — | Local | Agent connection status + lane summary |
| `/usage` | — | Local | Janus token usage quotas |
| `/export` | — | Local | Export chat as Markdown file |
| `/lanes` | — | Local | Lane concurrency status |
| `/search` | `<query>` | Local | Search chat message history |

#### Agent Commands

| Command | Args | Execution | Description |
|---------|------|-----------|-------------|
| `/skill` | `<name>` | Agent | Activate a skill by name |
| `/memory` | — / `<query>` | Local | List or search stored memories |
| `/heartbeat` | — / `wake` | Local/Agent | Show config or trigger immediate heartbeat |
| `/advisors` | — | Local | List configured advisors |
| `/voice` | — | Local | Toggle full-duplex voice conversation |
| `/personality` | — | Local | Show current personality config |
| `/wake` | `[reason]` | Agent | Trigger immediate heartbeat |

---

## 18. File Attachment & Drag-and-Drop

**Files:** `app/src/lib/components/chat/ChatComposer.svelte`, `app/src/lib/components/chat/ChatPane.svelte`, `src-tauri/src/main.rs`

### Two Entry Points

| Method | UI Element | Behavior |
|--------|-----------|----------|
| **Drag-and-drop** | Anywhere on the window | Tauri intercepts OS-level drag, inserts full path via `eval()` |
| **+ button** | Attachment button in composer | Opens native file dialog or HTML input |

### Drag-and-Drop Architecture

**Tauri mode:** `on_window_event` catches `WindowEvent::DragDrop`, serializes file paths as JSON,
calls global JS functions via `WebviewWindow::eval()`:
- `window.__NEBO_DRAG_ENTER__()` — show overlay
- `window.__NEBO_DRAG_LEAVE__()` — hide overlay
- `window.__NEBO_INSERT_FILES__(paths)` — insert file paths into composer

**Browser fallback:** Standard HTML5 drag events. Only filenames available (browser security).

### ChatComposer Attachment Handling

- `addFiles(files)` — generates preview URLs for images, stores in `attachments` array
- `removeAttachment(id)` — removes from array, revokes blob URL
- Image files get thumbnail previews; other files show as chips with size
- Files included in `onsend()` callback for the backend to process

### + Button (Native File Picker)

`POST /api/v1/files/pick` opens a native file dialog via `rfd` on the server.
Returns `{ paths: ["/full/path/..."] }`. Fallback: HTML `<input type="file">`.

---

## 19. Known Issues and Fixes

### FK Constraint on chat_messages (Fixed 2026-03-12)

**Problem:** `chat_messages.chat_id` has a `FOREIGN KEY` referencing `chats.id`, and
`PRAGMA foreign_keys = ON` is set on every connection. Agent/channel sessions had
no equivalent of companion chat upsert, causing `FOREIGN KEY constraint failed`.

**Fix:**
1. `create_chat_message_for_runner()` now does `INSERT OR IGNORE INTO chats` before inserting the message
2. `runner.run()` now propagates `append_message` errors via `?` instead of just warning

### Session Key = Chat ID Coupling (Resolved via 0075)

**Problem:** The system used `session_key` as both the session name AND the `chat_id`.

**Fix:** Migration 0075 decoupled sessions from chats via `active_chat_id` column on sessions
and `session_name` column on chats. Sessions can now have multiple chats. Runtime fallback
handles legacy sessions with `active_chat_id = session.name`.

---

## 20. Ask Widget System

Interactive user-prompt mechanism that lets tools block execution and wait for structured
user input via the chat UI. The `bot(resource: "ask")` tool calls `ctx.ask_user()`, which
renders a widget in the conversation, blocks until the user responds, and returns the
selection as a string.

### Architecture

```
Agent calls bot(resource: "ask", action: "confirm", text: "Continue?", options: [...])
  │
  ├─ bot_tool.rs handle_ask(input, ctx)
  │   ├─ Maps action to widget type (confirm→confirm, select→buttons/select, prompt→buttons)
  │   ├─ select uses "select" dropdown if >5 options, else "buttons"
  │   └─ Calls ctx.ask_user(text, widgets)
  │
  ├─ origin.rs ToolContext::ask_user()
  │   ├─ Creates oneshot channel (resp_tx, resp_rx)
  │   ├─ Inserts resp_tx into shared ask_channels (keyed by UUID request_id)
  │   ├─ Emits StreamEvent::ask_request via stream_tx
  │   └─ Blocks on resp_rx.await.ok()
  │         ↓
  │   chat_dispatch.rs → hub.broadcast("ask_request", {request_id, prompt, widgets})
  │         ↓
  │   listeners.ts → window.dispatchEvent('nebo:ask_request', data)
  │         ↓
  │   +page.svelte handleAskRequest() → appends { type: 'ask', ... } message
  │         ↓
  │   ChatPane → renders AskWidget.svelte (buttons/select/radio/checkbox/confirm)
  │         ↓
  │   User interacts → handleAskSubmit()
  │     → ws.send('ask_response', { request_id, value })
  │     → updates ask message with response badge
  │         ↓
  │   ws.rs handler → ask_channels.lock().remove(&request_id) → resp_tx.send(value)
  │         ↓
  └─ resp_rx returns Some(value) → handle_ask() returns ToolResult::ok({response: value})
     → Agent continues with user's choice
```

### Tool Interface

```
bot(resource: "ask", action: "confirm", text: "Proceed?", options: ["Yes", "No"])
bot(resource: "ask", action: "select", text: "Pick a calendar", options: ["Work", "Personal", "Family"])
bot(resource: "ask", action: "prompt", text: "What time works?", options: ["9 AM", "10 AM", "11 AM"])
```

### Widget Types

| Type | UI Element | Return Value |
|------|-----------|-------------|
| `buttons` | Horizontal button row | Label of clicked button |
| `select` | Dropdown menu + OK button | Selected option string |
| `confirm` | Yes/No buttons (default) | Label of clicked button |
| `radio` | Radio button group + Submit | Selected option string |
| `checkbox` | Checkbox list + Submit button | Comma-separated selected options |

Widget JSON format:
```json
[{"type": "checkbox", "label": "Pick items", "options": ["Option A", "Option B"]}]
```

### Frontend Component: AskWidget.svelte

**File:** `app/src/lib/components/chat/AskWidget.svelte` (ported from app-v1)

- **Props**: `requestId`, `prompt`, `widgets`, `response?`, `disabled?`, `onSubmit`
- **States**: Once answered → shows response as `badge badge-primary` badges
- **Disabled**: When `disabled` and not answered → shows "Skipped" ghost badge
- **Svelte 5**: Uses `$props()`, `$state()`, `$derived()`, all DaisyUI/Tailwind classes

### Threading Through the Stack

`ask_channels` is created in `crates/server/src/lib.rs` and threaded to both:
1. **Runner** — via `Runner::set_ask_channels()` builder → stored on Runner struct → passed to `run_loop()` → injected into `ToolContext`
2. **AppState** — stored as `state.ask_channels` for the WS handler to resolve responses

### Edge Cases

- **No UI connected** (CLI mode): `stream_tx`/`ask_channels` are `None` → `ask_user()` returns `None` → tool returns error
- **User navigates away**: Oneshot sender drops → `resp_rx.await` returns `Err` → returns `None` → tool error
- **Multiple asks**: Each gets unique `request_id`, renders as separate AskWidget messages
- **Widget disabled after chat completes**: `disabled={!isLoading}` in ChatPane

### Key Files

| File | Role |
|------|------|
| `crates/tools/src/bot_tool.rs` | `handle_ask(input, ctx)` — maps action to widget, calls `ctx.ask_user()` |
| `crates/tools/src/origin.rs` | `AskChannels` type, `ToolContext::ask_user()` |
| `crates/ai/src/types.rs` | `StreamEvent::ask_request()` constructor |
| `crates/agent/src/runner.rs` | Threads `ask_channels` into `run_loop()` → `ToolContext` |
| `crates/server/src/lib.rs` | Creates shared `ask_channels`, passes to Runner + AppState |
| `crates/server/src/handlers/ws.rs` | WS handler resolves `ask_response` → oneshot |
| `crates/server/src/chat_dispatch.rs` | Broadcasts `ask_request` stream events |
| `app/src/lib/components/chat/AskWidget.svelte` | Interactive widget UI (5 types) |
| `app/src/lib/websocket/listeners.ts` | `ask_request` → `nebo:ask_request` bridge |
| `app/src/routes/[agentId]/threads/[threadId]/+page.svelte` | `handleAskRequest`, `handleAskSubmit` |
| `app/src/lib/components/chat/ChatPane.svelte` | Renders AskWidget for `type: 'ask'` messages |

---

## 21. RunRegistry

**File:** `crates/server/src/run_registry.rs`

### Purpose

Global tracking of all active agent runs for visibility, progress monitoring, and cascading cancellation.

### Core Structs

```rust
pub struct RunEntry {
    pub run_id: String,                              // UUID
    pub session_key: String,
    pub entity_id: String,                           // Agent ID or "main"
    pub entity_name: String,                         // Display name
    pub origin: String,                              // "ws", "cron", "comm", "system"
    pub channel: String,
    pub cancel_token: CancellationToken,
    pub started_at: Instant,
    pub last_activity: Arc<AtomicU64>,               // Unix timestamp for stale detection
    pub iteration_count: Arc<AtomicU32>,
    pub tool_call_count: Arc<AtomicU32>,
    pub current_tool: Arc<Mutex<String>>,
    pub parent_run_id: Option<String>,               // For parent-child hierarchies
}

pub struct RunHandle {
    registry: Arc<RunRegistryInner>,
    pub run_id: String,
    pub last_activity: Arc<AtomicU64>,
    pub iteration_count: Arc<AtomicU32>,
    pub tool_call_count: Arc<AtomicU32>,
    pub current_tool: Arc<Mutex<String>>,
    pub cancel_token: CancellationToken,
}
// Auto-unregisters on drop (panic-safe)

pub struct RunSnapshot {
    pub run_id: String,
    pub session_key: String,
    pub entity_id: String,
    pub entity_name: String,
    pub origin: String,
    pub channel: String,
    pub iteration_count: u32,
    pub tool_call_count: u32,
    pub current_tool: String,
    pub elapsed_secs: u64,
    pub parent_run_id: Option<String>,
    pub child_count: usize,
}
```

### Public API

| Method | Description |
|--------|-------------|
| `register(params) -> RunHandle` | Register run, get live counters |
| `list_all() -> Vec<RunSnapshot>` | All runs (dashboard) |
| `list_top_level() -> Vec<RunSnapshot>` | Parent runs only |
| `list_children(parent_id) -> Vec<RunSnapshot>` | Direct children |
| `get(run_id) -> Option<RunSnapshot>` | Single run lookup |
| `find_by_session(key) -> Option<RunSnapshot>` | Find by session |
| `find_by_entity(id) -> Vec<RunSnapshot>` | All runs for entity |
| `active_count() -> usize` | Count active runs |
| `cancel(run_id) -> bool` | Cancel specific run |
| `cancel_by_session(key) -> bool` | Cancel by session |
| `cancel_by_entity(id) -> usize` | Cancel all for entity |
| `cancel_all() -> usize` | Emergency kill all |
| `is_session_active(key) -> bool` | Check if active |
| `cleanup_stale(max_idle_secs) -> usize` | Remove stale runs |

### RunHandle Usage

```rust
let handle = registry.register(params).await;
handle.inc_iteration();      // Update iteration counter
handle.start_tool("web");    // Set current_tool
handle.finish_tool();        // Clear current_tool
handle.touch();              // Update last_activity
drop(handle);                // Auto-unregisters
```

### Authorization

Main agent sees all runs; persona agents see only their own runs and descendants.

---

## 22. ConcurrencyController

**File:** `crates/agent/src/concurrency.rs`

### Purpose

Adaptive dynamic semaphore controlling global LLM concurrency based on machine resources and rate limit feedback.

### Core Struct

```rust
pub struct ConcurrencyController {
    llm_semaphore: Arc<Semaphore>,                  // Dynamic semaphore for LLM calls
    effective_permits: AtomicUsize,                 // Live permit count
    held_back: Mutex<Vec<OwnedSemaphorePermit>>,   // Permits held to reduce concurrency
    min_permits: usize,                             // Floor: 2
    ceiling: AtomicUsize,                           // Ceiling: adjusted by monitor
    backpressure: AtomicBool,                       // Rate limit flag
    tool_semaphore: Arc<Semaphore>,                 // Parallel tool execution cap (8)
}
```

### Initialization

```rust
let cpu_cores = std::thread::available_parallelism().unwrap_or(4);
let initial = (cpu_cores * 2).min(20);  // 2–20 permits based on CPU
```

### Public API

| Method | Description |
|--------|-------------|
| `acquire_llm_permit()` | Acquire permit before LLM call (blocks if at capacity) |
| `acquire_tool_permit()` | Acquire permit for parallel tool execution (cap: 8) |
| `report_success(meta)` | Clear backpressure, release held permits if headroom |
| `report_rate_limit(retry_after)` | Set backpressure, hold 50% of permits |
| `set_ceiling(n)` | Adjust ceiling (called by resource monitor) |
| `is_backpressured() -> bool` | Whether rate limited |
| `effective_permits() -> usize` | Current live permits |

### Adaptive Behavior

**Rate limit response (429):**
1. Set `backpressure = true`
2. Hold 50% of effective permits (`to_hold = effective_permits / 2`)
3. Reduces effective permits down to `min_permits` (2)
4. Future LLM calls block until success reported

**Success response:**
1. Clear backpressure
2. Release held permits if rate limit headers show remaining headroom (> 5 requests)

### Resource Monitor

Spawned as background task, polls every 30s:
- **Available memory**: `available_mb / 200` (e.g., 8GB = 40 permits)
- **CPU load**: If load > cores, scale down by `cores / load` (min 0.3x)
- **Result**: Ceiling ranges 2–50, dynamically adjusted

---

## 23. Orchestrator (Sub-Agents)

**File:** `crates/agent/src/orchestrator.rs`

### Purpose

Spawns and manages sub-agent tasks (blocking or fire-and-forget), supports DAG-based
task decomposition with reactive scheduling.

### Core Structs

```rust
struct ActiveAgent {
    task_id: String,
    description: String,
    status: String,
    cancel: CancellationToken,
}

pub struct Orchestrator {
    runner: Arc<Runner>,
    store: Arc<Store>,
    concurrency: Arc<ConcurrencyController>,
    active: Arc<RwLock<HashMap<String, ActiveAgent>>>,
    lanes: Option<Arc<LaneManager>>,
}
```

### Sub-Agent Lifecycle

1. **Spawn Request**: Creates unique task ID (`sa-{uuid}`), session key (`subagent:{parent}:{task_id}`), derives cancellation token from parent (cascading)
2. **Two Modes**:
   - **Blocking (wait=true)**: Runs to completion, returns result synchronously
   - **Fire-and-forget (wait=false)**: Spawns background task, returns immediately
3. **DAG Execution**: Decomposes task → builds TaskGraph → validates for cycles → reactive scheduling (starts tasks when deps complete) → collects dependency context (max 4000 chars per dep) → returns aggregated result

### Integration

Sub-agents call `runner.run()` with a `PromptMode::Minimal` to reduce system prompt overhead.
They inherit the parent's cancellation token (child_token), so cancelling the parent cascades.

---

## 24. @Mention Routing

**Files:** `crates/server/src/handlers/ws.rs`, `crates/server/src/chat_dispatch.rs`

### Purpose

Users can @mention sibling agents in messages (e.g., `Tell <@social-media-manager> to draft a post`).
The frontend serializes these as `<@agent-id>` tokens. The backend parses these tokens, forks async
chat runs to each mentioned agent, and routes their responses back to the originating thread.

### Architecture

```
User sends: "Tell <@social-media-manager> to draft a post"
  │
  ├─ dispatch_chat() in ws.rs:
  │   1. parse_mention_tokens() extracts ["social-media-manager"]
  │   2. Builds mention_context: "@Social Media Manager has been @mentioned and will
  │      respond separately in this thread. Do not answer on their behalf."
  │   3. Primary agent's ChatConfig gets mention_context (injected as system-role msg)
  │   4. run_chat() for primary agent (unchanged behavior)
  │   5. tokio::spawn → fork_mention_chat() for each mentioned agent
  │
  ├─ fork_mention_chat():
  │   1. Auto-activates agent if not in registry
  │   2. Creates isolated session: agent:<mentioned_id>:<channel>
  │   3. Prepends context: "[You were @mentioned in a conversation. Respond helpfully.]"
  │   4. Sets origin_agent_id = primary agent's ID
  │   5. Calls run_chat() → all WS broadcasts include "originAgentId"
  │   6. On completion: inject_delegate_response() into primary agent's session
  │
  └─ Frontend routing:
      isMyEvent(data) accepts data.agentId === myId OR data.originAgentId === myId
      Per-agent streaming buffers prevent text mixing
      Delegate messages render with agent identity chip (avatar + name)
```

### Key Functions

**`parse_mention_tokens(prompt, exclude_agent_id) -> Vec<String>`**
- Byte-level parser finds `<@...>` tokens
- Deduplicates via HashSet
- Excludes the primary agent (no self-mention)
- Validates: alphanumeric + `.` + `_` + `-`

**`fork_mention_chat(state, mentioned_id, prompt, user_id, channel, origin_agent_id)`**
- Auto-activates if agent not in registry (same pattern as dispatch_chat)
- Creates own session key (isolated conversation history)
- Passes `origin_agent_id` on ChatConfig so all WS events carry routing info

**`inject_delegate_response(state, mentioned_id, origin_agent_id, channel)`**
- Reads last assistant message from delegate's session
- Injects as **"system"** role message into primary agent's session:
  `[Response from @{id} ({name})]\n{content}`
- Hidden from frontend (loadMessages filters to user/assistant only)

### WS Payload Extension

All broadcasts in `run_chat()` use the `ws_payload!` macro, which conditionally adds
`"originAgentId"` when `origin_agent_id` is `Some(...)`:

```rust
macro_rules! ws_payload {
    ($($key:tt : $val:expr),*) => {{
        let mut v = json!({ "session_id": sid, "agentId": agent_id, $($key: $val),* });
        if let Some(ref oid) = origin_agent_id {
            v["originAgentId"] = Value::String(oid.clone());
        }
        v
    }};
}
```

### Frontend Per-Agent Streaming

**File:** `app/src/routes/[agentId]/threads/[threadId]/+page.svelte`

Per-agent streaming buffers handle concurrent streaming from primary + delegate agents:
- `streamingContent: Record<agentId, rawMarkdown>` — raw text accumulation; client renders to HTML

`displayMessages` derived appends transient streaming entries per agent, tagged with
`delegateAgentId` / `delegateAgentName` when the streamer is not the primary agent.

### ChatConfig Fields

| Field | Type | Purpose |
|-------|------|---------|
| `origin_agent_id` | `Option<String>` | Set on delegate forks; primary agent's ID. All WS payloads include `originAgentId` |
| `mention_context` | `Option<String>` | Invisible system-role msg for primary agent listing who was @mentioned |

### RunRequest Field

`mention_context: Option<String>` — injected as `"system"` role message after user message
in `Runner::run()`. Visible to LLM, invisible to frontend.

### Edge Cases

- **Self-mention**: `parse_mention_tokens` excludes the primary agent — no duplicate
- **Duplicate mentions**: HashSet dedup — one fork per unique agent
- **Agent not found**: `fork_mention_chat` logs warning and skips — no crash
- **Mentioned agent fails**: Error broadcasts with `originAgentId`, primary unaffected (separate tokio task)
- **Multiple mentions**: Each fork runs concurrently; per-agent buffers prevent text mixing
- **Loading state**: `isLoading` only clears when PRIMARY agent completes, not delegates
- **Page reload**: Delegate messages are in their own sessions — only primary agent's messages load. Full persistence is future work

### Channel Multi-Mention (Loop Fan-Out)

**File:** `crates/server/src/lib.rs` (`handle_comm_message` channel branch, ~lines 3015–3417)

The companion-thread @mention path above (`fork_mention_chat`) is distinct from how a
**loop channel** message resolves multiple mentions. When a channel message `@mentions`
several agents, each addressed agent receives a SEPARATE dispatch and replies independently
("fan-out").

**Mention resolution (non-name-based)** — `MENTION_TOKEN_RE` (`<@([0-9a-fA-F._-]+)>`,
lib.rs:62) captures every token; each `<@id>` resolves to a target (lib.rs:3030–3063):
- `id == bot_id` (`config::read_bot_id`) → the primary agent (empty-string agent id, "Nebo")
- otherwise → `state.store.get_agent_by_loop_agent_id(id)`, accepted only if `loop_exposed != 0`
- unresolved tokens are dropped with a warning (missing `loop_agent_id` or `loop_exposed=0`)

These are loop UUIDs, not agent names: `loop_agent_id` is stamped onto each exposed local
agent by `reconcile_agents()` via `set_agent_loop_agent_id()`
(`crates/server/src/codes.rs:1569`), so routing and attribution key on the stable id rather
than a display name. Resolved targets are deduped in order of appearance into
`mentioned_targets`.

**Fan-out (default)** — each resolved target gets its OWN channel session
(`neboai:channel:<convId>` for the primary, `neboai:channel:<convId>:<agentId>` for custom
agents) so histories don't collide, and runs concurrently on the `COMM` lane
(lib.rs:3335–3417). The fan-out set is capped at `MAX_FANOUT = 4` (lib.rs:3263). When more
than one agent is addressed (`is_group`), each receives a `mention_context` system note
instructing it to answer ONLY about itself in the first person and not speak for, introduce,
or quote the others — because a separate copy was delivered to each, and the platform places
the replies side by side (lib.rs:3370–3389).

**Coordination mode** — when the message also asks the agents to work together,
`coordinate = mentioned_targets.len() > 1 && wants_coordination(&text)` (lib.rs:3072).
`wants_coordination()` (lib.rs:70) is a conservative phrase match ("work together",
"collaborate", "as a team", "team up", "joint plan", "one combined", etc.) — independent
fan-out is the default; only explicit collaboration phrasing flips the mode. In coordination
mode the FIRST-mentioned agent is the LEAD and the only responder (`responders =
mentioned_targets[..1]`); the rest become `coordinator_peers` (lib.rs:3073–3084). The lead's
`mention_context` (lib.rs:3359–3369) tells it to consult each peer via
`bot(resource: "registry", action: "delegate", name: "<peer>", prompt: "...")` and write one
combined answer itself — peers do not reply on their own. Result: one reply attributed to
the lead instead of N independent answers.

**Attribution** — every dispatch and the delegate-peer name list resolve a display name via
the fallback chain `registry.get(agent_id).name → store.get_agent(agent_id).name → "Nebo"`
(lib.rs:3269–3280, 3299–3314). The primary agent uses `registry.get("assistant")`. This
ensures a custom agent's reply is attributed to its real name (e.g. "Researcher", "Chief of
Staff"), even when the agent is loop-exposed but not loaded in the local registry — never
mislabeled as the primary "Nebo". The resolved name flows into `ChatConfig.entity_name` and
the per-reply `senderName` metadata (see Loop/Comm Dispatch Rendering, §12).

### Not a Competing Pathway

@Mention routing is distinct from other inter-agent communication:

| Mechanism | Trigger | Target | Response Path |
|-----------|---------|--------|---------------|
| **@Mention (thread)** | `<@id>` in companion-thread message | Named local agent | Same thread (via `originAgentId`) |
| **@Mention (channel)** | `<@loop_id>` in loop channel message | Loop-exposed agent(s) | Each replies to the channel (fan-out), or one lead reply (coordination) |
| **Spawn/Orchestrate** | Tool call `bot(resource: "task")` | Anonymous sub-agent | Parent's tool result |
| **Loop DM** | Tool call `loop(resource: "dm")` | Remote agent via NeboAI | Separate conversation |

---

## 25. Redaction System

**File:** `crates/server/src/redact.rs`

### Purpose

Prevents secrets (API keys, tokens, passwords) passed as slash command arguments from
being stored in conversation history or logs. The original arguments are consumed for
their intended purpose (e.g., plugin dispatch) before redaction replaces them.

### Sensitive Commands

Case-insensitive match on the first whitespace-delimited token:

```
/auth, /login, /token, /key, /secret, /password, /apikey, /api-key, /api_key,
/credential, /credentials, /oauth, /connect, /register, /signup, /signin
```

### Behavior

```rust
pub fn redact_sensitive_args(prompt: &str) -> Option<String>
```

- Returns `None` for non-slash prompts, non-sensitive commands, or commands without arguments
- Returns `Some("{command} [redacted]")` when the first token matches a sensitive command and has trailing arguments
- All arguments after the command are replaced with a single `[redacted]` token

### Integration Points

- **`dispatch_chat()`** in `ws.rs` — called after code interception but before `ChatConfig` construction
- **`POST /agents/:id/chat`** in `agents.rs` — called before agent chat dispatch
- Redaction is applied to the stored prompt; the original prompt is already consumed for any plugin dispatch above

---

## 26. Ghost Text / Inline Completion

### Purpose

Provides lightweight inline text suggestions (like GitHub Copilot) while the user types
in the chat composer. Uses the cheapest available model with minimal context.

### WS Protocol

**Client → Server:** `"ghost_text"` message
```json
{ "partial_text": "I need to schedule a meet",
  "session_id": "...", "agent_id": "...", "request_id": "uuid" }
```

**Server → Client:** `"ghost_text"` event
```json
{ "request_id": "uuid", "suggestion": "ing with the client tomorrow" }
```

### Server Logic (`ws.rs`)

1. Extract `partial_text`, `session_id`, `agent_id`, `request_id` from message
2. **Guard:** If `partial_text.len() < 10`, broadcast empty suggestion and return
3. Build minimal context:
   - Session summary (truncated to 500 chars)
   - Last 4 messages from session (truncated to 200 chars each)
4. System prompt: `"Complete naturally, return ONLY completion text"`
5. Uses cheapest available model, `max_tokens: 50`
6. Spawns async task: `provider.stream()` → collect text → broadcast `"ghost_text"` event

### Frontend Behavior

- **500ms debounce** in `ChatComposer` before sending ghost_text request
- **Tab** to accept suggestion (inserts into editor)
- **Any other key** dismisses the suggestion
- Suggestion rendered as ghost/dimmed text after cursor position

---

## 27. Plan Mode Approval

### Purpose

When plan mode is enabled, the agent presents a plan describing intended tool calls
before executing them. The user can approve or reject the plan.

### ChatConfig Fields

```rust
pub plan_mode: bool,          // when true, agent shows plan before tool execution
pub tool_scope: Option<String>, // see §28
```

### Stream Event

```rust
StreamEvent::plan_approval(plan: impl Into<String>, request_id: impl Into<String>, widgets: Option<Value>)
```

Produces `StreamEventType::PlanApproval` with:
- `text` — the plan description
- `provider_metadata["request_id"]` — unique ID for this approval
- `widgets` — optional tool list for UI rendering

### WS Flow

1. Agent emits `StreamEvent::PlanApproval { request_id, text, widgets }`
2. `run_chat()` broadcasts `"plan_approval"` event:
   ```json
   { "session_id": "...", "agentId": "...", "request_id": "uuid",
     "plan": "I will search the web and then create a file...",
     "tools": [...] }
   ```
3. Frontend shows approval UI (plan text + approve/reject buttons)
4. Client sends `"plan_response"` WS message:
   ```json
   { "request_id": "uuid", "approved": true }
   ```
5. WS handler routes through `ask_channels` (same mechanism as `ask_response`):
   sends `"approved"` or `"rejected"` string via the oneshot channel
6. Runner receives the response and proceeds or aborts tool execution

---

## 28. Tool Scope Isolation

### Purpose

Restricts which tools, skills, and plugins are available to an agent during a chat run.
Used by app SDK embeds to limit agent capabilities per context.

### Configuration

`ChatConfig.tool_scope: Option<String>` — name of the scope to apply.

Scopes are defined in the agent's `agent.json`:
```json
{
  "scopes": {
    "write": { "tools": ["system", "web"], "skills": ["email-compose"] },
    "read": { "tools": ["web", "bot"], "skills": [] }
  }
}
```

### Dispatch

- `dispatch_chat()` parses `data["scope"]` from the WS message
- If non-empty, sets `ChatConfig.tool_scope = Some(scope)`
- Forwarded to `RunRequest.tool_scope` and applied during tool filtering in the runner

### Use Cases

- App SDK embeds restricting agent to read-only tools
- Workflow steps limiting available tools per stage
- Per-surface capability restriction (e.g., chat widget vs. full agent)

---

## 29. Unified Chat Controller (Frontend)

**File:** `app/src/lib/chat/controller.svelte.ts`

### Purpose

Single reactive controller for ALL chat surfaces (thread page, embed, web app).
Eliminates duplicated WS event handling across pages. Each surface creates a controller
instance and wires it to `ChatPane`; surface-specific logic (routing, history loading,
parent `postMessage`, A2UI) stays in the surface page.

### Factory

```typescript
createChatController(config: ChatControllerConfig): ChatController
```

**Config fields:**
- `agentId` — agent ID for event filtering
- `sessionKey?` — explicit session key (when set, events filtered by `session_id`; otherwise by `agentId`/`originAgentId`)
- `channel?` — channel for outbound messages (e.g., `"app"`, `"web"`)
- `onResponseComplete?` — callback when a response completes (e.g., embed `postMessage`)

### Managed State

| Property | Type | Description |
|----------|------|-------------|
| `messages` | `ChatMessage[]` | All messages (user, assistant, thinking, tool, ask) |
| `streamingContent` | `Record<string, string>` | Per-agent raw markdown buffers |
| `isLoading` | `boolean` | Active generation in progress |
| `tokenUsage` | `TokenUsage` | Input/output tokens + cache stats |
| `quotaWarning` | `string \| null` | Janus usage warning |
| `followupSuggestions` | `string[]` | Continuation chip suggestions |

### Handled WS Events

`chat_stream`, `chat_complete`, `thinking`, `tool_start`, `tool_result`, `tool_summary`,
`chat_error`, `chat_cancelled`, `usage`, `quota_warning`, `ask_request`,
`followup_suggestions`, `plan_approval`, `subagent_start`, `subagent_progress`,
`subagent_complete`

### Public API

| Method | Description |
|--------|-------------|
| `send(text, opts?)` | Send message (with optional `extraPayload`, `silent` flag) |
| `stop()` | Cancel active generation |
| `newThread()` | Reset for new conversation |
| `submitAsk(requestId, value)` | Respond to ask widget |
| `edit(msgId, newContent)` | Edit a user message |
| `redo(msgId)` | Re-run from a message |
| `prependMessages(msgs)` | Prepend history (e.g., from REST load) |

### Delegate Agent Support

Messages from @mentioned sub-agents are tagged with `delegateAgentId` / `delegateAgentName`.
Per-agent streaming buffers in `streamingContent` prevent text mixing when multiple agents
stream concurrently.

### Status Line Dedup

The controller strips prior `\n_Working..._\n` heartbeat lines before appending new ones,
preventing status line accumulation in streaming content.

---

## 30. App Sidecar Restore

**File:** `crates/server/src/lib.rs`

### Purpose

On server startup, previously-running app sidecars are automatically relaunched to prevent
user-visible breakage after a server restart.

### Mechanism

During server initialization (after agents are loaded):

1. Query `list_agents()` for all agents
2. Filter to enabled agents where `is_app == 1`
3. For each app agent, resolve `app_tool_dir()` from the agent's configuration
4. Create `AppLifecycle` instance and call `lifecycle.launch()`
5. Store successful lifecycles in `state.app_lifecycles` for runtime management
6. Log count of successfully launched sidecars

### Error Handling

- Individual sidecar launch failures are logged as warnings but do not block startup
- Other agents continue to be restored even if one fails

---

## Appendix: Initialization Order

```rust
// In crates/server/src/lib.rs:
let lanes = Arc::new(LaneManager::new());
lanes.start_pumps();

let concurrency = Arc::new(ConcurrencyController::new());
concurrency::spawn_monitor(concurrency.clone());

let runner = Arc::new(Runner::new(
    store, tools, providers, selector, concurrency,
    hooks, mcp_context, agent_registry, skill_loader,
).set_ask_channels(ask_channels));

let run_registry = RunRegistry::new();

// AppState wires everything together
```

### Hook System Summary

10 hook points wired (1 not):

| Hook | Type | When | Wired? |
|------|------|------|--------|
| `steering.generate` | Filter | Before steering pipeline | Yes |
| `message.pre_send` | Filter | Before LLM call | Yes |
| `message.post_receive` | Filter | After LLM response | Yes |
| `session.message_append` | Action | After message saved | Yes |
| `agent.turn` | Action | End of agentic loop turn | Yes |
| `agent.should_continue` | Filter | Before continuing loop | Yes |
| `tool.pre_execute` | Filter | Before tool execution | Yes |
| `tool.post_execute` | Action | After tool execution | Yes |
| `memory.extract` | Action | After message append | Yes |
| `prompt.assemble` | Filter | During prompt building | Yes |
| `response.stream` | Action | During streaming | **NOT WIRED** |
