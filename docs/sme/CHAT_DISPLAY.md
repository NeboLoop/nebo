# Chat Display & Conversation System — SME Deep Dive

> **Scope:** Everything from message persistence to pixel rendering. Covers the database schema, session management, real-time streaming pipeline, WebSocket events, HTTP endpoints, frontend components, and content block rendering.

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                       BROWSER (Web UI)                          │
│                                                                 │
│  agent/+page.svelte                                            │
│  ├─ ChatInput.svelte           (user input + voice + queue)    │
│  ├─ MessageGroup.svelte        (grouped message rendering)     │
│  │   ├─ ThinkingBlock.svelte   (collapsible reasoning)         │
│  │   ├─ ToolCard.svelte        (compact tool status)           │
│  │   ├─ AskWidget.svelte       (interactive input blocks)      │
│  │   └─ Markdown.svelte        (prose text rendering)          │
│  ├─ ToolOutputSidebar.svelte   (full tool output drawer)       │
│  └─ ApprovalModal              (tool approval dialog)          │
│                                                                 │
│  WebSocket: /ws?clientId={uuid}&userId={userId}                │
└─────────────────────────────────────────────────────────────────┘
                          ↑↓ WebSocket
┌─────────────────────────────────────────────────────────────────┐
│               Client Hub (internal/realtime/hub.go)             │
│  - Maintains N browser WebSocket connections                   │
│  - Broadcasts events to all connected clients                  │
└─────────────────────────────────────────────────────────────────┘
                          ↑↓ Function calls
┌─────────────────────────────────────────────────────────────────┐
│           ChatContext (internal/realtime/chat.go)               │
│  Bridge between Agent Hub and Client Hub                       │
│  - Tracks pending requests (map[requestID] → pendingRequest)   │
│  - Accumulates streamed content with UTF-8 safety              │
│  - Builds contentBlocks incrementally                          │
│  - Handles title generation, introduction, session reset       │
└─────────────────────────────────────────────────────────────────┘
                          ↑↓ Function calls
┌─────────────────────────────────────────────────────────────────┐
│             Agent Hub (internal/agenthub/hub.go)                │
│  - Manages the ONE agent WebSocket connection                  │
│  - Routes frames by type: req, res, stream, event              │
└─────────────────────────────────────────────────────────────────┘
                          ↑↓ WebSocket
┌─────────────────────────────────────────────────────────────────┐
│                    THE AGENT (Go process)                       │
│  - Runner executes agentic loop                                │
│  - Persists messages via AppendMessage() — SINGLE WRITE PATH   │
│  - Sends stream/res frames to Agent Hub                        │
└─────────────────────────────────────────────────────────────────┘
                          ↓ writes
┌─────────────────────────────────────────────────────────────────┐
│               SQLite (chat_messages table)                      │
│  - Unified storage for all message types                       │
│  - Tool calls/results stored as separate JSON columns          │
│  - SessionKey = chat_id (not session UUID)                     │
└─────────────────────────────────────────────────────────────────┘
```

---

## 1. Database Schema

### 1.1 Tables

**chats** (`internal/db/migrations/0008_chats.sql`, `0009_companion_mode.sql`)
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

**chat_messages** (`0008_chats.sql`, `0009_companion_mode.sql`, `0045_unified_messages.sql`)
```sql
CREATE TABLE chat_messages (
    id TEXT PRIMARY KEY,                    -- UUID generated per message
    chat_id TEXT NOT NULL,                  -- FK to chats.id (= sessionKey)
    role TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'system', 'tool')),
    content TEXT NOT NULL,
    metadata TEXT,                          -- JSON: toolCalls, thinking, contentBlocks
    tool_calls TEXT,                        -- JSON array of tool invocations (separate column)
    tool_results TEXT,                      -- JSON array of tool results (separate column)
    token_estimate INTEGER,                 -- Token count for context budgeting
    day_marker TEXT,                        -- ISO date (YYYY-MM-DD) for history browsing
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    FOREIGN KEY (chat_id) REFERENCES chats(id) ON DELETE CASCADE
);
-- Indexes: chat_id, created_at, (chat_id, day_marker)
```

**sessions** (`0010_auth_profiles.sql`, extended by `0020-0043`)
```sql
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,                    -- UUID
    name TEXT,                              -- sessionKey (maps to chat_messages.chat_id)
    scope TEXT DEFAULT 'global',            -- global, user, channel, agent
    scope_id TEXT,                          -- user_id or channel_id
    summary TEXT,                           -- compacted conversation summary
    token_count INTEGER DEFAULT 0,
    message_count INTEGER DEFAULT 0,
    last_compacted_at INTEGER,
    compaction_count INTEGER DEFAULT 0,
    active_task TEXT,                       -- pinned task (survives compaction)
    last_summarized_count INTEGER DEFAULT 0,
    -- ... policy fields: send_policy, model_override, provider_override, etc.
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
-- UNIQUE index on (name, scope, scope_id)
```

**Migration 0045 (unified messages):** Dropped `session_messages` table. Migrated all data to `chat_messages`. Added `tool_calls`, `tool_results`, `token_estimate` columns.

### 1.2 Key Mapping: Session ID vs Session Key vs Chat ID

```
Runner works with:     sessionID (UUID from sessions.id)
                            ↓ resolved via SessionManager cache
