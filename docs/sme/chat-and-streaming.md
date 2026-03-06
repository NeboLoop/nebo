# Chat and Streaming: Complete Logic Deep-Dive

**Source:** `internal/realtime/`, `internal/agenthub/`, `internal/db/session_manager.go`, `internal/agent/tools/agent_tool.go`, `cmd/nebo/agent.go`, `app/src/routes/(app)/agent/` | **Target:** `crates/server/src/handlers/ws.rs`, `crates/agent/src/runner.rs`, `crates/agent/src/session.rs`, `crates/db/src/queries/chats.rs` | **Status:** Draft

This document consolidates four Go SME docs -- AGENT_INPUT, CHAT_DISPLAY, CHAT_SYSTEMS, and WEBFORMS -- into a single comprehensive reference for the Rust rewrite. It covers the full chat pipeline from user keystroke to database persistence, including real-time streaming, content block assembly, interactive widgets, approval modals, session management, and channel chat.

---

## Table of Contents

1. [Architecture](#1-architecture)
2. [WebSocket Protocol](#2-websocket-protocol)
3. [Chat Message Pipeline](#3-chat-message-pipeline)
4. [Stream Processing](#4-stream-processing)
5. [Content Block System](#5-content-block-system)
6. [Ask Widget System](#6-ask-widget-system)
7. [Approval Modal System](#7-approval-modal-system)
8. [Message Persistence](#8-message-persistence)
9. [Session Management](#9-session-management)
10. [Barge-In and Message Queuing](#10-barge-in-and-message-queuing)
11. [Stream Resumption](#11-stream-resumption)
12. [Channel Chat](#12-channel-chat)
13. [Frontend Reference](#13-frontend-reference)
14. [Rust Implementation Status](#14-rust-implementation-status)

---

## 1. Architecture

**File(s):** `internal/realtime/hub.go`, `internal/agenthub/hub.go`, `internal/realtime/chat.go` (Go); `crates/server/src/handlers/ws.rs`, `crates/server/src/state.rs` (Rust)

### 1.1 Go: Two-Hub Architecture

The Go codebase separates agent-facing and browser-facing WebSocket management into two distinct hubs bridged by a `ChatContext` struct.

```
Frontend (Svelte 5)
  | browser WebSocket (/ws)
Client Hub (internal/realtime/hub.go)
  | event/response routing
ChatContext (internal/realtime/chat.go)
  | agent frames
Agent Hub (internal/agenthub/hub.go)
  | agent WebSocket (internal)
Agent (runner loop)
```

- **Client Hub** -- Manages N browser WebSocket connections. Broadcasts events to all connected clients. Each client has a buffered send channel (256 entries) and a write pump goroutine.
- **Agent Hub** -- Manages the ONE agent WebSocket connection. Routes frames by type: `req`, `res`, `stream`, `event`. Handles synchronous request/response correlation via a `pendingSync` map.
- **ChatContext** -- The bridge between the two hubs. Tracks pending requests in `map[requestID]*pendingRequest`, active sessions in `map[sessionID]string`, pending approvals in `map[approvalID]string`, and pending asks in `map[requestID]string`. Accumulates streamed content with UTF-8 boundary safety, builds `contentBlocks` incrementally, and handles title generation, introduction, and session reset.

### 1.2 Rust: Single Merged Hub

The Rust implementation collapses all three layers into a single WebSocket handler and a broadcast channel. The agent runs in-process -- there is no separate agent WebSocket connection.

```rust
// crates/server/src/handlers/ws.rs

pub struct ClientHub {
    tx: broadcast::Sender<HubEvent>,
}

pub struct HubEvent {
    pub event_type: String,
    pub payload: serde_json::Value,
}
```

The `ClientHub` provides two operations: `broadcast(event_type, payload)` sends to all subscribers, and `subscribe()` returns a `broadcast::Receiver<HubEvent>` for a new client.

Key differences from Go:

| Aspect | Go | Rust |
|--------|-----|------|
| Hub count | 2 (Agent Hub + Client Hub) | 1 (`ClientHub` with `broadcast::channel(256)`) |
| Bridge layer | `ChatContext` struct with maps and mutexes | None -- `dispatch_chat` directly streams via hub |
| Agent connection | Separate internal WebSocket | Agent code runs in-process (no WebSocket) |
| Stream accumulation | `pendingRequest` struct with UTF-8 tracking | Events forwarded directly to broadcast channel |
| Concurrency | Goroutines + `sync.Mutex` + Go channels | Tokio tasks + `CancellationToken` + `mpsc`/`broadcast`/`oneshot` |
| Per-client state | `Client` struct with buffered send channel | Each client has its own `broadcast::Receiver` |

### 1.3 AppState -- Shared Application State

The Rust `AppState` struct is passed to all Axum handlers via extractors. It holds all shared resources:

```rust
// crates/server/src/state.rs

pub struct AppState {
    pub config: Config,
    pub store: Arc<Store>,
    pub auth: Arc<AuthService>,
    pub hub: Arc<ClientHub>,
    pub runner: Arc<Runner>,
    pub tools: Arc<Registry>,
    pub bridge: Arc<mcp::Bridge>,
    pub models_config: Arc<config::ModelsConfig>,
    pub cli_statuses: Arc<config::AllCliStatuses>,
    pub lanes: Arc<LaneManager>,
    pub snapshot_store: Arc<browser::SnapshotStore>,
    pub approval_channels: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,
    pub ask_channels: Arc<Mutex<HashMap<String, oneshot::Sender<String>>>>,
}
```

The `approval_channels` and `ask_channels` fields are the Rust equivalents of Go's `pendingApproval` and `pendingAsk` maps on `agentState`. They use `tokio::sync::oneshot` channels instead of Go's buffered channels.

### 1.4 Connection Lifecycle

The Rust WebSocket handler `handle_client_ws` runs a `tokio::select!` loop that simultaneously:

1. Receives hub events from the broadcast channel and forwards them to the client
2. Receives messages from the client WebSocket and dispatches them

```rust
// crates/server/src/handlers/ws.rs -- simplified

async fn handle_client_ws(mut socket: WebSocket, state: AppState) {
    let mut hub_rx = state.hub.subscribe();
    let active_runs: ActiveRuns = Arc::new(Mutex::new(HashMap::new()));

    // Send welcome frame
    socket.send(Message::Text(json!({"type": "connected", "version": ...}).to_string())).await;

    loop {
        tokio::select! {
            result = hub_rx.recv() => { /* forward event to client */ }
            Some(msg) = socket.recv() => { /* dispatch client message */ }
        }
    }
}
```

When the broadcast channel lags (client too slow), the Rust implementation logs a warning and continues. The Go implementation drops messages if the client's 256-entry send buffer is full.

---

## 2. WebSocket Protocol

**File(s):** `internal/realtime/chat.go`, `internal/realtime/client.go` (Go); `crates/server/src/handlers/ws.rs` (Rust); `app/src/lib/websocket/client.ts` (frontend)

### 2.1 Connection

The browser connects to `/ws`. Go adds `?clientId={uuid}&userId={userId}` query params; Rust currently has no query params. On connect, the server sends a welcome frame:

```json
{"type": "connected", "version": "0.1.0"}
```

The client may send `auth` or `connect` messages, to which the server responds:

```json
{"type": "auth_ok"}
```

### 2.2 Frame Format

All frames are JSON objects with a top-level `type` field:

```json
{
  "type": "event_type",
  "data": { ... }
}
```

The Go backend may batch multiple JSON messages per WebSocket frame, separated by newlines. The frontend client splits on `\n` and processes each line independently. The Rust backend sends one JSON object per frame.

### 2.3 Client-to-Server Message Types

| Type | Payload | Purpose |
|------|---------|---------|
| `chat` | `{session_id, prompt, companion?, user_id?, system?, channel?}` | Send user message to agent |
| `cancel` | `{session_id}` | Cancel active agent run |
| `ping` | `{}` | Keep-alive (server replies with `pong`) |
| `session_reset` | `{session_id}` | Wipe all messages for session |
| `check_stream` | `{session_id}` | Check if a stream is active (page load resumption) |
| `approval_response` | `{tool_call_id, approved}` (Rust) / `{request_id, approved, always?}` (Go) | User responds to tool approval |
| `ask_response` | `{question_id, answer}` (Rust) / `{request_id, value}` (Go) | User responds to ask widget |
| `request_introduction` | `{session_id}` | Request agent introduction for empty chat |
| `auth` / `connect` | `{}` | Authentication handshake |

### 2.4 Server-to-Client Event Types (13)

All 13 event handlers are registered in the frontend's `onMount()`:

| Event | Payload | Purpose |
|-------|---------|---------|
| `chat_stream` | `{session_id, content}` | Text delta (partial streaming chunk) |
| `chat_complete` | `{session_id}` | Response finished -- frontend stops loading |
| `tool_start` | `{session_id, tool_call_id, tool_name, input}` | Tool execution started |
| `tool_result` | `{session_id, tool_call_id, tool_name, content, is_error?}` | Tool execution completed |
| `image` | `{session_id, image_url}` | Image produced by tool |
| `thinking` | `{session_id, content}` | Extended thinking content |
| `error` | `{session_id, error}` | LLM or agent error |
| `approval_request` | `{session_id, tool_call_id, tool_name, args}` | Tool needs user approval |
| `ask_request` | `{session_id, question_id, prompt}` (Rust) / `{request_id, prompt, widgets}` (Go) | Agent asks user a question |
| `stream_status` | `{session_id, status, content?}` | Response to `check_stream` |
| `chat_cancelled` | `{session_id}` | Cancel confirmed |
| `reminder_complete` | `{session_id}` | Scheduled task completed |
| `dm_user_message` | `{session_id, content, ...}` | DM from NeboLoop owner |

### 2.5 Go-Only Internal Frame Types (Agent Hub)

The Go Agent Hub uses a separate frame format for agent-to-server communication. These do NOT exist in the Rust implementation because the agent runs in-process:

```json
{
  "type": "req|res|stream|event|approval_response|ask_response",
  "id": "correlation-id",
  "method": "run|introduce|cancel|generate_title",
  "params": {},
  "ok": true,
  "payload": {},
  "error": ""
}
```

| Frame Type | Direction | Purpose |
|------------|-----------|---------|
| `req` | Server -> Agent | Request agent action |
| `res` | Agent -> Server | Final response (complete) |
| `stream` | Agent -> Server | Streaming chunk (incremental) |
| `event` | Agent -> Server | Broadcast event (lane updates, DMs) |
| `approval_response` | Browser -> Agent | User approves tool execution |
| `ask_response` | Browser -> Agent | User provides input |

### 2.6 Stream Chunk Payload Keys (Go)

The agent sends `"stream"` frames with various payload keys that are NOT mutually exclusive -- a single frame can carry multiple keys:

| Payload Key | Type | Purpose |
|-------------|------|---------|
| `chunk` | string | Text being generated |
| `tool` + `tool_id` + `input` | string | Tool execution starting |
| `tool_result` + `tool_id` + `tool_name` | string | Tool completed |
| `image_url` | string | Screenshot/image produced |
| `thinking` | string | Extended thinking content |

---

## 3. Chat Message Pipeline

**File(s):** `internal/realtime/chat.go` (Go); `crates/server/src/handlers/ws.rs`, `crates/agent/src/runner.rs` (Rust)

### 3.1 End-to-End Flow

```
1. USER TYPES MESSAGE
   ChatInput.svelte -> textarea value bound via $bindable()

2. USER SENDS (Enter or click)
   If isLoading: barge-in (cancel + send new) or queue
   If idle: add optimistic user Message to messages[], set isLoading=true
   WebSocket send: {type: "chat", data: {session_id, prompt, companion: true}}

3. SERVER RECEIVES (ws handler)
   Go:   Client Hub -> ChatContext.handleChatMessage()
   Rust: handle_client_ws -> dispatch_chat()

4. SESSION SETUP
   Go:   waitForAgent(5s), GetOrCreateCompanionChat, create pendingRequest,
         track in activeSessions, save user message to chat_messages
   Rust: Runner internally resolves session via SessionManager.get_or_create()

5. AGENT PROCESSES (agentic loop)
   Go:   Send "run" frame to Agent Hub -> agent picks up -> runner.Run()
   Rust: runner.run(RunRequest) returns mpsc::Receiver<StreamEvent>

   Loop:
   a. Append user message to chat_messages (single write path)
   b. Build context (system prompt, history, memories, steering messages)
   c. Call LLM provider (stream mode)
   d. Process streaming events: text chunks, tool calls, tool results
   e. Execute tools when requested (may trigger approval/ask blocking)
   f. Append assistant message to chat_messages
   g. Repeat if tool results require further LLM processing

6. STREAM EVENTS -> HUB -> BROWSER
   Go:   ChatContext processes stream frames, accumulates content,
         builds contentBlocks, broadcasts via Client Hub
   Rust: dispatch_chat reads from mpsc::Receiver, broadcasts via ClientHub

7. FRONTEND HANDLES EVENTS
   chat_stream   -> append delta to currentStreamingMessage.content
   tool_start    -> append ToolCall, append tool contentBlock
   tool_result   -> update ToolCall output/status, maybe append image block
   thinking      -> accumulate on currentStreamingMessage.thinking
   chat_complete -> isLoading=false, finalize message, process queue

8. AGENT COMPLETES
   Go:   ChatContext flushes remaining buffered text, updates chat timestamp,
         triggers title generation for new chats, sends chat_complete
   Rust: dispatch_chat loop ends, broadcasts chat_complete, removes from active_runs
```

### 3.2 Go: handleChatMessage() Detail

**File(s):** `internal/realtime/chat.go`

1. Wait up to 5s for agent to connect (`waitForAgent`)
2. Get or create companion chat session via `GetOrCreateCompanionChat(chatID, userID)`
3. Save user message to `chat_messages` DB table
4. Create `pendingRequest` tracked in `chatCtx.pending[requestID]`
5. Track in `chatCtx.activeSessions[sessionID] = requestID`
6. Send frame to agent via Agent Hub:

```json
{
  "type": "req",
  "id": "chat-1234567890",
  "method": "run",
  "params": {"session_key": "uuid", "prompt": "hello", "user_id": "..."}
}
```

### 3.3 Rust: dispatch_chat() Detail

**File(s):** `crates/server/src/handlers/ws.rs`

```rust
async fn dispatch_chat(state: &AppState, msg: &serde_json::Value, active_runs: ActiveRuns) {
    let data = &msg["data"];
    let session_id = data["session_id"].as_str().unwrap_or("default").to_string();
    let prompt = data["prompt"].as_str().unwrap_or("").to_string();
    // ... extract system, user_id, channel

    if prompt.is_empty() {
        state.hub.broadcast("chat_error", json!({"error": "empty prompt", "session_id": sid}));
        return;
    }

    let cancel_token = CancellationToken::new();
    active_runs.lock().await.insert(sid.clone(), cancel_token.clone());

    // Broadcast chat_created so frontend can track new conversations
    hub.broadcast("chat_created", json!({"session_id": sid, "channel": channel}));

    // Route through lane system for concurrency control
    let lane_task = make_task(lanes::MAIN, format!("chat:{}", sid), async move {
        let req = RunRequest {
            session_key: sid.clone(),
            prompt,
            system,
            user_id,
            channel,
            origin: Origin::User,
            ..Default::default()
        };

        match runner.run(req).await {
            Ok(mut rx) => {
                loop {
                    let event = tokio::select! {
                        _ = cancel_token.cancelled() => {
                            hub.broadcast("chat_cancelled", json!({"session_id": sid}));
                            break;
                        }
                        ev = rx.recv() => match ev {
                            Some(e) => e,
                            None => break,
                        }
                    };
                    match event.event_type {
                        StreamEventType::Text => {
                            hub.broadcast("chat_stream", json!({"session_id": sid, "content": event.text}));
                        }
                        StreamEventType::ToolCall => {
                            if let Some(ref tc) = event.tool_call {
                                hub.broadcast("tool_start", json!({
                                    "session_id": sid, "tool_call_id": tc.id,
                                    "tool_name": tc.name, "input": tc.input,
                                }));
                            }
                        }
                        StreamEventType::ToolResult => { /* broadcast tool_result */ }
                        StreamEventType::Thinking => { /* broadcast thinking */ }
                        StreamEventType::Error => { /* broadcast chat_error */ }
                        StreamEventType::ApprovalRequest => { /* broadcast approval_request */ }
                        StreamEventType::AskRequest => { /* broadcast ask_request */ }
                        StreamEventType::Usage => { /* broadcast usage */ }
                        StreamEventType::Done => { /* handled after loop */ }
                    }
                }
                // Always send chat_complete when stream ends
                hub.broadcast("chat_complete", json!({"session_id": sid}));
            }
            Err(e) => {
                hub.broadcast("chat_error", json!({"session_id": sid, "error": e.to_string()}));
                hub.broadcast("chat_complete", json!({"session_id": sid}));
            }
        }
        active_runs_cleanup.lock().await.remove(&sid);
        Ok(())
    });
    state.lanes.enqueue_async(lanes::MAIN, lane_task);
}
```

### 3.4 RunRequest and StreamEventType

```rust
// crates/agent/src/runner.rs
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
}

// crates/ai/src/types.rs
pub enum StreamEventType {
    Text,            // Text delta
    ToolCall,        // Tool invocation started
    ToolResult,      // Tool execution completed
    Error,           // LLM or agent error
    Done,            // Stream finished
    Thinking,        // Extended thinking content
    Usage,           // Token usage statistics
    ApprovalRequest, // Tool needs user approval
    AskRequest,      // Agent asks user a question
}
```

### 3.5 StreamEvent Struct

```rust
// crates/ai/src/types.rs
pub struct StreamEvent {
    pub event_type: StreamEventType,
    pub text: String,
    pub tool_call: Option<ToolCall>,
    pub error: Option<String>,
    pub usage: Option<UsageInfo>,
}
```

The `AskRequest` variant reuses the `error` field for `question_id` and `text` for the prompt. This is a pragmatic reuse rather than adding a dedicated field.

---

## 4. Stream Processing

**File(s):** `internal/realtime/chat.go` (Go); `crates/server/src/handlers/ws.rs` (Rust)

### 4.1 Go: Pending Request Tracking

The Go ChatContext maintains per-request state for every in-flight chat:

```go
type pendingRequest struct {
    client          *Client       // Browser connection to respond to
    sessionID       string        // Chat ID
    prompt          string        // Original user message (empty = title gen)
    isNewChat       bool          // Triggers title generation on complete
    streamedContent string        // Accumulated text from all chunks
    cleanSentLen    int           // UTF-8 boundary tracker (bytes sent so far)
    toolCalls       []toolCallInfo
    thinking        string
    contentBlocks   []contentBlock
    messageID       string        // DB message UUID
}
```

This struct is the heart of Go's stream processing. The Rust implementation has NO equivalent -- events are forwarded without accumulation.

### 4.2 Go: UTF-8 Boundary Protection

CRITICAL for emoji/CJK character integrity. Multi-byte characters can be split across streaming chunks from the LLM provider. The Go code prevents sending partial runes to the browser:

```go
// Hold back 20 chars (fence markers are 18 chars)
safeLen := len(clean) - 20
// Back up to UTF-8 rune boundary
for safeLen > req.cleanSentLen && !utf8.RuneStart(clean[safeLen]) {
    safeLen--
}
// Send only the delta since last send
delta = clean[req.cleanSentLen:safeLen]
req.cleanSentLen = safeLen
```

On completion (`"res"` frame), the remaining held-back 20 chars are flushed.

### 4.3 Go: Fence Marker Stripping

LLMs sometimes emit markdown fence markers (` ``` `) that should NOT appear in the streamed output. The function `afv.StripFenceMarkers(req.streamedContent)` removes them. Because fence markers are 18 characters, the 20-char holdback buffer ensures partial markers do NOT leak to the frontend.

### 4.4 Go: Processing Order per Stream Frame

For each `"stream"` frame:

1. **Text chunks:** Accumulate in `req.streamedContent`, strip fence markers, hold back 20 chars, back up to UTF-8 boundary, calculate delta (new chars since last send), append to last text content block (or create new), send `chat_stream` event.
2. **Tool start:** Flush held-back text buffer first (so text appears before tool card), track tool call in `req.toolCalls[]`, append tool content block, send `tool_start` event.
3. **Tool result:** Update matching tool call (by ID or first running), if tool produced an image append image content block, send `tool_result` and optionally `image` events.
4. **Thinking:** Accumulate in `req.thinking`, send `thinking` event.

For `"res"` frames (final):
1. Remove from `pending` and `activeSessions`
2. Flush remaining buffered content (the held-back 20 chars)
3. Update chat timestamp; message persistence is handled by the runner (single write path)
4. If new chat: trigger title generation (separate agent call with empty prompt)
5. Send `chat_complete` event

### 4.5 Go: Tool Result Matching

Two-pass matching strategy in ChatContext:

```go
// 1. Try by tool_id (exact match)
for i := range req.toolCalls {
    if req.toolCalls[i].ID == toolID && toolID != "" {
        req.toolCalls[i].Output = toolResult
        updated = true; break
    }
}
// 2. Fallback: first "running" tool (if ID not found or empty)
if !updated {
    for i := range req.toolCalls {
        if req.toolCalls[i].Status == "running" {
            req.toolCalls[i].Output = toolResult; break
        }
    }
}
```

### 4.6 Rust: Direct Event Forwarding

The Rust implementation forwards `StreamEvent` values directly from the runner's `mpsc::Receiver` to the broadcast hub. There is NO server-side accumulation, fence-stripping, UTF-8 boundary tracking, or content block building. This works because:

- Rust `String` types are always valid UTF-8 by construction
- The AI provider implementations emit complete UTF-8 strings per chunk
- The frontend handles content block assembly
- Tool result matching is delegated to the frontend

**Gaps to address:**

| Gap | Impact | Priority |
|-----|--------|----------|
| No fence marker stripping | LLM fence markers may leak to UI | Medium |
| No content accumulation | `stream_status` cannot replay missed content | Medium |
| No server-side contentBlock building | `metadata` column stays NULL | Low (frontend handles it) |
| No text holdback buffer | Rare: partial multi-byte if provider sends raw bytes | Low |

---

## 5. Content Block System

**File(s):** `internal/realtime/chat.go` (Go); `app/src/routes/(app)/agent/+page.svelte`, `app/src/lib/components/chat/MessageGroup.svelte` (frontend)

### 5.1 Content Block Types (4)

Messages carry structured `contentBlocks[]` for interleaved rendering of different content types:

| Type | Key Fields | Renders As |
|------|-----------|------------|
| `text` | `text` | Markdown prose via `Markdown.svelte` |
| `tool` | `toolCallIndex` (index into parent `toolCalls[]`) | `ToolCard.svelte` (icon, status, clickable) |
| `image` | `imageData`, `imageMimeType`, `imageURL` | `<img>` tag (base64 inline or server URL, zoom on click) |
| `ask` | `askRequestId`, `askPrompt`, `askWidgets[]`, `askResponse` | `AskWidget.svelte` (interactive controls) |

### 5.2 TypeScript Interfaces

```typescript
interface Message {
    id: string;
    role: 'user' | 'assistant' | 'system';
    content: string;              // Full accumulated text
    contentHtml?: string;         // Pre-rendered HTML (from DB on load)
    timestamp: Date;
    toolCalls?: ToolCall[];
    streaming?: boolean;          // Currently being streamed
    thinking?: string;            // Extended thinking content
    contentBlocks?: ContentBlock[];
}

interface ContentBlock {
    type: 'text' | 'tool' | 'image' | 'ask';
    text?: string;
    toolCallIndex?: number;       // Index into toolCalls[]
    imageData?: string;           // Base64 for inline images
    imageMimeType?: string;
    imageURL?: string;            // Server URL (/api/v1/files/...)
    askRequestId?: string;
    askPrompt?: string;
    askWidgets?: AskWidgetDef[];
    askResponse?: string;         // Filled after user submits
}

interface ToolCall {
    id?: string;
    name: string;
    input: string;                // JSON string
    output?: string;              // Result text
    status?: 'running' | 'complete' | 'error';
}
```

### 5.3 Content Block Assembly During Streaming

Text chunks append to the last text block or create a new one:

```typescript
if (blocks.length === 0 || blocks[blocks.length - 1].type !== 'text') {
    blocks.push({ type: 'text', text: chunk });
} else {
    blocks[blocks.length - 1].text += chunk;
}
```

Tool starts insert a new tool block with a `toolCallIndex` pointing into the parent message's `toolCalls[]` array. Tool results update the matching tool call's `output` and `status`. Image blocks are appended after tool results when `imageURL` is present. Ask blocks are appended when `ask_request` events arrive.

The Go backend (`chat.go`) mirrors this EXACT pattern -- maintaining `req.contentBlocks` for DB persistence in the `metadata` JSON column.

### 5.4 Pre-Resolution in MessageGroup.svelte

The `MessageGroup.svelte` component pre-resolves all blocks into a flat `ResolvedBlock[]` array to avoid multi-level lookups during Svelte 5 reactivity:

```typescript
interface ResolvedBlock {
    type: 'text' | 'tool' | 'image' | 'ask';
    key: string;                 // Svelte keyed block: "tool-0-running"
    text?: string;
    tool?: ToolCall;             // Pre-resolved (not an index)
    imageData?: string;
    imageMimeType?: string;
    imageURL?: string;
    askRequestId?: string;
    askPrompt?: string;
    askWidgets?: AskWidgetDef[];
    askResponse?: string;
    isLastBlock: boolean;        // Used for streaming cursor animation
}
```

### 5.5 Rendering Order

1. Thinking block (if present, collapsed by default via `ThinkingBlock.svelte`)
2. Content blocks in order (text/tool/image/ask interleaved)
3. Legacy fallback: messages without contentBlocks render as simple text + tool list
4. Footer: sender name + timestamp

### 5.6 Server-Side Content Block Persistence (Go)

On stream completion, `buildMetadata()` serializes all contentBlocks (including ask blocks with their `askResponse`) into the `metadata` JSON column on `chat_messages`:

```json
{
    "toolCalls": [...],
    "thinking": "...",
    "contentBlocks": [
        {"type": "text", "text": "Here is the result:"},
        {"type": "tool", "toolCallIndex": 0},
        {"type": "text", "text": "The file was updated."},
        {"type": "ask", "askRequestId": "abc", "askPrompt": "Continue?", "askResponse": "Yes"}
    ]
}
```

Separate `tool_calls` and `tool_results` columns exist for easier querying by the runner context builder.

---

## 6. Ask Widget System

**File(s):** `internal/agent/tools/agent_tool.go` (Go); `cmd/nebo/agent.go` (Go); `crates/server/src/handlers/ws.rs` (Rust); `app/src/lib/components/chat/AskWidget.svelte` (frontend)

### 6.1 Widget Types (6)

| Type | Renders As | Options Field | Default Behavior |
|------|-----------|---------------|------------------|
| `buttons` | Row of outlined buttons | Required -- each option is a button | N/A |
| `confirm` | Row of buttons (like buttons) | Optional -- defaults to `["Yes", "No"]` | Yes/No buttons |
| `select` | Dropdown + OK button | Required -- each option is a `<option>` | Disabled OK until selection |
| `radio` | Vertical radio list + Submit button | Required -- each option is a radio input | Disabled Submit until one selected |
| `checkbox` | Vertical checkbox list + Submit button | Required -- each option is a checkbox | Disabled Submit until >=1 checked; submits comma-separated |
| `text_input` | Text field + Submit button | N/A | Free-form text input |

### 6.2 Go Data Structures

```go
// internal/agent/tools/agent_tool.go
type AskWidget struct {
    Type    string   `json:"type"`              // "buttons", "select", "confirm", "radio", "checkbox"
    Label   string   `json:"label,omitempty"`
    Options []string `json:"options,omitempty"`
    Default string   `json:"default,omitempty"` // Pre-filled value
}

type AskCallback func(ctx context.Context, requestID, prompt string, widgets []AskWidget) (string, error)
```

TypeScript mirror:

```typescript
export interface AskWidgetDef {
    type: 'buttons' | 'select' | 'confirm' | 'radio' | 'checkbox';
    label?: string;
    options?: string[];
    default?: string;
}
```

### 6.3 End-to-End Ask Flow

```
Agent LLM decides to ask user
  |
  v
agent(resource: message, action: ask, prompt: "Which option?", widgets: [...])
  |
  v
AgentDomainTool.messageAsk()              -- validates prompt, defaults to confirm(Yes/No)
  | Generates UUID requestID
  | Calls askCallback(ctx, requestID, prompt, widgets)
  |
  v
Go:   agentState.requestAsk()            -- creates chan string (capacity 1), blocks
Rust: runner emits StreamEvent::AskRequest, awaits oneshot::Receiver
  |
  v
Hub broadcasts: {type: "ask_request", request_id/question_id, prompt, widgets?}
  |
  v
Browser: handleAskRequest()              -- sets pendingAskRequest = true
  | Appends ask contentBlock to currentStreamingMessage.contentBlocks
  |
  v
MessageGroup.svelte resolves block -> AskWidget.svelte renders
  | Shows prompt text + widget(s) based on type
  | User interacts (clicks button / selects option / types text)
  | Calls submit(value)
  |
  v
Browser: handleAskSubmit()
  | Sends: client.send('ask_response', {request_id/question_id, value/answer})
  | Updates contentBlock.askResponse locally
  | Updates messages array for non-streaming messages
  |
  v
Go:   ChatContext.handleAskResponse() -> Hub.SendAskResponse() -> channel unblocks
Rust: ws handler removes oneshot::Sender from ask_channels, sends answer
  |
  v
Agent continues with user's answer as ToolResult
```

### 6.4 Blocking Channel Pattern

**Go approach:**

```go
// agent.go -- creates buffered channel, blocks goroutine
respCh := make(chan string, 1)
state.pendingAsk[requestID] = respCh
// Send ask_request frame to hub...
select {
case resp := <-respCh:
    return resp, nil
case <-ctx.Done():
    return "", ctx.Err()
}
```

**Rust approach:**

```rust
// On ask_request: runner creates oneshot, stores sender in state.ask_channels
// On ask_response: handler removes sender, sends answer
"ask_response" => {
    let question_id = parsed["data"]["question_id"].as_str().unwrap_or("").to_string();
    let answer = parsed["data"]["answer"].as_str().unwrap_or("").to_string();
    let mut channels = state.ask_channels.lock().await;
    if let Some(tx) = channels.remove(&question_id) {
        let _ = tx.send(answer);
    }
}
```

### 6.5 Ask Persistence

Ask widgets are persisted via `contentBlocks` in the `metadata` JSON column on `chat_messages`. On page reload, the frontend parses metadata, extracts contentBlocks, and renders answered ask widgets as read-only badges showing the `askResponse` value.

### 6.6 Error Handling and Edge Cases

| Scenario | Behavior |
|----------|----------|
| No web UI connected | Go: returns error "Interactive prompts require the web UI" |
| Empty prompt | Returns error: prompt is required |
| No widgets specified | Defaults to `confirm` with `["Yes", "No"]` |
| Context cancelled (timeout) | Returns `ctx.Err()` (Go) / `Cancelled` (Rust) |
| Browser disconnects mid-ask | Blocking task waits until context/token cancellation |
| Multiple pending requests | Server iterates pending map, appends ask block to first active request |
| Duplicate response | Go: buffered channel (capacity 1) + default case prevents goroutine leak |

### 6.7 Tool Invocation Pattern

The agent (LLM) invokes the ask widget via the STRAP agent tool:

```
agent(resource: message, action: ask, prompt: "Which option do you prefer?", widgets: [
  {type: "buttons", label: "Choose one:", options: ["Option A", "Option B", "Option C"]}
])
```

Default when no widgets specified (simple confirmation):

```
agent(resource: message, action: ask, prompt: "Should I proceed?")
  -> defaults to: widgets: [{type: "confirm", options: ["Yes", "No"]}]
```

---

## 7. Approval Modal System

**File(s):** `internal/agent/tools/policy.go`, `cmd/nebo/agent.go` (Go); `crates/server/src/handlers/ws.rs` (Rust); `app/src/lib/components/ui/ApprovalModal.svelte` (frontend)

### 7.1 Policy-Driven Approval

Tools may require user approval before execution, determined by `Policy.RequiresApproval()`. Policy levels:

| Level | Behavior |
|-------|----------|
| `PolicyDeny` | Block all dangerous tools |
| `PolicyAllowlist` | Allow whitelisted commands, prompt for others (default) |
| `PolicyFull` | Allow all tools without approval |

Safe bins (always allowed without approval): `ls`, `pwd`, `cat`, `grep`, `find`, `git status`, `git log`, `git diff`, `go`/`node`/`python --version`.

### 7.2 Approval Flow

```
Agent executes tool (e.g., shell command)
  |
  v
Registry.Execute()                     -- checks policy.RequiresApproval(cmd)
  | If yes: calls policy.ApprovalCallback(ctx, toolName, input)
  |
  v
Go:   agentState.requestApproval()     -- creates chan approvalResponse, blocks
Rust: runner emits StreamEvent::ApprovalRequest, awaits oneshot::Receiver<bool>
  |
  v
Hub broadcasts: {type: "approval_request", tool_call_id/request_id, tool_name, args/input}
  |
  v
Browser: ApprovalModal renders         -- shows tool name + formatted input
  | Three buttons: Deny / Once / Always
  |
  v
Browser sends: {type: "approval_response", tool_call_id/request_id, approved, always?}
  |
  v
Go:   channel unblocks with approvalResponse{Approved, Always}
      If Always=true: adds command to runtime allowlist (NOT persisted to disk)
Rust: oneshot sender sends bool (true/false)
  |
  v
Tool execution proceeds (approved=true) or is rejected (approved=false)
```

### 7.3 Go Data Structures

```go
type approvalResponse struct {
    Approved bool
    Always   bool
}

type pendingApprovalInfo struct {
    RespCh   chan approvalResponse
    ToolName string
    Input    json.RawMessage
}

// agent.go -- agentState fields
pendingApproval map[string]*pendingApprovalInfo  // requestID -> info
approvalMu      sync.RWMutex
```

### 7.4 Rust Implementation

```rust
// crates/server/src/state.rs
pub approval_channels: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,

// crates/server/src/handlers/ws.rs
"approval_response" => {
    let tool_call_id = parsed["data"]["tool_call_id"].as_str().unwrap_or("").to_string();
    let approved = parsed["data"]["approved"].as_bool().unwrap_or(false);
    let mut channels = state.approval_channels.lock().await;
    if let Some(tx) = channels.remove(&tool_call_id) {
        let _ = tx.send(approved);
    }
}
```

### 7.5 ApprovalModal.svelte

The modal overlay renders with three action buttons:

- **Deny** -- Rejects tool execution
- **Once** -- Approves this specific execution only
- **Always** -- Approves and adds to runtime allowlist

Shows formatted tool name and input: bash commands display the command string, other tools show path or JSON.

Approvals are queued in `approvalQueue[]` and shown one at a time.

### 7.6 Persistence

Approval state is NOT persisted. It lives entirely in-memory. If the browser disconnects mid-approval, the approval is lost and the blocking task eventually times out.

The "Always" allowlist (Go only) survives for the current session but is NOT written to disk. The Rust implementation does NOT yet support "Always" -- approvals are simple `bool` values.

---

## 8. Message Persistence

**File(s):** `internal/db/session_manager.go`, `internal/db/queries/chats.sql` (Go); `crates/agent/src/session.rs`, `crates/db/src/queries/chats.rs` (Rust)

### 8.1 Single Write Path

All message persistence flows through a single path:

```
Runner -> SessionManager.append_message() -> chat_messages table
```

This is a CRITICAL design invariant. The ChatContext/hub does NOT save messages. The frontend does NOT save messages. Only the runner writes to the database through the SessionManager. If the runner crashes before saving, streaming content is lost (but session state survives via SQLite).

### 8.2 Database Schema

**chats table:**

```sql
CREATE TABLE chats (
    id TEXT PRIMARY KEY,                    -- sessionKey (e.g., "companion-default")
    title TEXT NOT NULL DEFAULT 'New Chat',
    user_id TEXT,                           -- companion mode: one chat per user
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);
-- UNIQUE index on user_id WHERE user_id IS NOT NULL
```

**chat_messages table:**

```sql
CREATE TABLE chat_messages (
    id TEXT PRIMARY KEY,                    -- UUID generated per message
    chat_id TEXT NOT NULL,                  -- FK to chats.id (= sessionKey)
    role TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'system', 'tool')),
    content TEXT NOT NULL,
    metadata TEXT,                          -- JSON: {toolCalls, thinking, contentBlocks}
    tool_calls TEXT,                        -- JSON array of tool invocations
    tool_results TEXT,                      -- JSON array of tool results
    token_estimate INTEGER,                 -- Token count for context budgeting
    day_marker TEXT,                        -- ISO date (YYYY-MM-DD) for history browsing
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    FOREIGN KEY (chat_id) REFERENCES chats(id) ON DELETE CASCADE
);
-- Indexes: chat_id, created_at, (chat_id, day_marker)
```

### 8.3 Rust Models

```rust
// crates/db/src/models.rs

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
    pub role: String,
    pub content: String,
    pub metadata: Option<String>,
    pub created_at: i64,
    pub day_marker: Option<String>,
    pub tool_calls: Option<String>,
    pub tool_results: Option<String>,
    pub token_estimate: Option<i64>,
}
```

### 8.4 Rust SessionManager.append_message()

```rust
// crates/agent/src/session.rs

pub fn append_message(
    &self,
    session_id: &str,
    role: &str,
    content: &str,
    tool_calls: Option<&str>,
    tool_results: Option<&str>,
) -> Result<ChatMessage, NeboError> {
    // 1. Skip truly empty messages
    if content.is_empty()
        && tool_calls.map_or(true, |tc| tc.is_empty() || tc == "[]" || tc == "null")
        && tool_results.map_or(true, |tr| tr.is_empty() || tr == "[]" || tr == "null")
    {
        return Err(NeboError::Validation("empty message".to_string()));
    }

    // 2. Resolve session_id -> chat_id via cache
    let chat_id = self.resolve_chat_id(session_id);
    let msg_id = uuid::Uuid::new_v4().to_string();

    // 3. Estimate tokens (chars / 4 heuristic)
    let token_estimate = estimate_tokens(content, tool_calls, tool_results);

    // 4. Write to chat_messages
    let msg = self.store.create_chat_message_for_runner(
        &msg_id, &chat_id, role, content, tool_calls, tool_results, Some(token_estimate),
    )?;

    // 5. Increment session message count
    let _ = self.store.increment_session_message_count(session_id);

    Ok(msg)
}
```

### 8.5 Orphan Sanitization

When messages are loaded via `get_messages()`, the `sanitize_messages()` function strips tool messages whose `tool_results` reference `tool_call_id` values that do NOT match any assistant message's `tool_calls`. This prevents display artifacts from partial saves or interrupted runs.

```rust
fn sanitize_messages(messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
    // Collect all tool_call IDs from assistant messages
    let mut known_call_ids = HashSet::new();
    for msg in &messages {
        if msg.role == "assistant" {
            if let Some(ref tc_json) = msg.tool_calls {
                // Parse JSON, extract IDs
            }
        }
    }
    // Filter: keep non-tool messages; for tool messages, verify references
    messages.into_iter().filter(|msg| { ... }).collect()
}
```

### 8.6 Key Queries

| Operation | Go | Rust |
|-----------|-----|------|
| Upsert companion chat | `GetOrCreateCompanionChat(id, user_id)` | `store.get_or_create_companion_chat(id, user_id)` |
| Insert message (runner) | `CreateChatMessageForRunner(...)` | `store.create_chat_message_for_runner(...)` |
| Get all messages | `GetChatMessages(chat_id)` | `store.get_chat_messages(chat_id)` |
| Recent with tools | `GetRecentChatMessagesWithTools(chat_id, limit)` | `store.get_recent_chat_messages_with_tools(chat_id, limit)` |
| Delete all messages | `DeleteChatMessagesByChatId(chat_id)` | `store.delete_chat_messages_by_chat_id(chat_id)` |
| Update title | `UpdateChatTitle(id, title)` | `store.update_chat_title(id, title)` |
| Search | `SearchChatMessages(chat_id, query, limit, offset)` | `store.search_chat_messages(chat_id, query, limit, offset)` |
| Messages by day | `GetMessagesByDay(chat_id, day_marker)` | `store.get_chat_messages_by_day(chat_id, day)` |
| Day listing | Custom | `store.list_chat_days(chat_id, limit, offset)` |
| Count | `CountChatMessages(chat_id)` | `store.count_chat_messages(chat_id)` |
| Delete after timestamp | N/A | `store.delete_chat_messages_after(chat_id, created_at)` |

---

## 9. Session Management

**File(s):** `internal/db/session_manager.go`, `internal/agent/session/keyparser.go` (Go); `crates/agent/src/session.rs`, `crates/agent/src/keyparser.rs` (Rust)

### 9.1 Key Mapping: Session ID vs Session Key vs Chat ID

```
Runner works with:     session_key (string from RunRequest, e.g. "companion-default")
                            | resolved via SessionManager.get_or_create()
SessionManager maps:   session_id (UUID from sessions.id) <-> session_key (sessions.name)
                            | session_key used as
chat_messages stores:  chat_id = session_key
```

The `SessionManager` caches the `session_id -> session_key` mapping in memory to avoid per-message DB lookups:

- **Go:** `sync.Map`
- **Rust:** `Arc<RwLock<HashMap<String, String>>>`

### 9.2 Session Key Naming Conventions

| Pattern | Meaning | Example |
|---------|---------|---------|
| `companion-default` | Web UI companion chat | `companion-default` |
| `dm-{conversationID}` | External DM session | `dm-abc123` |
| `subagent-{uuid}` | Sub-agent session | `subagent-550e8400` |
| `{channel}:group:{id}` | Group chat | `discord:group:12345` |
| `{channel}:channel:{id}` | Channel session | `slack:channel:general` |
| `{channel}:dm:{id}` | Channel DM | `telegram:dm:user42` |
| `{parent}:thread:{id}` | Threaded conversation | `discord:group:123:thread:t456` |
| `{parent}:topic:{id}` | Topic-grouped conversation | `slack:channel:abc:topic:t789` |
| `agent:{agentId}:rest` | Agent-scoped session | `agent:bot1:main` |
| `acp:...` | ACP session | `acp:session1` |

### 9.3 Rust Key Parser

```rust
// crates/agent/src/keyparser.rs

pub struct SessionKeyInfo {
    pub raw: String,
    pub channel: String,
    pub chat_type: String,
    pub chat_id: String,
    pub agent_id: String,
    pub is_subagent: bool,
    pub is_acp: bool,
    pub is_thread: bool,
    pub is_topic: bool,
    pub parent_key: String,
    pub rest: String,
}

pub fn parse_session_key(key: &str) -> SessionKeyInfo { ... }
pub fn is_subagent_key(key: &str) -> bool { key.starts_with("subagent:") }
pub fn is_acp_key(key: &str) -> bool { key.starts_with("acp:") }
pub fn is_agent_key(key: &str) -> bool { key.starts_with("agent:") }
pub fn build_session_key(channel: &str, chat_type: &str, chat_id: &str) -> String { ... }
pub fn build_thread_session_key(parent_key: &str, thread_id: &str) -> String { ... }
pub fn build_subagent_session_key(parent_id: &str, subagent_id: &str) -> String { ... }
```

The key parser has full test coverage (12 tests) in the Rust implementation.

### 9.4 Companion Chat Scoping

The companion chat uses `"companion-default"` as the `user_id` value (constant `COMPANION_USER_ID` in the Rust chat handler). The `chats` table has a UNIQUE index on `user_id WHERE user_id IS NOT NULL`, so there is EXACTLY one companion chat per user.

```rust
// crates/server/src/handlers/chat.rs
const COMPANION_USER_ID: &str = "companion-default";

pub async fn get_companion_chat(State(state): State<AppState>) -> HandlerResult<serde_json::Value> {
    let chat = if let Ok(Some(chat)) = state.store.get_companion_chat_by_user(COMPANION_USER_ID) {
        chat
    } else {
        let id = uuid::Uuid::new_v4().to_string();
        state.store.get_or_create_companion_chat(&id, COMPANION_USER_ID)?
    };
    let messages = state.store.get_chat_messages(&chat.id).unwrap_or_default();
    Ok(Json(json!({"chat": chat, "messages": messages, "totalMessages": messages.len()})))
}
```

### 9.5 Session Lifecycle (Rust)

```rust
// crates/agent/src/session.rs

impl SessionManager {
    pub fn get_or_create(&self, session_key: &str, user_id: &str) -> Result<Session, NeboError>;
    pub fn resolve_session_key(&self, session_id: &str) -> Result<String, NeboError>;
    pub fn get_messages(&self, session_id: &str) -> Result<Vec<ChatMessage>, NeboError>;
    pub fn append_message(&self, session_id: &str, role: &str, content: &str, ...) -> Result<ChatMessage, NeboError>;
    pub fn get_summary(&self, session_id: &str) -> Result<String, NeboError>;
    pub fn update_summary(&self, session_id: &str, summary: &str) -> Result<(), NeboError>;
    pub fn get_active_task(&self, session_id: &str) -> Result<String, NeboError>;
    pub fn set_active_task(&self, session_id: &str, task: &str) -> Result<(), NeboError>;
    pub fn reset(&self, session_id: &str) -> Result<(), NeboError>;
    pub fn delete_session(&self, session_id: &str) -> Result<(), NeboError>;
}
```

### 9.6 Session Reset

When the frontend sends `session_reset`:

```rust
"session_reset" => {
    let session_id = parsed["data"]["session_id"].as_str().unwrap_or("default").to_string();
    let result = state.runner.sessions().reset(&session_id);
    // Broadcasts session_reset response with success/error
    state.hub.broadcast("session_reset", json!({
        "session_id": session_id,
        "success": result.is_ok(),
    }));
}
```

The `reset()` method deletes all `chat_messages` for the resolved `chat_id` and resets all session counters (message_count, token_count, summary, compaction_count, active_task, memory_flush fields).

### 9.7 Go Backward Compatibility

Go defines type aliases so callers can import from either package:

```go
type Manager     = db.SessionManager
type Message     = db.AgentMessage
type ToolCall    = db.AgentToolCall
type ToolResult  = db.AgentToolResult
```

Rust does NOT need this pattern -- `pub use` re-exports serve the same purpose.

---

## 10. Barge-In and Message Queuing

**File(s):** `app/src/routes/(app)/agent/+page.svelte` (frontend)

### 10.1 Barge-In

When `sendMessage()` is called while `isLoading === true`:

1. Cancel active response: `client.send('cancel', { session_id })`
2. Mark current streaming message as interrupted: `currentStreamingMessage.streaming = false`
3. Force all running tools to complete status
4. Set `isLoading = false`
5. Clear message queue -- the new message supersedes everything
6. Fall through to send the new message immediately

**Key insight:** The new message supersedes the queue. Queue is discarded on barge-in.

```typescript
function sendMessage() {
    if (!inputValue.trim()) return;
    const prompt = inputValue.trim();
    inputValue = '';
    clearDraft();

    // BARGE-IN: if agent is responding, cancel and send new message
    if (isLoading) {
        client.send('cancel', { session_id: chatId || '' });
        currentStreamingMessage.streaming = false;
        isLoading = false;
        messageQueue = [];     // Clear queue -- new message supersedes
    }

    messages = [...messages, { id: uuid, role: 'user', content: prompt, ... }];
    autoScrollEnabled = true;
    handleSendPrompt(prompt);
}
```

### 10.2 Message Queue

The `messageQueue` state array exists for queuing messages while the agent is busy:

```typescript
let messageQueue = $state<QueuedMessage[]>([]);
```

`processQueue()` runs after `handleChatComplete()`:
1. Pop first item from queue
2. Add it to `messages[]` as a user message
3. Call `handleSendPrompt()` to send to the agent

Users can:
- `recallFromQueue()` -- pop last queued message back to input for editing
- `cancelQueuedMessage(id)` -- remove a specific queued message

The queue renders as pill-style "clock" badges below the input area.

### 10.3 Cancel Flow

```
cancelMessage()
  |-- WebSocket send: {type: "cancel", session_id}
  |-- Start 2s timeout:
      |-- If no chat_cancelled arrives -> force-reset:
          |-- Append "[Generation cancelled]" to message content
          |-- Mark all running tools as complete
          |-- Clear isLoading = false
          |-- Process queue
```

Server-side cancel handling:

```rust
// crates/server/src/handlers/ws.rs
"cancel" => {
    let session_id = parsed["data"]["session_id"].as_str().unwrap_or("default").to_string();
    let runs = active_runs.lock().await;
    if let Some(token) = runs.get(&session_id) {
        token.cancel();  // Triggers select! cancellation in dispatch_chat
    }
    state.hub.broadcast("chat_cancelled", json!({"session_id": session_id}));
}
```

The `CancellationToken.cancel()` causes the `tokio::select!` in `dispatch_chat` to take the cancellation branch, which broadcasts `chat_cancelled` and breaks out of the event loop. The `chat_complete` event is still sent after the loop exits.

---

## 11. Stream Resumption

**File(s):** `internal/realtime/chat.go` (Go); `crates/server/src/handlers/ws.rs` (Rust)

### 11.1 check_stream Protocol

When the page loads or the WebSocket reconnects, the frontend sends `check_stream` to determine if an agent run is already in progress:

```json
{"type": "check_stream", "data": {"session_id": "companion-chat-id"}}
```

### 11.2 Go Implementation (Full)

The Go ChatContext checks `activeSessions[sessionID]`. If a pending request exists:

1. Sends `stream_status` with `active=true` and the accumulated content (the buffered `streamedContent` from the pending request)
2. Frontend sets `isLoading=true`, creates a streaming message with the accumulated content
3. Subsequent `chat_stream` events continue appending

If no active session:
1. Sends `stream_status` with `active=false`
2. Frontend checks if messages are empty -- if so, requests introduction after 5s timeout

### 11.3 Rust Implementation (Partial)

```rust
"check_stream" => {
    let session_id = parsed["data"]["session_id"].as_str().unwrap_or("default").to_string();
    let running = active_runs.lock().await.contains_key(&session_id);
    let status = if running { "running" } else { "idle" };
    let reply = json!({
        "type": "stream_status",
        "data": {"session_id": session_id, "status": status},
    });
    // Send directly to THIS client (not broadcast)
    socket.send(Message::Text(serde_json::to_string(&reply).unwrap().into())).await;
}
```

**Gap:** The Rust implementation reports `running`/`idle` but does NOT send accumulated content. A page refresh during streaming will show the loading indicator but NOT the content streamed so far. To match Go behavior, the Rust implementation needs to accumulate streamed content per session and include it in the `stream_status` response.

### 11.4 Active Session Tracking

| Aspect | Go | Rust |
|--------|-----|------|
| Storage | `ChatContext.activeSessions map[string]string` | `ActiveRuns = Arc<Mutex<HashMap<String, CancellationToken>>>` |
| Keyed by | sessionID | sessionID |
| Value | requestID | CancellationToken |
| Set when | pendingRequest is created | dispatch_chat registers the run |
| Cleared when | "res" frame arrives or cancel | Lane task completes (removes from HashMap) |

### 11.5 Frontend Stream Status Handling

```typescript
function handleStreamStatus(data) {
    if (data.active || data.status === 'running') {
        isLoading = true;
        if (data.content) {
            // Reconstruct in-flight message from accumulated content
            currentStreamingMessage = {
                id: uuid(), role: 'assistant', content: data.content,
                streaming: true, timestamp: new Date(),
            };
        }
    } else if (messages.length === 0) {
        // No active stream and empty chat -- request introduction
        setTimeout(() => requestIntroduction(), 5000);
    }
}
```

---

## 12. Channel Chat

**File(s):** `app/src/lib/components/chat/ChannelChat.svelte`, `internal/handler/agent/loopshandler.go`, `cmd/nebo/agent.go` (Go)

### 12.1 Overview

Nebo presents two distinct chat interfaces through a single page component, switched by a Svelte 5 context value `channelState.activeChannelId`:

- **Companion Chat** (empty `activeChannelId`) -- Full agentic streaming via WebSocket, ~1800+ lines in +page.svelte
- **Channel Chat** (non-empty `activeChannelId`) -- HTTP polling, NeboLoop channel messages, ~300 lines in ChannelChat.svelte

The switching is a top-level `{#if}` / `{:else}` -- no overlay, no dual view.

### 12.2 Channel Chat Architecture

```
ChannelChat.svelte
  | HTTP GET (every 10s polling)
/api/v1/agent/channels/{id}/messages?limit=N
  |
GetChannelMessagesHandler -> hub.SendRequestSync("get_channel_messages")
  | frame to agent
Agent -> NeboLoop SDK -> REST API -> messages + members
  | frame back
Handler returns JSON
```

### 12.3 Comparison Table

| Aspect | Companion Chat | Channel Chat |
|--------|----------------|--------------|
| **Component** | Inline in +page.svelte (1800+ lines) | ChannelChat.svelte (~300 lines) |
| **Transport** | WebSocket (real-time streaming) | HTTP polling (10s interval) |
| **Storage** | Local SQLite (`chat_messages`) | NeboLoop backend (REST API) |
| **Agent processing** | Full agentic loop (`runner.Run()`) | None -- display only |
| **Tool execution** | Yes (tool_start, tool_result, approval) | No |
| **Voice** | Full duplex + TTS + wake word | Text only |
| **Participants** | User <-> Agent (1:1) | Owner <-> Multiple bots (1:N) |
| **Send method** | WebSocket `chat` event | HTTP POST |
| **Content rendering** | Client-side `Markdown.svelte` | Server-rendered `contentHtml` |
| **Message queue** | Yes (barge-in with queue/recall) | No (simple send) |
| **Streaming** | Yes (`chat_stream` events) | No (fetch complete messages) |
| **Session** | Persistent companion session | Stateless (no local session) |
| **History** | Local with "View N earlier messages" | "View older" (30 -> 200) |
| **Loading state** | `isLoading` (single boolean gate) | `loading` / `sending` (separate) |
| **Optimistic updates** | User message added before send | Optimistic message with temp ID |
| **Cancel** | WebSocket `cancel` + 2s timeout | N/A |
| **Staleness** | 60s inactivity -> force stop warning | N/A |
| **Latency** | Real-time | Up to 10 seconds |

### 12.4 Channel Chat Send Flow

```
User types -> handleSend()
  |-- Create optimistic message: {id: "temp-{Date.now()}", from: "You", content, ...}
  |-- Add to rawMessages immediately (instant feedback)
  |-- Clear input
  |-- HTTP POST: sendChannelMessage({text}, channelId)
      |-- On success: loadMessages() to refresh (replaces optimistic with real)
      |-- On error: remove optimistic message
```

### 12.5 HumanInjected Flag

When the owner sends via the channel UI, the message includes:

```go
CommMessage{
    HumanInjected: true,
    HumanID: getNeboLoopOwnerID(commManager),
}
```

This tells the NeboLoop gateway to attribute the message to the owner, not the bot. Other bots in the loop see the message as from the owner.

### 12.6 Channel State Context

```typescript
// app/src/routes/(app)/agent/+layout.svelte
class ChannelState {
    activeChannelId = $state('');     // Empty = companion, non-empty = channel
    activeChannelName = $state('');   // e.g., "general"
    activeLoopName = $state('');      // e.g., "MyLoop"
}
```

Sidebar callbacks:
- `onSelectMyChat()` -> clears all three fields (back to companion)
- `onSelectChannel(id, name, loopName)` -> sets all three fields (switches to channel)

### 12.7 DM Integration

Owner DMs from NeboLoop share the companion chat session:
- `dm_user_message` event adds user message to companion chat `messages[]`
- `chat_stream` with `source: 'dm'` does NOT re-arm `isLoading` (DM activity should NOT block web UI)
- `chat_complete` with `source: 'dm'` finalizes streaming but does NOT touch `isLoading`

---

## 13. Frontend Reference

**File(s):** `app/src/routes/(app)/agent/+page.svelte`, `app/src/lib/components/chat/*.svelte`, `app/src/lib/websocket/client.ts`

The frontend is shared between Go and Rust backends -- unchanged Svelte 5 code served from `../../app/build/` via rust-embed.

### 13.1 Component Hierarchy

```
+page.svelte (agent page, ~2,455 lines)
|-- ChatInput.svelte (314 lines)
|   |-- Textarea (auto-height, min 24px, max 200px, Enter sends, Shift+Enter newline)
|   |-- File browser (native dialog + HTML fallback)
|   |-- Queued message tray (pill badges)
|   |-- Send/Cancel buttons + Voice toggle + Plus button
|-- Messages Container (scrollable)
|   |-- groupedMessages[] -> MessageGroup.svelte
|       |-- ThinkingBlock.svelte (collapsible, brain icon -> Loader2 while streaming)
|       |-- Content blocks (interleaved):
|       |   |-- text -> Markdown.svelte (prose, copy button on last block)
|       |   |-- tool -> ToolCard.svelte (icon, status badge, clickable -> ToolOutputSidebar)
|       |   |-- image -> <img> (base64 or URL, zoom on click)
|       |   |-- ask -> AskWidget.svelte (buttons/select/radio/checkbox/confirm/text_input)
|       |-- ReadingIndicator (streaming, no blocks yet)
|       |-- Footer (sender name, timestamp)
|-- ToolOutputSidebar.svelte (right drawer, 480px, Command + Output tabs)
|-- ApprovalModal (queued, one at a time, Deny/Once/Always)
|-- Voice UI (VoiceSession, Waveform, Transcript, TTS)
```

### 13.2 isLoading -- The Single Source of Truth

```typescript
let isLoading = $state(false);
```

**State transitions:**

```
IDLE (isLoading=false)
  |-- User sends message -> sendToAgent() sets isLoading=true        -> LOADING
  |-- Introduction requested -> sets isLoading=true                   -> LOADING
  |-- Stream resumed -> handleStreamStatus() sets isLoading=true      -> LOADING

LOADING (isLoading=true)
  |-- chat_complete arrives -> isLoading=false                        -> IDLE
  |-- chat_cancelled arrives -> isLoading=false                       -> IDLE
  |-- error arrives -> isLoading=false                                -> IDLE
  |-- chat_response arrives -> isLoading=false                        -> IDLE
  |-- WebSocket disconnects -> isLoading=false                        -> IDLE
  |-- User barge-in -> isLoading=false then true                      -> LOADING (new)
  |-- User cancels -> sends cancel, waits for chat_cancelled
      |-- 2s timeout -> force-reset isLoading=false                   -> IDLE
```

**Design principle:** `isLoading` is ONLY cleared by definitive events -- never by a timer. This prevents the UI from going idle while the agent works on long-running tasks (e.g., multi-step tool execution with gaps between events).

### 13.3 UI State Based on isLoading

| UI Element | isLoading=false | isLoading=true |
|------------|-----------------|----------------|
| Send/Stop button | ArrowUp (send) | Red Square (stop) |
| Placeholder text | `"Reply..."` | `"Type to queue your next message..."` |
| Textarea | Enabled | Enabled (user CAN still type for barge-in) |
| Streaming cursor | Hidden | Pulsing `\|` after last text |
| Message bubble | Static | `animate-pulse-border` class |

### 13.4 Safety Nets

| Mechanism | Duration | Behavior |
|-----------|----------|----------|
| Inactivity timeout | 30s | Appends `*[Timed out]*`, stops loading |
| Stream staleness | 60s | Shows "Force stop" button (warning only, does NOT auto-stop) |
| Loading timeout | 30s | Safety net for stuck loading state |
| Cancel timeout | 2s | Safety net for cancel acknowledgement |

Exemptions: DM events, running tools, and pending ask requests all pause the inactivity timeout.

### 13.5 WebSocket Client

**File:** `app/src/lib/websocket/client.ts` (375 lines)

Singleton `WebSocketClient` class via `getWebSocketClient()`:

- `connect(userId?)` -- Opens WebSocket to `/ws?clientId={uuid}&userId={userId}`
- `send(type, data)` -- Serializes to JSON, queues if disconnected, drains on next `onopen`
- `on(type, handler)` -- Subscribe to event type, returns unsubscribe function
- `onStatus(handler)` -- Subscribe to connection status changes
- `isConnected()` -- Returns `currentStatus === 'connected'`
- Auto-reconnect: exponential backoff `min(2000 * 2^attempts, 30000)ms`
- Message batching: backend may batch multiple JSON messages per frame (newline-separated)
- Ping/pong: 30s keep-alive

### 13.6 Auto-Scroll

Uses `requestAnimationFrame` to coalesce rapid chunk updates. `$effect` tracks `messages.length` + `currentStreamingMessage?.content`. Behavior: `instant` during streaming, `smooth` after completion. User scroll >100px from bottom disables auto-scroll; scrolling back re-enables it. `scrollingProgrammatically` flag prevents detection interference.

### 13.7 Message Grouping

```typescript
const groupedMessages = $derived.by((): MessageGroupType[] => {
    // Groups consecutive messages by same role (user/assistant)
    // System/tool messages break groups (currentGroup = null)
    // Returns array of { role, messages[] }
});
```

Reuses the same `MessageGroup.svelte` component for both companion and channel chat.

### 13.8 Draft Persistence

```typescript
const DRAFT_STORAGE_KEY = 'nebo_companion_draft';
// On mount: localStorage.getItem(key) -> inputValue
// On input change: $effect saves to localStorage
// On send or reset: clearDraft() removes key
```

---

## 14. Rust Implementation Status

### 14.1 WebSocket Protocol

| Feature | Status | Notes |
|---------|--------|-------|
| WebSocket upgrade at `/ws` | Y | `client_ws_handler` in `ws.rs` |
| Welcome frame on connect | Y | Sends `connected` with version |
| `chat` message handling | Y | `dispatch_chat` function |
| `cancel` message handling | Y | `CancellationToken` pattern |
| `ping`/`pong` (app-level) | Y | JSON ping/pong in select loop |
| `ping`/`pong` (WS-level) | Y | WebSocket Ping/Pong frames |
| `session_reset` | Y | Calls `runner.sessions().reset()` |
| `check_stream` | Y | Reports running/idle status |
| `approval_response` | Y | `oneshot::Sender<bool>` pattern |
| `ask_response` | Y | `oneshot::Sender<String>` pattern |
| `request_introduction` | P | Sends `chat_complete` stub (introduction NOT implemented) |
| `auth`/`connect` | Y | Returns `auth_ok` |
| Agent WebSocket at `/api/v1/agent/ws` | Y | `agent_ws_handler` (forwards events to ClientHub) |

### 14.2 Server-to-Client Events

| Event | Status | Notes |
|-------|--------|-------|
| `chat_stream` | Y | Text delta forwarded from runner stream |
| `chat_complete` | Y | Always sent after stream ends (normal, error, or cancel) |
| `tool_start` | Y | Forwarded from `StreamEventType::ToolCall` |
| `tool_result` | Y | Forwarded from `StreamEventType::ToolResult` |
| `image` | N | Image events NOT yet emitted by tools |
| `thinking` | Y | Forwarded from `StreamEventType::Thinking` |
| `error` / `chat_error` | Y | Forwarded from `StreamEventType::Error` |
| `approval_request` | Y | Forwarded from `StreamEventType::ApprovalRequest` |
| `ask_request` | Y | Forwarded from `StreamEventType::AskRequest` |
| `stream_status` | P | Reports running/idle but NOT accumulated content |
| `chat_cancelled` | Y | Broadcast on cancel |
| `reminder_complete` | N | Cron scheduler does NOT yet emit this |
| `dm_user_message` | N | NeboLoop comm integration NOT yet present |
| `usage` | Y | Token usage stats (Rust-only; Go does NOT broadcast this) |
| `chat_created` | Y | Broadcast when dispatch_chat starts (Rust-only) |

### 14.3 Stream Processing

| Feature | Status | Notes |
|---------|--------|-------|
| UTF-8 boundary protection | N | Rust strings are valid UTF-8, but provider chunks may need buffering |
| Fence marker stripping | N | NOT implemented |
| Content block building (server-side) | N | Frontend handles assembly |
| Tool result matching (server-side) | N | Frontend handles matching |
| Text holdback buffer (20 chars) | N | NOT implemented (no fence marker stripping) |
| Flush on completion | N | NOT needed without holdback buffer |

### 14.4 Content Block System

| Feature | Status | Notes |
|---------|--------|-------|
| Text blocks (frontend) | Y | Frontend assembles from `chat_stream` events |
| Tool blocks (frontend) | Y | Frontend creates from `tool_start` / `tool_result` |
| Image blocks | N | No `image` event emitted by tools |
| Ask blocks | P | Ask events emitted but missing `widgets` payload in broadcast |
| Metadata persistence | N | `metadata` column NOT populated by runner |
| contentBlocks in metadata JSON | N | NOT built server-side |

### 14.5 Ask Widget System

| Feature | Status | Notes |
|---------|--------|-------|
| `ask_request` event broadcast | Y | Via `StreamEventType::AskRequest` |
| `ask_response` handling | Y | `oneshot` channel in `ask_channels` |
| Blocking until response | Y | Runner awaits oneshot receiver |
| Widget types in payload | N | Only `question_id` and `prompt` sent (no `widgets` array) |
| Default confirm(Yes/No) | N | No default widget generation |
| Ask block persistence in metadata | N | NOT implemented |
| text_input widget type | N | NOT implemented in agent tool |

### 14.6 Approval Modal System

| Feature | Status | Notes |
|---------|--------|-------|
| `approval_request` broadcast | Y | Via `StreamEventType::ApprovalRequest` |
| `approval_response` handling | Y | `oneshot` channel in `approval_channels` |
| Blocking until response | Y | Runner awaits oneshot receiver |
| "Always" allowlist | N | Only `bool` approved, no `always` flag |
| Policy-driven approval | P | Tool registry checks policy, but interactive escalation limited |

### 14.7 Message Persistence

| Feature | Status | Notes |
|---------|--------|-------|
| Single write path (runner only) | Y | `SessionManager.append_message()` |
| Empty message guard | Y | Drops empty messages with validation error |
| Orphan sanitization | Y | `sanitize_messages()` in `session.rs` |
| Token estimation | Y | chars/4 heuristic |
| day_marker generation | Y | `date('now', 'localtime')` in SQL |
| Metadata JSON building | N | `metadata` column always NULL |
| tool_calls column | Y | Stored as JSON string by runner |
| tool_results column | Y | Stored as JSON string by runner |

### 14.8 Session Management

| Feature | Status | Notes |
|---------|--------|-------|
| SessionManager with cache | Y | `HashMap<String, String>` with RwLock |
| Session key resolution | Y | `resolve_session_key()` with DB fallback |
| get_or_create session | Y | Scoped upsert with ON CONFLICT |
| Session reset | Y | Deletes messages + resets all counters |
| Key parser | Y | Full implementation with 12 tests |
| Companion chat scoping | Y | `companion-default` user_id constant |
| Summary management | Y | get_summary, update_summary |
| Active task management | Y | get/set/clear active_task |
| Work tasks management | Y | get/set work_tasks |

### 14.9 Chat HTTP Endpoints

| Endpoint | Status | Notes |
|----------|--------|-------|
| `GET /api/v1/chats` | Y | `list_chats` handler |
| `POST /api/v1/chats` | Y | `create_chat` handler |
| `GET /api/v1/chats/:id` | Y | `get_chat` handler |
| `PUT /api/v1/chats/:id` | Y | `update_chat` handler |
| `DELETE /api/v1/chats/:id` | Y | `delete_chat` handler (cascades messages) |
| `GET /api/v1/chats/companion` | Y | `get_companion_chat` handler |
| `GET /api/v1/chats/search` | Y | `search_messages` handler |
| `POST /api/v1/chats/message` | Y | `send_message` handler |
| `GET /api/v1/chats/days` | Y | `list_chat_days` handler |
| `GET /api/v1/chats/history/:day` | Y | `get_chat_history_by_day` handler |
| `GET /api/v1/chats/:id/messages` | Y | `get_chat_messages` handler |

### 14.10 Barge-In and Queuing

| Feature | Status | Notes |
|---------|--------|-------|
| Cancel via CancellationToken | Y | Token cancels the select! loop in dispatch_chat |
| chat_cancelled broadcast | Y | Sent immediately on cancel |
| chat_complete after cancel | Y | Always sent after loop exits |
| Frontend barge-in logic | Y | Shared frontend code (unchanged) |
| Frontend message queue | Y | Shared frontend code (unchanged) |

### 14.11 Stream Resumption

| Feature | Status | Notes |
|---------|--------|-------|
| check_stream handling | Y | Reports running/idle via socket.send |
| Active session tracking | Y | `ActiveRuns` HashMap with CancellationToken |
| Accumulated content in stream_status | N | Only status string, no content replay |
| Introduction request | P | Stub sends chat_complete (no actual introduction) |

### 14.12 Channel Chat

| Feature | Status | Notes |
|---------|--------|-------|
| NeboLoop loop/channel listing | N | No NeboLoop comm integration in Rust |
| Channel message fetch | N | No HTTP handlers for channels |
| Channel message send | N | No HTTP handlers for channels |
| HTTP polling frontend | Y | Shared frontend code (unchanged) |
| Sidebar loop navigation | Y | Shared frontend code (unchanged) |
| HumanInjected flag | N | No comm message support |

### 14.13 Summary

| Category | Y | P | N |
|----------|---|---|---|
| WebSocket Protocol | 11 | 1 | 0 |
| Server Events | 9 | 1 | 3 |
| Stream Processing | 0 | 0 | 6 |
| Content Blocks | 2 | 1 | 3 |
| Ask Widgets | 3 | 0 | 3 |
| Approval Modals | 3 | 1 | 1 |
| Message Persistence | 6 | 0 | 1 |
| Session Management | 9 | 0 | 0 |
| Chat HTTP Endpoints | 11 | 0 | 0 |
| Barge-In / Queuing | 5 | 0 | 0 |
| Stream Resumption | 2 | 1 | 1 |
| Channel Chat | 2 | 0 | 4 |
| **TOTAL** | **63** | **4** | **22** |

Legend: Y = implemented, P = partial, N = not started

---

## Key Files Reference

### Go Source Files

| File | Purpose |
|------|---------|
| `internal/realtime/hub.go` | Client Hub: browser WebSocket connections, broadcast |
| `internal/realtime/chat.go` | ChatContext: pending requests, stream processing, content blocks, UTF-8 safety |
| `internal/realtime/client.go` | Client connection: read/write pumps, buffered send channel (256) |
| `internal/agenthub/hub.go` | Agent Hub: agent WebSocket, frame routing, sync request/response |
| `internal/db/session_manager.go` | SessionManager: single write path, key resolution, compaction |
| `internal/db/queries/chats.sql` | SQL queries for chats and chat_messages tables |
| `internal/agent/session/keyparser.go` | Session key parser (hierarchical key format) |
| `internal/agent/tools/agent_tool.go` | AskWidget type, AskCallback, messageAsk handler |
| `internal/agent/tools/policy.go` | Policy levels, ApprovalCallback, safe bins |
| `cmd/nebo/agent.go` | Agent-side state, approval/ask request/handle, callback wiring |
| `internal/handler/chat/*.go` | HTTP handlers for chat CRUD endpoints |
| `internal/handler/agent/loopshandler.go` | HTTP handlers for NeboLoop loop/channel operations |

### Rust Source Files

| File | Purpose |
|------|---------|
| `crates/server/src/handlers/ws.rs` | Merged hub: ClientHub, handle_client_ws, dispatch_chat, active_runs |
| `crates/server/src/handlers/chat.rs` | HTTP handlers: chat CRUD, companion, search, days, history |
| `crates/server/src/state.rs` | AppState: approval_channels, ask_channels, hub, runner, lanes |
| `crates/agent/src/runner.rs` | Agentic loop: RunRequest, RunState, MAX_ITERATIONS, provider fallback |
| `crates/agent/src/session.rs` | SessionManager: append_message, get_messages, reset, key cache |
| `crates/agent/src/keyparser.rs` | Session key parser: parse, build, predicates (12 tests) |
| `crates/agent/src/lanes.rs` | Lane system: per-lane concurrency limits, task queuing |
| `crates/ai/src/types.rs` | StreamEventType, StreamEvent, ToolCall, Provider trait, ProviderError |
| `crates/db/src/models.rs` | Chat, ChatMessage, Session structs (serde-serializable) |
| `crates/db/src/queries/chats.rs` | SQLite queries: create, get, list, search, delete, companion, day grouping |
| `crates/db/src/queries/sessions.rs` | SQLite queries: create, get, update, reset, delete, scoped sessions |
| `crates/types/src/constants.rs` | Lane names (MAIN, EVENTS, ...), origin types (USER, COMM, ...) |

### Frontend Files (Shared -- Unchanged)

| File | Purpose |
|------|---------|
| `app/src/routes/(app)/agent/+page.svelte` | Main chat page: state, 13+ event handlers, send/cancel/queue (~2,455 lines) |
| `app/src/routes/(app)/agent/+layout.svelte` | ChannelState context, sidebar layout (~60 lines) |
| `app/src/lib/components/chat/ChatInput.svelte` | Input: textarea, send/stop, file drop, queue tray (~314 lines) |
| `app/src/lib/components/chat/MessageGroup.svelte` | Rendering: grouping, content blocks, pre-resolution (~300 lines) |
| `app/src/lib/components/chat/ToolCard.svelte` | Tool display: icon, status badge, clickable |
| `app/src/lib/components/chat/ThinkingBlock.svelte` | Thinking: collapsible, brain/loader icon, pre-wrap |
| `app/src/lib/components/chat/AskWidget.svelte` | Ask widget: 6 types, submit callback, read-only badge (~108 lines) |
| `app/src/lib/components/chat/ChannelChat.svelte` | Channel chat: HTTP polling, optimistic send (~300 lines) |
| `app/src/lib/components/chat/ToolOutputSidebar.svelte` | Tool output: right drawer, command + output tabs |
| `app/src/lib/components/ui/ApprovalModal.svelte` | Approval: modal overlay, Deny/Once/Always buttons |
| `app/src/lib/components/ui/Markdown.svelte` | Markdown: custom parser, 50KB truncation, Twitter embeds |
| `app/src/lib/websocket/client.ts` | WebSocket: singleton, auto-reconnect, batching, queue (~375 lines) |
| `app/src/lib/components/sidebar/Sidebar.svelte` | Sidebar: loop/channel navigation, 60s refresh (~300 lines) |

---

*Generated: 2026-03-04*
