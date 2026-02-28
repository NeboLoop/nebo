# Agent Input & Message Streaming — SME Deep Dive

## Overview

The agent input system is a full-duplex pipeline connecting the Svelte 5 frontend to the Go agent backend via two WebSocket hubs. A single `isLoading` state variable is the master signal for whether the agent is active.

**Architecture at a glance:**
```
Frontend (ChatInput + page.svelte)
  ↕ browser WebSocket (/ws)
Client Hub (internal/realtime/hub.go)
  ↕ event/response routing
ChatContext (internal/realtime/chat.go)
  ↕ agent frames
Agent Hub (internal/agenthub/hub.go)
  ↕ agent WebSocket (internal)
Agent (runner loop)
```

---

## 1. Frontend Components

### ChatInput Component
**File:** `app/src/lib/components/chat/ChatInput.svelte` (314 lines)

Stateless input control — all behavior driven by props from the parent page.

**Props:**
```typescript
interface Props {
  value: string;                // Two-way bindable ($bindable())
  placeholder?: string;         // Dynamic — changes based on isLoading
  disabled?: boolean;           // Disables textarea (only used for voice recording)
  isLoading?: boolean;          // Controls which button shows (send vs stop)
  isRecording?: boolean;        // Voice recording active
  voiceMode?: boolean;          // Conversational voice mode active
  queuedMessages?: QueuedMessage[];  // Shows pending pill-style tray
  isDraggingOver?: boolean;     // File drag-over visual state
  onSend: () => void;           // Send callback → parent's sendMessage()
  onCancel?: () => void;        // Stop callback → parent's cancelMessage()
  onCancelQueued?: (id: string) => void;
  onNewSession?: () => void;
  onToggleVoice?: () => void;
}
```