SessionManager maps:   sessionID → sessionKey (sessions.name)
                            ↓ used as
chat_messages stores:  chat_id = sessionKey
```

The `SessionManager` (`internal/db/session_manager.go:99-111`) caches this mapping in a `sync.Map` to avoid per-message DB lookups.

### 1.3 Go Structs

```go
// internal/db/models.go (sqlc-generated)
type ChatMessage struct {
    ID, ChatID, Role, Content string
    Metadata, DayMarker, ToolCalls, ToolResults sql.NullString
    TokenEstimate sql.NullInt64
    CreatedAt int64
}

// internal/db/session_manager.go (wrapper types)
type AgentMessage struct {
    ID          int64           `json:"id,omitempty"`
    SessionID   string          `json:"session_id"`
    Role        string          `json:"role"`         // user, assistant, system, tool
    Content     string          `json:"content,omitempty"`
    ToolCalls   json.RawMessage `json:"tool_calls,omitempty"`
    ToolResults json.RawMessage `json:"tool_results,omitempty"`
    CreatedAt   time.Time       `json:"created_at"`
}

type AgentToolCall struct {
    ID    string          `json:"id"`
    Name  string          `json:"name"`
    Input json.RawMessage `json:"input"`
}

type AgentToolResult struct {
    ToolCallID string `json:"tool_call_id"`
    Content    string `json:"content"`
    IsError    bool   `json:"is_error,omitempty"`
}
```

### 1.4 Key Queries (`internal/db/queries/chats.sql`)

| Query | Purpose |
|-------|---------|
| `GetOrCreateCompanionChat(id, user_id)` | Upsert companion chat per user |
| `CreateChatMessageForRunner(...)` | Insert with tool_calls, tool_results, token_estimate |
| `GetChatMessages(chat_id)` | All messages ordered by created_at |
| `GetRecentChatMessagesWithTools(chat_id, limit)` | Last N messages with tool data |
| `GetChatWithMessages(chat_id)` | Join: chat + all messages |
| `GetMessagesByDay(chat_id, day_marker)` | Messages for a specific day |
| `SearchChatMessages(chat_id, query, limit, offset)` | Full-text search |
| `CountChatMessages(chat_id)` | Count for pagination |
| `DeleteChatMessagesByChatId(chat_id)` | Wipe all messages (session reset) |
| `UpdateChatTitle(id, title)` | Set auto-generated title |

---

## 2. Session Management (`internal/db/session_manager.go`)

### 2.1 Core Operations

**GetOrCreate(sessionKey, userID)** (lines 123-190)
- Checks DB for existing session by name+scope
- Creates session + matching chats row if not found
- Scope: `user` (with userID) or `agent` (without)

**AppendMessage(sessionID, msg)** (lines 250-289) — THE SINGLE WRITE PATH
- Guards: skips empty messages (no content + no tool calls + no tool results)
- Resolves sessionID → sessionKey via cache
- Generates UUID for message ID
- Marshals tool_calls/tool_results to JSON
- Writes to `chat_messages(chat_id=sessionKey)`
- Increments session message_count

**GetMessages(sessionID, limit)** (lines 205-248)
- Fetches from chat_messages via sessionKey
- Supports `limit=0` for all messages
- Sanitizes: strips orphaned tool_results with no matching tool_calls (lines 421-449)

### 2.2 Session Key Naming (`internal/agent/session/keyparser.go`)

```
companion-default              — Web UI companion chat
dm-{conversationID}            — External DM session
subagent-{uuid}                — Sub-agent session
{channel}:group:{id}           — Group chat
{channel}:channel:{id}         — Channel session
{channel}:dm:{id}              — Channel DM
{parent}:thread:{id}           — Threaded conversation
```

### 2.3 Backward Compatibility (`internal/agent/session/session.go`)

Type aliases so callers can import from either package:
```go
type Manager     = db.SessionManager
type Message     = db.AgentMessage
type ToolCall    = db.AgentToolCall
type ToolResult  = db.AgentToolResult
```

---

## 3. Real-Time Streaming Pipeline

### 3.1 Frame Format (Agent Hub)

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
| `req` | Server→Agent | Request agent action |
| `res` | Agent→Server | Final response (complete) |
| `stream` | Agent→Server | Streaming chunk (incremental) |
| `event` | Agent→Server | Broadcast event (lane updates, DMs) |
| `approval_response` | Browser→Agent | User approves tool execution |
| `ask_response` | Browser→Agent | User provides input |

### 3.2 Stream Chunk Payloads

| Field | Type | Purpose |
|-------|------|---------|
| `chunk` | string | Text delta (accumulated with UTF-8 boundary safety) |
| `tool` | string | Tool name (triggers tool_start event) |
| `tool_id` | string | Tool call ID for matching results |
| `input` | string | Tool input JSON |
| `tool_result` | string | Tool output (triggers tool_result event) |
| `thinking` | string | Extended thinking content |
| `image_url` | string | Image from tool (triggers image event) |

### 3.3 ChatContext Processing (`internal/realtime/chat.go`)

#### Pending Request Tracking (lines 281-524)

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

#### UTF-8 Boundary Protection

Prevents splitting multi-byte characters (emoji, CJK) across WebSocket messages:

```go
safeLen := len(req.streamedContent)
for safeLen > req.cleanSentLen && !utf8.RuneStart(req.streamedContent[safeLen]) {
    safeLen--
}
delta := req.streamedContent[req.cleanSentLen:safeLen]
req.cleanSentLen = safeLen
```

#### Tool Result Matching

```go
// 1. Try by tool_id
for i := range req.toolCalls {
    if req.toolCalls[i].ID == toolID && toolID != "" {
        req.toolCalls[i].Output = toolResult
        updated = true; break
    }
}
// 2. Fallback: first "running" tool
if !updated {
    for i := range req.toolCalls {
        if req.toolCalls[i].Status == "running" {
            req.toolCalls[i].Output = toolResult; break
        }
    }
}
```

### 3.4 WebSocket Events (Server → Browser)

| Event | Data | Purpose |
|-------|------|---------|
| `chat_stream` | `{session_id, content}` | Text delta (partial) |
| `chat_complete` | `{session_id}` | Response finished |
| `tool_start` | `{session_id, tool, tool_id, input}` | Tool invoked |
| `tool_result` | `{session_id, result, tool_name, tool_id}` | Tool completed |
| `image` | `{session_id, image_url}` | Image from tool |
| `thinking` | `{session_id, content}` | Extended thinking |
| `error` | `{session_id, error}` | LLM error |
| `approval_request` | `{request_id, tool, input}` | Tool needs approval |
| `ask_request` | `{request_id, prompt, widgets}` | Agent asks user |
| `stream_status` | `{session_id, active, content}` | Stream resume check |
| `chat_cancelled` | `{session_id}` | User cancelled |
| `dm_user_message` | `{session_id, content, ...}` | DM from owner |
| `reminder_complete` | `{session_id}` | Scheduled task done |

### 3.5 Special Message Flows

**Title Generation** (lines 486-508, 812-853)
1. After first message completes, `handleAgentResponse` checks `req.isNewChat`
2. Sends separate "run" request to agent with empty prompt
3. Agent generates title, response updates `chats.title`
4. No `chat_complete` sent for title requests

**Introduction** (lines 529-588)
1. Frontend sends `request_introduction` message
2. Creates pendingRequest with `prompt = "__introduction__"`
3. Agent checks if user needs introduction and responds
4. Does not update chat history

**Session Reset** (lines 640-684)
1. Frontend sends `session_reset` with `session_id`
2. Deletes all messages for that chat_id
3. Sends `session_reset` response with `ok:true`

**Stream Resumption** (lines 855-901)
1. On page load, frontend sends `check_stream`
2. If active stream found in `pending`, sends accumulated content via `stream_status`
3. Browser reconstructs in-flight message

---

## 4. HTTP Endpoints

### 4.1 Chat Handlers (`internal/handler/chat/`)

| Endpoint | Handler | Purpose |
|----------|---------|---------|
| `GET /api/v1/chats` | `ListChatsHandler` | Returns exactly ONE companion chat per user |
| `GET /api/v1/chats/{id}` | `GetChatHandler` | Chat + all messages (with markdown-rendered HTML) |
| `DELETE /api/v1/chats/{id}` | `DeleteChatHandler` | Delete chat and cascade messages |

**ListChats quirk:** Always returns `Total: 1` because single bot paradigm = single companion chat.

**GetChat:** Returns messages with `ContentHtml` field pre-rendered via `markdown.Render()`.

### 4.2 Agent Session Handlers (`internal/handler/agent/`)

| Endpoint | Handler | Purpose |
|----------|---------|---------|
| `GET /api/v1/agent/sessions/{id}/messages` | `GetAgentSessionMessagesHandler` | Reads from chat_messages via sessionKey |

Resolves session UUID → session name → chat_id, then queries chat_messages. Fallback to raw ID if no name. Limits to 100 messages.

---

## 5. Frontend Architecture

### 5.1 Main Chat Page (`app/src/routes/(app)/agent/+page.svelte`, ~2,455 lines)

**Critical State (Svelte 5 runes):**
```typescript
let messages = $state<Message[]>([])             // All displayed messages
let currentStreamingMessage = $state<Message>    // In-flight streaming response
let messageQueue = $state<QueuedMessage[]>([])   // Messages waiting to send
let isLoading = $state(false)                    // Single loading control
let chatId = $state<string | null>(null)         // Session ID
let totalMessages = $state<number>(0)            // Total count (pagination)
```

**Initialization (onMount):**
1. Load companion chat via `getCompanionChat()` API
2. Parse messages: extract metadata (toolCalls, thinking, contentBlocks)
3. Handle legacy multipart content format (Anthropic content arrays)
4. Connect WebSocket client, register 13 event handlers
5. Restore draft from localStorage
6. Check for active stream (`check_stream`)
7. Request introduction if empty chat

**Message Grouping (Slack-style):**
```typescript
const groupedMessages = $derived.by((): MessageGroupType[] => {
    // Groups consecutive messages by same role
    // System messages excluded
    // Used by <MessageGroup> component
})
```

### 5.2 Message Types

```typescript
interface Message {
    id: string
    role: 'user' | 'assistant' | 'system'
    content: string                    // Raw text (may have <thinking> markers)
    contentHtml?: string               // Pre-rendered HTML
    timestamp: Date
    toolCalls?: ToolCall[]
    streaming?: boolean                // Actively streaming
    thinking?: string                  // Extracted thinking content
    contentBlocks?: ContentBlock[]     // Structured content
}

