# Webforms & Ask Widget — Internal Reference

The webforms system enables the agent to ask the user interactive questions mid-conversation and collect structured responses. This covers both the **Ask Widget** (agent-initiated structured prompts) and the **Approval Modal** (tool-policy-initiated yes/no/always prompts). Both share the same WebSocket plumbing but serve different purposes and have distinct UI components.

**No public documentation exists.** Everything below is derived from source code.

---

## System Overview

Two interaction patterns, one plumbing layer:

| Feature | Ask Widget | Approval Modal |
|---------|-----------|----------------|
| **Trigger** | Agent calls `agent(resource: message, action: ask)` | Tool execution hits `Policy.RequiresApproval()` |
| **UI** | Inline in message stream (4 widget types) | Modal overlay with Approve/Deny/Always |
| **Response type** | Free-form string (user's selection or typed text) | Boolean (approved) + optional "always" flag |
| **Persistence** | Stored in `contentBlocks` metadata on chat message | Not persisted (in-memory only) |
| **Blocking** | Blocks tool execution until user responds | Blocks tool execution until user responds |
| **Component** | `AskWidget.svelte` | `ApprovalModal.svelte` |

Both use the same pattern: Go channel blocks the agent goroutine, WebSocket delivers the request to browser, browser sends response back, Go channel unblocks.

---

## Widget Types

Five widget types defined in `AskWidget` (`internal/agent/tools/agent_tool.go:98-103`):

| Type | Renders As | Options Field | Default Behavior |
|------|-----------|---------------|------------------|
| `buttons` | Row of outlined buttons | Required — each option is a button | N/A |
| `confirm` | Row of buttons (like `buttons`) | Optional — defaults to `["Yes", "No"]` | Yes/No buttons |
| `select` | Dropdown + OK button | Required — each option is a `<option>` | Disabled OK until selection |
| `radio` | Vertical radio list + Submit button | Required — each option is a radio input | Disabled Submit until one selected |
| `checkbox` | Vertical checkbox list + Submit button | Required — each option is a checkbox | Disabled Submit until ≥1 checked; submits comma-separated string |

Go struct:
```go
// internal/agent/tools/agent_tool.go:98-103
type AskWidget struct {
    Type    string   `json:"type"`              // "buttons", "select", "confirm", "radio", "checkbox"
    Label   string   `json:"label,omitempty"`
    Options []string `json:"options,omitempty"` // for buttons/select
    Default string   `json:"default,omitempty"` // pre-filled value
}
```

TypeScript interface:
```typescript
// app/src/lib/components/chat/AskWidget.svelte:2-7
export interface AskWidgetDef {
    type: 'buttons' | 'select' | 'confirm' | 'radio' | 'checkbox';
    label?: string;
    options?: string[];
    default?: string;
}
```

---

## End-to-End Data Flow

### Ask Widget Flow

```
Agent LLM decides to ask user
  │
  ▼
agent(resource: message, action: ask, prompt: "...", widgets: [...])
  │
  ▼
AgentDomainTool.messageAsk()              ← agent_tool.go:1191-1232
  │  Validates prompt, defaults widgets to confirm(Yes/No)
  │  Generates UUID requestID
  │  Calls askCallback(ctx, requestID, prompt, widgets)
  │
  ▼
agentState.requestAsk()                   ← cmd/nebo/agent.go:190-222
  │  Creates buffered chan string (capacity 1)
  │  Stores in pendingAsk[requestID]
  │  Sends frame: {type: "ask_request", id: requestID, payload: {prompt, widgets}}
  │  Blocks on: select { case <-respCh / case <-ctx.Done() }
  │
  ▼
Hub.handleFrame() case "ask_request"      ← hub.go:598-613
  │  Extracts prompt + widgets from payload
  │  Calls registered askHandler(agentID, requestID, prompt, widgetsRaw)
  │
  ▼
ChatContext.handleAskRequest()            ← chat.go:204-239
  │  Stores pendingAsks[requestID] = agentID
  │  Appends contentBlock{Type:"ask"} to active pendingRequest
  │  Broadcasts to all browser clients: {type: "ask_request", ...}
  │
  ▼
Browser: handleAskRequest()              ← agent/+page.svelte:1081-1102
  │  Appends ask block to currentStreamingMessage.contentBlocks
  │
  ▼
MessageGroup.svelte resolves block       ← MessageGroup.svelte:127-134
  │
  ▼
AskWidget.svelte renders                 ← AskWidget.svelte:44-108
  │  Shows prompt text + widget(s) based on type
  │  User interacts (clicks button / selects option / types text)
  │  Calls submit(value)
  │
  ▼
Browser: handleAskSubmit()               ← agent/+page.svelte:1104-1140
  │  Sends: client.send('ask_response', {request_id, value})
  │  Updates contentBlocks locally with askResponse
  │  Updates messages array for non-streaming messages
  │
  ▼
ChatContext.handleAskResponse()           ← chat.go:241-279
  │  Removes from pendingAsks
  │  Updates contentBlock.AskResponse in pending request
  │  Calls hub.SendAskResponse(agentID, requestID, value)
  │
  ▼
Hub.SendAskResponse()                    ← hub.go:413-421
  │  Sends frame: {type: "ask_response", id: requestID, payload: {value}}
  │
  ▼
agentState.handleAskResponse()           ← cmd/nebo/agent.go:224-235
  │  Sends value to pendingAsk[requestID] channel
  │
  ▼
agentState.requestAsk() unblocks         ← cmd/nebo/agent.go:216-218
  │  Returns user's string value
  │
  ▼
AgentDomainTool.messageAsk() returns     ← agent_tool.go:1229-1231
  │  ToolResult{Content: response}
  │
  ▼
Agent continues with user's answer
```

### Approval Flow

```
Agent executes tool (e.g., shell command)
  │
  ▼
Registry.Execute()                        ← registry.go
  │  Checks policy.RequiresApproval(cmd)
  │  If yes → calls policy.ApprovalCallback(ctx, toolName, input)
  │
  ▼
agentState.requestApproval()              ← cmd/nebo/agent.go:122-175
  │  Creates buffered chan approvalResponse (capacity 1)
  │  Stores in pendingApproval[requestID]
  │  Sends frame: {type: "approval_request", id, payload: {tool, input}}
  │  Blocks on: select { case <-respCh / case <-ctx.Done() }
  │
  ▼
Hub.handleFrame() case "approval_request" ← hub.go:582-596
  │  Calls approvalHandler(agentID, requestID, toolName, inputRaw)
  │
  ▼
ChatContext.handleApprovalRequest()       ← chat.go:151-173
  │  Stores pendingApprovals[requestID] = agentID
  │  Broadcasts: {type: "approval_request", request_id, tool, input}
  │
  ▼
Browser: ApprovalModal renders            ← ApprovalModal.svelte
  │  Shows tool name + formatted input
  │  User clicks: Deny / Once / Always
  │
  ▼
Browser sends approval_response           ← agent/+page.svelte:1194-1220
  │  client.send('approval_response', {request_id, approved, always?})
  │
  ▼
ChatContext.handleApprovalResponse()      ← chat.go:175-202
  │  Removes from pendingApprovals
  │  Calls hub.SendApprovalResponseWithAlways(agentID, requestID, approved, always)
  │
  ▼
agentState.handleApprovalResponse()      ← cmd/nebo/agent.go:177-188
  │  Sends to pendingApproval[requestID].RespCh
  │
  ▼
agentState.requestApproval() unblocks    ← cmd/nebo/agent.go:151-171
  │  If always=true, adds command to policy allowlist
  │  Returns approved bool
  │
  ▼
Tool execution proceeds or is rejected
```

---

## Go Data Structures

### Agent-Side State (`cmd/nebo/agent.go`)

```go
// agent.go:54-58
type approvalResponse struct {
    Approved bool
    Always   bool
}

// agent.go:60-65
type pendingApprovalInfo struct {
    RespCh   chan approvalResponse
    ToolName string
    Input    json.RawMessage
}

// agent.go:67-104 (relevant fields)
type agentState struct {
    pendingApproval map[string]*pendingApprovalInfo  // :71
    approvalMu      sync.RWMutex                     // :72
    pendingAsk      map[string]chan string            // :73
    pendingAskMu    sync.RWMutex                     // :74
    policy          *tools.Policy                    // :76
}
```

### Server-Side State (`internal/realtime/chat.go`)

```go
// chat.go:22-44
type ChatContext struct {
    pending          map[string]*pendingRequest  // :27 — requestID → streaming state
    pendingApprovals map[string]string           // :35 — approvalID → agentID
    pendingAsks      map[string]string           // :39 — requestID → agentID
}

// chat.go:54-63
type contentBlock struct {
    Type          string          `json:"type"`                    // "text", "tool", "image", or "ask"
    AskRequestID  string          `json:"askRequestId,omitempty"`
    AskPrompt     string          `json:"askPrompt,omitempty"`
    AskWidgets    json.RawMessage `json:"askWidgets,omitempty"`
    AskResponse   string          `json:"askResponse,omitempty"`
}
```

### Hub Handler Types (`internal/agenthub/hub.go`)

```go
// hub.go:44
type ApprovalRequestHandler func(agentID, requestID, toolName string, input json.RawMessage)

// hub.go:47
type AskRequestHandler func(agentID, requestID, prompt string, widgets json.RawMessage)
```

### Tool Types (`internal/agent/tools/`)

```go
// agent_tool.go:98-103
type AskWidget struct { ... }

// agent_tool.go:107
type AskCallback func(ctx context.Context, requestID, prompt string, widgets []AskWidget) (string, error)

// policy.go:32
type ApprovalCallback func(ctx context.Context, toolName string, input json.RawMessage) (bool, error)
```

---

## WebSocket Frame Types

Six frame types carry ask/approval traffic:

| Frame Type | Direction | Payload |
|------------|-----------|---------|
| `ask_request` | Agent → Hub → Browser | `{prompt, widgets}` |
| `ask_response` | Browser → Hub → Agent | `{request_id, value}` |
| `approval_request` | Agent → Hub → Browser | `{tool, input}` |
| `approval_response` | Browser → Hub → Agent | `{request_id, approved, always?}` |

All frames include `id` (the requestID) at the top level.

---

## Persistence Model

**Ask widgets: persisted via contentBlocks metadata.**

When the streaming response completes, `buildMetadata()` (`chat.go:1094-1110`) serializes all `contentBlocks` (including ask blocks with their `askResponse`) into the `metadata` JSON column on the `chat_messages` table. On page reload, the frontend parses metadata → contentBlocks → renders answered ask widgets as read-only badges.

**Approval requests: NOT persisted.**

Approval state lives entirely in-memory (`pendingApproval` maps on both agent and server side). If the browser disconnects mid-approval, the approval is lost and the agent's blocking goroutine will eventually time out via context cancellation.

---

## Callback Wiring

The ask callback is wired during agent startup:

```go
// cmd/nebo/agent.go:2131-2133
agentTool.SetAskCallback(func(ctx context.Context, reqID, prompt string, widgets []tools.AskWidget) (string, error) {
    return state.requestAsk(ctx, reqID, prompt, widgets)
})
```

The approval callback is wired via the tool policy:

```go
// Policy.ApprovalCallback is set during agent initialization
// and called by Registry.Execute() when a tool requires approval
```

Both callbacks bridge the tool system → agentState → WebSocket transport → browser UI.

---

## Handler Registration Chain

```go
// chat.go:98-100 — ChatContext registers itself with the Hub
hub.SetApprovalHandler(c.handleApprovalRequest)
hub.SetAskHandler(c.handleAskRequest)

// client.go:131-148 — Client message handlers route browser responses
SetApprovalResponseHandler(func(c *Client, msg *Message) {
    go chatCtx.handleApprovalResponse(msg)
})
SetAskResponseHandler(func(c *Client, msg *Message) {
    go chatCtx.handleAskResponse(msg)
})
```

---

## Error Handling & Edge Cases

| Scenario | Behavior |
|----------|----------|
| No web UI connected | `askCallback == nil` → returns error: "Interactive prompts require the web UI" (`agent_tool.go:1192-1196`) |
| Empty prompt | Returns error: "'prompt' (or 'text') is required for ask action" (`agent_tool.go:1204-1208`) |
| No widgets specified | Defaults to `confirm` with `["Yes", "No"]` (`agent_tool.go:1211-1218`) |
| Context cancelled (timeout) | `requestAsk()` returns `ctx.Err()` via select on `ctx.Done()` (`agent.go:219-221`) |
| Browser disconnects mid-ask | Agent goroutine blocks until context cancellation |
| Multiple pending requests | Server iterates `c.pending` map, appends ask block to first active request (`chat.go:215-222`) |
| "Always" approval | Adds command to runtime allowlist — survives for session but not persisted to disk (`agent.go:154-170`) |
| Duplicate response | Buffered channel (capacity 1) + default case prevents goroutine leak (`agent.go:230-233`) |

---

## Frontend Component Details

### AskWidget.svelte (`app/src/lib/components/chat/AskWidget.svelte`)

- **108 lines**, Svelte 5 component with `$props()`, `$state`, `$derived`
- Props: `requestId`, `prompt`, `widgets[]`, `response?`, `onSubmit` callback
- Once `response` is set (non-null), widget becomes read-only — shows a `badge badge-primary` with the response value
- Renders one widget per entry in the `widgets` array (typically one)
- DaisyUI classes throughout (btn, select, input, badge)

### ApprovalModal.svelte (`app/src/lib/components/ui/ApprovalModal.svelte`)

- Modal overlay with three buttons: Deny, Once, Always
- Shows tool name and formatted input (bash → command string, others → path or JSON)
- Props: `request` (nullable — null hides the modal), `onApprove`, `onApproveAlways`, `onDeny`

### MessageGroup.svelte Integration (`app/src/lib/components/chat/MessageGroup.svelte`)

- `ContentBlock` interface includes ask fields (`askRequestId`, `askPrompt`, `askWidgets`, `askResponse`) — lines 17-28
- Block resolution at lines 127-134: ask blocks are pushed to `resolvedBlocks` array
- Rendering at lines 217-224: `<AskWidget>` with `onSubmit` prop delegating to parent's `onAskSubmit`

### agent/+page.svelte Integration

- `handleAskRequest()` at line 1081: appends ask contentBlock to streaming message
- `handleAskSubmit()` at line 1104: sends `ask_response` WebSocket message, updates local state
- Approval handlers at lines 1194-1220: `handleApprove()`, `handleApproveAlways()`, `handleDeny()`
- WebSocket listener registered at line 291: `client.on('ask_request', handleAskRequest)`

---

## Tool Invocation Pattern

The agent (LLM) invokes the ask widget via the STRAP agent tool:

```
agent(resource: message, action: ask, prompt: "Which option do you prefer?", widgets: [
  {type: "buttons", label: "Choose one:", options: ["Option A", "Option B", "Option C"]}
])
```

Default (no widgets specified — simple confirmation):
```
agent(resource: message, action: ask, prompt: "Should I proceed with the deployment?")
→ defaults to: widgets: [{type: "confirm", options: ["Yes", "No"]}]
```

---

## Critical Files

| File | Lines | Purpose |
|------|-------|---------|
| `internal/agent/tools/agent_tool.go` | 98-107, 1191-1232 | AskWidget type, AskCallback type, messageAsk handler |
| `internal/agent/tools/policy.go` | 12-58 | PolicyLevel, AskMode, ApprovalCallback, SafeBins |
| `cmd/nebo/agent.go` | 54-65, 67-104, 122-235, 2131-2133 | Agent-side state, request/handle functions, callback wiring |
| `internal/agenthub/hub.go` | 44-47, 69-71, 384-421, 598-613 | Handler types, registration, send functions, frame routing |
| `internal/realtime/chat.go` | 22-44, 54-63, 98-100, 131-148, 151-279, 1094-1110 | ChatContext state, handler registration, ask/approval handlers, metadata persistence |
| `app/src/lib/components/chat/AskWidget.svelte` | 1-108 | Ask widget UI component (4 widget types) |
| `app/src/lib/components/ui/ApprovalModal.svelte` | — | Tool approval modal (Deny/Once/Always) |
| `app/src/lib/components/chat/MessageGroup.svelte` | 17-28, 127-134, 217-224 | ContentBlock types, ask block resolution & rendering |
| `app/src/routes/(app)/agent/+page.svelte` | 68-74, 291, 1081-1140, 1194-1220 | ContentBlock type, WS listener, ask/approval handlers |