**Key behaviors:**
- **Textarea:** Auto-expanding (min 24px, max 200px), Enter sends, Shift+Enter newline
- **Send button:** Circular ArrowUp, enabled when `canSend = value.trim() && !disabled && !isRecording`
- **Stop button:** Circular red Square, shown when `isLoading && onCancel` is truthy
- **Placeholder:** Changes: `"Reply..."` (idle) → `"Type to queue your next message..."` (loading)
- **Textarea is NOT disabled** when loading — user can type ahead for barge-in or queuing
- **File drop:** Extracts file paths (Wails File.path → file:// URIs → text/plain → filename fallback), appends to input value
- **Plus button:** Opens native file picker (desktop) or HTML file input (headless)

**Derived state:**
```typescript
const canSend = $derived(value.trim() && !disabled && !isRecording);
```

### Agent Page
**File:** `app/src/routes/(app)/agent/+page.svelte` (~1,954 lines)

This is the main chat page — the brain of the frontend.

**Core state variables:**
```typescript
let chatId = $state<string | null>(null);        // Companion chat session ID
let messages = $state<Message[]>([]);             // All messages in view
let inputValue = $state('');                      // Two-way bound to ChatInput
let isLoading = $state(false);                    // THE master agent-activity signal
let wsConnected = $state(false);                  // WebSocket connection status
let currentStreamingMessage = $state<Message | null>(null);  // Active streaming response
let messageQueue = $state<QueuedMessage[]>([]);   // Queued user messages
let draftInitialized = $state(false);             // Draft localStorage hydration flag
```

---

## 2. The Send Flow (User → Agent)

### Step-by-step:

**1. User presses Enter or clicks Send**
```
ChatInput.handleKeydown(e) → if Enter && !Shift → onSend()
  OR
ChatInput.handleSend() → if value.trim() && !disabled && !isRecording → onSend()
```

**2. Parent `sendMessage()` (line 1061)**
```typescript
function sendMessage() {
  if (!inputValue.trim()) return;
  const prompt = inputValue.trim();
  inputValue = '';         // Clear input
  clearDraft();            // Remove localStorage draft

  // BARGE-IN: if agent is responding, cancel and send new message
  if (isLoading) {
    client.send('cancel', { session_id: chatId || '' });
    // Mark current streaming message as interrupted
    currentStreamingMessage.streaming = false;
    isLoading = false;
    messageQueue = [];     // Clear queue — new message supersedes
  }

  // Show user message immediately (optimistic)
  messages = [...messages, { id: uuid, role: 'user', content: prompt, ... }];
  autoScrollEnabled = true;
  handleSendPrompt(prompt);
}
```

**3. `handleSendPrompt()` → `sendToAgent()`**
```typescript
function sendToAgent(prompt: string) {
  isLoading = true;      // ← THE MOMENT LOADING STATE BECOMES TRUE
  client.send('chat', {
    session_id: chatId || '',
    prompt: prompt,
    companion: true       // Flag: this is from companion chat
  });
}
```

**4. WebSocket client serializes and sends**
```json
{
  "type": "chat",
  "data": { "session_id": "uuid", "prompt": "hello", "companion": true },
  "timestamp": "2026-02-23T..."
}
```

### Backend receives the message

**5. Client Hub dispatches to chat handler** (`internal/realtime/chat.go:117-137`)

`RegisterChatHandler` → `SetChatHandler(func)` → `go handleChatMessage(c, msg, chatCtx)`

**6. `handleChatMessage()` (line 633)**
- Waits up to 5s for agent to connect (`waitForAgent`)
- Gets or creates companion chat session
- Saves user message to `chat_messages` DB table
- Creates `pendingRequest` tracked in `chatCtx.pending[requestID]`
- Tracks in `chatCtx.activeSessions[sessionID] = requestID`
- Sends frame to agent via agent hub:
```json
{
  "type": "req",
  "id": "chat-1234567890",
  "method": "run",
  "params": { "session_key": "uuid", "prompt": "hello", "user_id": "...", "system": null }
}
```

---

## 3. The Stream Flow (Agent → User)

### Agent sends streaming chunks

The agent loop sends `"stream"` frames with various payload keys:

| Payload Key | Type | Purpose |
|---|---|---|
| `chunk` | string | Text being generated |
| `tool` + `tool_id` + `input` | string | Tool execution starting |
| `tool_result` + `tool_id` + `tool_name` | string | Tool completed |
| `image_url` | string | Screenshot/image produced |
| `thinking` | string | Extended thinking content |

These are NOT mutually exclusive — a single frame can carry multiple keys.

### Backend processing (`handleAgentResponse`, line 193)

**For `"stream"` frames:**

1. **Text chunks:** Accumulated in `req.streamedContent`, then:
   - Strip fence markers: `afv.StripFenceMarkers(req.streamedContent)`
   - Hold back last 20 chars (fence markers are 18 chars, prevents partial markers leaking)
   - Back up to UTF-8 rune boundary (protects multi-byte chars: emojis, CJK)
   - Calculate delta (new chars since last send)
   - Append to `req.contentBlocks` (last text block or create new)
   - Send via `sendChatStream(client, sessionID, delta)`

2. **Tool start:** Flush held-back text buffer first (so text appears before tool card), then:
   - Track tool call info in `req.toolCalls[]`
   - Append tool content block
   - Send `tool_start` event to frontend

3. **Tool result:** Update matching tool call (by ID or first running), then:
   - If tool produced an image, append image content block
   - Send `tool_result` and optionally `image` events

4. **Thinking:** Accumulate in `req.thinking`, send `thinking` event

**For `"res"` frames (final):**
1. Remove from `pending` and `activeSessions`
2. Flush remaining buffered content (the held-back 20 chars)
3. Update chat timestamp; message persistence is handled by the runner (single write path)
4. For new chats: trigger title generation
5. Send `chat_complete` event to frontend

### Frontend receives events

All 13 event handlers registered in `onMount()` (line 222):

| Handler | Event | Key Action |
|---|---|---|
| `handleChatStream` | `chat_stream` | Append chunk to `currentStreamingMessage.content`, update contentBlocks |
| `handleChatComplete` | `chat_complete` | Set `streaming=false`, `isLoading=false`, call `processQueue()` |
| `handleChatResponse` | `chat_response` | Non-streaming response (rare) |
| `handleToolStart` | `tool_start` | Append tool to `toolCalls[]`, append tool content block |
| `handleToolResult` | `tool_result` | Update tool with output/status, append image block if present |
| `handleImage` | `image` | Append image content block |
| `handleThinking` | `thinking` | Append to `message.thinking` |
| `handleError` | `error` | Mark tools as error, create error message |
| `handleApprovalRequest` | `approval_request` | Push to `approvalQueue[]`, show modal |
| `handleStreamStatus` | `stream_status` | Resume incomplete stream on page load |
| `handleChatCancelled` | `chat_cancelled` | Mark streaming=false, add cancel note |
| `handleReminderComplete` | `reminder_complete` | Create reminder message |
| `handleDMUserMessage` | `dm_user_message` | Create user message from NeboLoop DM |

---

## 4. How the Input Knows the Agent is Active

### The Single Source of Truth: `isLoading`

```typescript
let isLoading = $state(false);
```

**State transitions:**
```
IDLE (isLoading=false)
  ├─ User sends message → sendToAgent() sets isLoading=true  → LOADING
  ├─ Introduction requested → doRequestIntroduction() sets isLoading=true → LOADING
  └─ Stream resumed → handleStreamStatus() sets isLoading=true → LOADING

LOADING (isLoading=true)
  ├─ chat_complete arrives → handleChatComplete() sets isLoading=false → IDLE
  ├─ chat_cancelled arrives → handleChatCancelled() sets isLoading=false → IDLE
  ├─ error arrives → handleError() sets isLoading=false → IDLE
  ├─ User barge-in → sendMessage() sets isLoading=false then true → LOADING (new request)
  ├─ User cancels → cancelMessage() sends cancel frame → waits for chat_cancelled
  │   └─ 2s timeout → force-reset isLoading=false → IDLE
  └─ 30s inactivity timeout → force-reset isLoading=false → IDLE
```

### What Changes in the UI

| UI Element | isLoading=false | isLoading=true |
|---|---|---|
| **Send/Stop button** | ArrowUp (send) | Red Square (stop) |
| **Placeholder text** | `"Reply..."` | `"Type to queue your next message..."` |
| **Textarea** | Enabled | Enabled (user CAN still type) |
| **Streaming cursor** | Hidden | Pulsing `\|` after last text |
| **Message bubble** | Static | `animate-pulse-border` class |

### Safety Nets

**1. Loading timeout (30s):**
```typescript
$effect(() => {
  if (isLoading) {
    loadingTimeoutId = setTimeout(() => {
      if (isLoading) {
        isLoading = false;  // Force reset
        messageQueue = [];
      }
    }, 30_000);
  }
});
```

**2. Cancel timeout (2s):**
If cancel response doesn't arrive within 2s, force-reset loading state.

**3. Tool status safety net:**
On `chat_complete`, any still-running tools are force-marked as complete (in case `tool_result` was missed).

**4. Stream resumption on page load:**
`checkForActiveStream()` → sends `check_stream` → backend checks `activeSessions` map → sends `stream_status` with accumulated content.

---

## 5. WebSocket Client

**File:** `app/src/lib/websocket/client.ts` (375 lines)

Singleton `WebSocketClient` class:

```typescript
const client = getWebSocketClient();  // Singleton
```

**Key methods:**
- `connect(userId?)` — Opens WebSocket to `/ws?clientId=...&userId=...`
- `send(type, data)` — Serializes to JSON and sends (queues if disconnected)
- `on(type, handler)` — Subscribe to message type, returns unsubscribe fn
- `onStatus(handler)` — Subscribe to connection status changes
- `isConnected()` — Returns `currentStatus === 'connected'`

**Auto-reconnect:** Exponential backoff: `min(2000 * 2^attempts, 30000)ms`

**Message batching:** Backend may batch multiple JSON messages in one frame (newline-separated). Client splits on `\n` and processes each.

**Message queue:** If WebSocket isn't open when `send()` is called, messages queue in memory and drain on next `onopen`.

---

## 6. Message Queuing & Barge-In

### Barge-In (new message while agent is processing)

When `sendMessage()` is called and `isLoading === true`:
1. Cancel active response: `client.send('cancel', { session_id })`
2. Mark streaming message as interrupted (`streaming=false`)
3. Force all running tools to complete
4. Set `isLoading = false`
5. Clear message queue
6. Fall through to send the new message immediately

**Key insight:** The new message supersedes everything. Queue is discarded.

### Message Queue (NOT currently used for text input)

The `messageQueue` state exists for edge cases (e.g., DM messages arriving). The main text input uses barge-in, not queuing. The queue pill tray UI is wired up but barge-in always fires first.

`processQueue()` runs after `handleChatComplete()` to send the next queued message if any remain.

---

## 7. Content Blocks & Message Structure

### Message Interface
```typescript
interface Message {
  id: string;
  role: 'user' | 'assistant' | 'system';
  content: string;              // Full text content
  contentHtml?: string;         // Pre-rendered HTML (from DB)
  timestamp: Date;
  toolCalls?: ToolCall[];       // All tool calls in this message
  streaming?: boolean;          // Currently being streamed
  thinking?: string;            // Extended thinking content
  contentBlocks?: ContentBlock[];  // Interleaved text/tool/image blocks
}

interface ContentBlock {
  type: 'text' | 'tool' | 'image';
  text?: string;                // For text blocks
  toolCallIndex?: number;       // Index into toolCalls[] for tool blocks
  imageData?: string;           // Base64 for inline images
  imageMimeType?: string;       // e.g. "image/png"
  imageURL?: string;            // URL for server-hosted images
}
```

### Content Block Assembly (during streaming)

Text chunks append to the last text block (or create new):
```typescript
if (blocks.length === 0 || blocks[blocks.length - 1].type !== 'text') {
  blocks.push({ type: 'text', text: chunk });
} else {
  blocks[blocks.length - 1].text += chunk;
}
```

Tool starts insert a new tool block. Tool results update the matching tool call. Images append image blocks.

The backend (`chat.go`) mirrors this exact pattern — maintaining `req.contentBlocks` for DB persistence.

---

## 8. Backend State Tracking

### ChatContext Maps

```go
type ChatContext struct {
  pending          map[string]*pendingRequest   // requestID → request info
  activeSessions   map[string]string            // sessionID → requestID
  pendingApprovals map[string]string            // approvalID → agentID
}
```

**`activeSessions`** is the backend equivalent of `isLoading` — it tracks which sessions have active requests. Used by:
- `handleCheckStream()` — tells frontend if a stream is active on page load
- `handleCancel()` — finds the request to clean up

### Fence Marker Buffering

Critical for emoji/character integrity:

```go
// Hold back 20 chars (fence markers are 18 chars)
safeLen := len(clean) - 20
// Back up to UTF-8 rune boundary
for safeLen > req.cleanSentLen && !utf8.RuneStart(clean[safeLen]) {
    safeLen--
}
// Send only the delta
delta = clean[req.cleanSentLen:safeLen]
```

On completion, flush the remaining 20 chars.

---

## 9. Draft Persistence

Auto-saves to `localStorage`:
```typescript
const DRAFT_STORAGE_KEY = 'nebo_companion_draft';

// On mount: load saved draft
onMount → localStorage.getItem(DRAFT_STORAGE_KEY) → inputValue

// On input change: auto-save
$effect(() => {
  if (inputValue) localStorage.setItem(key, inputValue);
  else localStorage.removeItem(key);
});

// On send: clear draft
clearDraft() → localStorage.removeItem(key)
```

---

## 10. Stream Resumption

When the page loads or reconnects:

1. `loadCompanionChat()` loads messages from DB
2. `checkForActiveStream()` sends `check_stream` to backend
3. Backend checks `activeSessions` — if active, sends `stream_status` with accumulated content
4. Frontend receives `stream_status`:
   - If `active=true`: sets `isLoading=true`, creates streaming message with accumulated content
   - If `active=false` and messages empty: requests introduction after 5s timeout

---

## 11. Key Files Reference

| File | Role |
|---|---|
| `app/src/lib/components/chat/ChatInput.svelte` | Input textarea, send/stop buttons, file drop |
| `app/src/routes/(app)/agent/+page.svelte` | Page logic: state, handlers, send/receive, scroll |
| `app/src/lib/websocket/client.ts` | Singleton WebSocket client, event routing |
| `internal/realtime/chat.go` | ChatContext: pending requests, stream processing, DB saves |
| `internal/realtime/hub.go` | Client hub: browser WebSocket connections, broadcast |
| `internal/agenthub/hub.go` | Agent hub: agent WebSocket, frame routing |
| `app/src/lib/components/chat/MessageGroup.svelte` | Message rendering: grouping, content blocks, tool cards |
| `app/src/lib/components/chat/ToolCard.svelte` | Tool execution display: running/complete/error states |