interface ToolCall {
    id?: string
    name: string                       // e.g., "file(action: read, ...)"
    input: string                      // JSON input
    output?: string                    // Result (when complete)
    status?: 'running' | 'complete' | 'error'
}

interface ContentBlock {
    type: 'text' | 'tool' | 'image' | 'ask'
    text?: string
    toolCallIndex?: number             // Index into toolCalls[]
    imageData?: string                 // Base64 inline
    imageMimeType?: string
    imageURL?: string                  // Server URL (/api/v1/files/...)
    askRequestId?: string
    askPrompt?: string
    askWidgets?: AskWidgetDef[]
    askResponse?: string               // Filled after user submits
}
```

### 5.3 Event Handlers

**`handleChatStream` (text chunk):**
1. Re-arms isLoading if stream resumed after timeout
2. Resets inactivity timeout
3. Feeds to streaming TTS (sentence buffering)
4. Auto-marks running tools as complete when new text arrives
5. Appends chunk to `currentStreamingMessage.content`
6. Updates or creates last text contentBlock

**`handleToolStart` (tool invoked):**
1. Resets inactivity timeout
2. Creates ToolCall with `status: 'running'`
3. Appends to `currentStreamingMessage.toolCalls[]`
4. Appends `{type: 'tool', toolCallIndex}` block

**`handleToolResult` (tool completed):**
1. Finds tool by ID, fallback to first running tool
2. Updates output and status to 'complete'
3. Appends image block if imageURL present
4. Fallback: searches last 5 assistant messages backwards

**`handleChatComplete` (response finished):**
1. Stops TTS streaming (flushes buffer)
2. Marks `currentStreamingMessage.streaming = false`
3. Safety net: marks any remaining running tools as complete
4. For DM completions: ignores (doesn't touch web UI loading state)
5. For web UI: sets `isLoading = false`, processes message queue

**`handleApprovalRequest` (tool needs approval):**
1. Queues to `approvalQueue[]`
2. Shows ApprovalModal (one at a time)
3. Resolved via `approval_response` message

**`handleAskRequest` (agent asks user):**
1. Sets `pendingAskRequest = true` (disables inactivity timeout)
2. Appends `{type: 'ask'}` block with widgets
3. Widget renders interactive controls

### 5.4 Message Queue (Barge-in)

When user sends while `isLoading`:
- Message added to `messageQueue[]`
- Queue rendered as "clock" badges below input
- Can recall last queued message with Up arrow
- Can cancel individual queued messages
- Processed one-by-one after each response completes

### 5.5 Timeouts and Safety

| Mechanism | Duration | Behavior |
|-----------|----------|----------|
| Inactivity timeout | 30s | Appends `*[Timed out]*`, stops loading |
| Stream staleness | 60s | Shows "Force stop" button (warning only) |
| Loading timeout | 30s | Safety net for stuck loading state |
| Cancel timeout | 2s | Safety net for cancel acknowledgement |

Exemptions: DM events, running tools, pending ask requests all pause the inactivity timeout.

### 5.6 Auto-scroll

- `$effect` tracks `messages.length` + `currentStreamingMessage?.content`
- Uses `requestAnimationFrame` to coalesce rapid chunk updates
- Sets `scrollingProgrammatically` flag to prevent user scroll detection interference
- Instant during streaming, smooth after completion

### 5.7 Draft Persistence

- localStorage key: `nebo_companion_draft`
- Saves on every input change
- Cleared on send or reset chat

---

## 6. Component Hierarchy

```
+page.svelte (agent page, ~2,455 lines)
├── ChatInput.svelte
│   ├── Textarea (auto-height)
│   ├── File browser (native dialog + fallback)
│   ├── Queued message tray
│   └── Send/Cancel buttons + Voice toggle
├── Messages Container (scrollable)
│   └── groupedMessages[] → MessageGroup.svelte
│       ├── ThinkingBlock.svelte (if thinking, collapsible)
│       ├── Content blocks (interleaved):
│       │   ├── text → Markdown.svelte (prose, copy button on last block)
│       │   ├── tool → ToolCard.svelte (icon, status badge, clickable)
│       │   ├── image → <img> (base64 or URL, zoom on click)
│       │   └── ask → AskWidget.svelte (buttons/select/radio/checkbox)
│       ├── ReadingIndicator (streaming, no blocks yet)
│       └── Footer (sender name, timestamp)
├── ToolOutputSidebar.svelte (right drawer, 480px)
│   ├── Command tab
│   └── Output tab (with copy button)
├── ApprovalModal (queued, one at a time)
└── Voice UI (VoiceSession)
    ├── Waveform display
    ├── Transcript
    └── TTS output controls
```

### 6.1 MessageGroup.svelte (`app/src/lib/components/chat/`)

**Pre-resolution optimization:** The `resolvedMessages` derived state resolves all data upfront — tool lookups, thinking extraction, block assembly — so the template has flat, pre-loaded data and avoids multi-level lookups during reactivity.

```typescript
interface ResolvedBlock {
    type: 'text' | 'tool' | 'image' | 'ask'
    key: string                 // Svelte keyed block: "tool-0-running"
    text?: string
    tool?: ToolCall             // Pre-resolved (not an index)
    imageData?: string
    imageMimeType?: string
    imageURL?: string
    askRequestId?: string
    askPrompt?: string
    askWidgets?: AskWidgetDef[]
    askResponse?: string
    isLastBlock: boolean        // Used for cursor animation
}
```

**Rendering order:**
1. Thinking block (if present, collapsed by default)
2. Content blocks in order (text/tool/image/ask interleaved)
3. Legacy fallback: messages without contentBlocks render as simple text + tool list
4. Footer: sender name + timestamp

### 6.2 ToolCard.svelte

- Icon derived from tool name (file, shell, web, etc.)
- Input path extraction via JSON parse with regex fallback
- Status badge: "Running..." (spinner) → "Completed" (green) → "Error" (red)
- Clickable → opens ToolOutputSidebar

### 6.3 ThinkingBlock.svelte

- Initially collapsed
- Brain icon → spinning Loader2 during streaming
- Pre-wrap whitespace (monospace)
- Pulsing cursor while streaming

### 6.4 AskWidget.svelte

- Widget types: buttons, select, radio, checkbox, confirm, text_input
- States: disabled → "Skipped", answered → badge display, interactive → controls
- Submit sends `ask_response` message with value
- Auto-disabled when streaming completes without answer

### 6.5 Markdown.svelte (`app/src/lib/components/ui/`)

- Custom markdown parser via `parseMarkdown()`
- Pre-rendered HTML support (`preRenderedHtml` prop)
- Content truncation at 50KB (with "Show full" button)
- Twitter/X embed support (lazy-loads widget.js)
- NeboLoop link interception
- Tailwind prose styling

---

## 7. WebSocket Client (`app/src/lib/websocket/client.ts`)

**Singleton:** `getWebSocketClient()` returns shared instance.

**Connection:**
- URL: `/ws?clientId={uuid}&userId={userId}`
- Auto-reconnect: exponential backoff (2s × 2^n, cap 30s)
- Message queueing while disconnected
- Ping/pong: 30s keep-alive

**Batched messages:** Backend may send multiple JSON messages per frame separated by `\n`. Each line parsed separately.

**13 registered event types:**
```
chat_stream, chat_complete, tool_start, tool_result, image, thinking,
error, approval_request, ask_request, stream_status, chat_cancelled,
reminder_complete, dm_user_message
```

---

## 8. Complete Message Lifecycle

```
1. USER TYPES MESSAGE
   └─ ChatInput.svelte → value bound to textarea

2. USER SENDS (Enter / click)
   ├─ If isLoading: add to messageQueue[] (barge-in)
   └─ Else: add user Message to messages[], set isLoading=true
      └─ WebSocket send: {type:"chat", data:{session_id, prompt, companion:true}}

3. CHATCONTEXT RECEIVES (realtime/chat.go)
   ├─ GetOrCreateCompanionChat (ensure chat row exists)
   ├─ Create pendingRequest tracking object
   └─ Send "run" frame to agent via Agent Hub

4. AGENT PROCESSES (runner/runner.go)
   ├─ AppendMessage(user msg) → chat_messages       ← User message persisted
   ├─ Build context, call LLM
   ├─ Stream chunks via "stream" frames
   ├─ Execute tools (stream tool_start/result frames)
   └─ AppendMessage(assistant msg) → chat_messages   ← Assistant message persisted

5. CHATCONTEXT PROCESSES STREAMS (realtime/chat.go)
   ├─ Text chunks: accumulate, UTF-8 boundary check, send delta
   ├─ Tool start: flush text, track tool, send tool_start
   ├─ Tool result: update tool, send tool_result + image if any
   ├─ Thinking: accumulate, send thinking
   └─ All: build contentBlocks array incrementally

6. CLIENT HUB BROADCASTS (realtime/hub.go)
   └─ Serialize event, send to all connected browser clients

7. FRONTEND HANDLES EVENTS (agent/+page.svelte)
   ├─ chat_stream: append delta to currentStreamingMessage.content
   ├─ tool_start: append ToolCall, append tool contentBlock
   ├─ tool_result: update ToolCall output/status, maybe append image
   ├─ thinking: accumulate on currentStreamingMessage.thinking
   └─ All: trigger Svelte 5 $state reactivity → re-render

8. AGENT COMPLETES → sends "res" frame
   └─ ChatContext:
      ├─ Flush remaining buffered text
      ├─ Update chat timestamp
      ├─ If new chat: request title generation (separate agent call)
      └─ Send chat_complete event

9. FRONTEND FINALIZES
   ├─ isLoading = false
   ├─ currentStreamingMessage.streaming = false
   ├─ Safety: mark any remaining running tools as complete
   ├─ Move streaming message to messages[]
   └─ Process messageQueue (next queued message, if any)
```

---

## 9. Design Patterns

### Single Write Path
All message persistence flows through `Runner → SessionManager.AppendMessage() → chat_messages`. ChatContext does NOT save messages. Frontend does NOT save messages. If runner crashes before saving, streaming content is lost (but session state survives via SQLite).

### Content Block Pattern
Messages carry structured `contentBlocks[]` instead of monolithic text. Each block type (text/tool/image/ask) renders via its own component. Tool blocks reference `toolCallIndex` into the parent message's `toolCalls[]` array.

### Two-Hub Architecture
Agent Hub manages the single agent connection and routes frames by type. Client Hub manages N browser connections and broadcasts events. ChatContext bridges between them, decoupling the agent protocol from the browser protocol.

### Pending Request Tracking
Every in-flight chat request gets a `pendingRequest` struct in ChatContext's `pending` map. This enables stream resumption on browser reconnect, tool matching by ID, UTF-8 boundary tracking, and incremental contentBlock building.

---

## 10. Key File Reference

| Component | File | Key Lines |
|-----------|------|-----------|
| DB Schema (chats) | `internal/db/migrations/0008_chats.sql` | All |
| DB Schema (unified) | `internal/db/migrations/0045_unified_messages.sql` | All |
| SQL Queries | `internal/db/queries/chats.sql` | All |
| Generated models | `internal/db/models.go` | Chat, ChatMessage, Session |
| Session Manager | `internal/db/session_manager.go` | 48-449 |
| Session key parser | `internal/agent/session/keyparser.go` | All |
| ChatContext | `internal/realtime/chat.go` | All (~1000 lines) |
| Agent Hub | `internal/agenthub/hub.go` | All |
| Client Hub | `internal/realtime/hub.go` | All |
| Client conn | `internal/realtime/client.go` | 58-145 |
| Chat HTTP handlers | `internal/handler/chat/*.go` | All |
| Agent session handler | `internal/handler/agent/getagentsessionmessageshandler.go` | All |
| Frontend chat page | `app/src/routes/(app)/agent/+page.svelte` | All (~2455 lines) |
| MessageGroup | `app/src/lib/components/chat/MessageGroup.svelte` | All |
| ToolCard | `app/src/lib/components/chat/ToolCard.svelte` | All |
| ThinkingBlock | `app/src/lib/components/chat/ThinkingBlock.svelte` | All |
| AskWidget | `app/src/lib/components/chat/AskWidget.svelte` | All |
| ToolOutputSidebar | `app/src/lib/components/chat/ToolOutputSidebar.svelte` | All |
| Markdown | `app/src/lib/components/ui/Markdown.svelte` | All |
| WebSocket client | `app/src/lib/websocket/client.ts` | All |
| Runner (message save) | `internal/agent/runner/runner.go` | AppendMessage calls |
| sqlc config | `sqlc.yaml` | All |

---

## 11. Edge Cases & Quirks

1. **Empty message guard:** Messages with no content + no tool calls + no tool results are silently dropped by `AppendMessage()` — prevents ghost records from cancelled runs.

2. **Orphan sanitization:** `GetMessages()` strips tool_results whose tool_call_id doesn't match any tool_calls — prevents display artifacts from partial saves.

3. **Title generation:** Uses empty prompt (`""`) as discriminator. The ChatContext checks `req.prompt == ""` to know this is a title response, not a chat response. No `chat_complete` is sent for title requests.

4. **Tool status safety net:** Multiple places mark running tools as complete: `handleChatStream` (new text arrived = previous tool done), `handleChatComplete` (response finished), and backward search in `handleToolResult` (searches last 5 messages).

5. **Stream staleness vs inactivity:** 30s inactivity timeout stops loading and appends "[Timed out]". 60s staleness shows "Force stop" button but doesn't auto-stop. Different UX purposes.

6. **DM completion isolation:** `handleChatComplete` ignores completions for DM sessions — they don't affect web UI loading state.

7. **Metadata JSON schema:** The `metadata` column stores `{"toolCalls": [...], "thinking": "...", "contentBlocks": [...]}`. Separate `tool_calls` and `tool_results` columns exist for easier querying.

8. **Legacy content format:** Frontend handles Anthropic-style multipart content arrays (array of `{type: "text", text: "..."}`) by converting them to contentBlocks on load.

9. **ListChats always returns 1:** Single bot paradigm means one companion chat per user. No pagination needed.

10. **Message lookup by ID, not index:** `replaceMessageById()` finds messages by ID before updating. Array index would break when DM events insert messages during streaming.
