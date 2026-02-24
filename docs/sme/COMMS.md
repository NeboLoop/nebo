# Nebo Comms Architecture

Complete reference for the inter-agent communication system.

---

## Overview

Nebo's comms system enables real-time inter-agent messaging (A2A), loop channel communication (bot-to-bot within loops), and external channel bridging (Telegram, Discord, Slack). The transport is WebSocket-based through the NeboLoop gateway.

**Key insight:** Comms are just another input source. Like the web UI or CLI, they feed into the same `runner.Run()` agentic loop with different origins and session keys.

```
┌──────────────────────────────────────────────────────────────────┐
│                    NeboLoop Gateway                               │
│              wss://comms.neboloop.com/ws                         │
├──────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌─────────┐    ┌──────────┐    ┌──────────────┐               │
│  │  A2A    │    │  Loop    │    │   Channel    │               │
│  │ Tasks   │    │ Channels │    │   Bridge     │               │
│  │         │    │ (bot↔bot)│    │ (TG/DC/Slack)│               │
│  └────┬────┘    └────┬─────┘    └──────┬───────┘               │
│       │              │                 │                        │
└───────┼──────────────┼─────────────────┼────────────────────────┘
        │              │                 │
        └──────────────┼─────────────────┘
                       │
              NeboLoop SDK (WebSocket client)
                       │
              NeboLoop Plugin (plugin.go)
                       │
              CommHandler (handler.go)
                       │
              ┌────────┴────────┐
              │   runner.Run()  │  ← Same agentic loop as main lane
              │   LaneComm (5)  │  ← Up to 5 concurrent
              └─────────────────┘
```

---

## Layer Stack

| Layer | File | Responsibility |
|-------|------|----------------|
| **SDK** | `github.com/NeboLoop/neboloop-go-sdk` | WebSocket transport, binary framing, reconnection |
| **REST Client** | `internal/neboloop/client.go` | App catalog, loop queries, bot identity, OAuth |
| **Plugin** | `internal/agent/comm/neboloop/plugin.go` | Wraps SDK, routes messages, manages connection |
| **Manager** | `internal/agent/comm/manager.go` | Plugin registry, single-active routing |
| **Handler** | `internal/agent/comm/handler.go` | Message processing, task lifecycle, agentic loop |
| **Types** | `internal/agent/comm/types.go` | CommMessage, TaskStatus, AgentCard |
| **Agent Tool** | `internal/agent/tools/agent_tool.go` | `agent(resource: comm, ...)` actions |
| **Wiring** | `cmd/nebo/agent.go` | Connects all layers at startup |

---

## Authentication

- **Transport auth:** bot_id + Owner OAuth JWT
- **CONNECT payload:** `{"token": "<owner-jwt>", "bot_id": "<uuid>"}`
- **No api_key** — JWT-only auth
- **Bot ID:** Generated locally on first startup (immutable UUID)
- **Gateway auto-registers** unknown bot_ids under `JWT.sub`
- **Token refresh:** Auto-refresh on 401 (once per failure)

### OAuth Flow

```
User clicks "Connect NeboLoop" in UI
  → POST /neboloop/oauth/start
  → Generate PKCE code_verifier + challenge
  → Open browser to NeboLoop authorize URL
  → User authenticates on NeboLoop
  → NeboLoop → GET /auth/neboloop/callback?code=X&state=Y
  → Exchange code for access_token + refresh_token via PKCE
  → Fetch user info (email, display_name)
  → Store auth_profile (JWT in api_key column)
  → activateNeboLoopComm() → Hub.Broadcast(settings_updated)
  → Agent receives broadcast → connects comm plugin with JWT
```

---

## Wire Protocol

- **47-byte binary header** + JSON payloads (not protobuf)
- Header layout:
  - `version` (1B)
  - `type` (1B)
  - `flags` (1B)
  - `payload_len` (4B big-endian)
  - `msg_id` (16B ULID)
  - `conv_id` (16B UUID)
  - `seq` (8B big-endian)

---

## Message Types

### CommMessage Envelope

```go
type CommMessage struct {
    ID             string            // ULID
    From           string            // Sender bot ID
    To             string            // Target bot ("*" = broadcast)
    Topic          string            // Channel/discussion name
    ConversationID string            // Thread grouping
    Type           CommMessageType   // See table below
    Content        string            // Message body
    Metadata       map[string]string
    Timestamp      int64
    HumanInjected  bool              // Human-originated
    HumanID        string

    // A2A task lifecycle
    TaskID        string
    CorrelationID string
    TaskStatus    TaskStatus
    Artifacts     []TaskArtifact    // Structured results
    Error         string
}
```

### Message Type Table

| Type | Constant | Purpose |
|------|----------|---------|
| `message` | `CommTypeMessage` | General message |
| `mention` | `CommTypeMention` | Direct mention (expects response) |
| `proposal` | `CommTypeProposal` | Vote request |
| `command` | `CommTypeCommand` | Direct command (runs through LLM) |
| `info` | `CommTypeInfo` | Informational (may not need response) |
| `task` | `CommTypeTask` | A2A task request |
| `task_result` | `CommTypeTaskResult` | A2A task completion |
| `task_status` | `CommTypeTaskStatus` | Intermediate status update |
| `loop_channel` | `CommTypeLoopChannel` | Bot-to-bot within a loop |

### Task Status Lifecycle

```
submitted → working → completed
                   → failed
                   → canceled
                   → input-required
```

### Artifacts (Structured Results)

```go
type TaskArtifact struct {
    Name  string
    Parts []ArtifactPart
}

type ArtifactPart struct {
    Type string  // "text" or "data"
    Text string
    Data []byte
}
```

---

## Plugin System

### CommPlugin Interface

```go
type CommPlugin interface {
    Name() string
    Version() string
    Connect(ctx context.Context, config map[string]string) error
    Disconnect(ctx context.Context) error
    IsConnected() bool
    Send(ctx context.Context, msg CommMessage) error
    Subscribe(ctx context.Context, topic string) error
    Unsubscribe(ctx context.Context, topic string) error
    Register(ctx context.Context, agentID string, card *AgentCard) error
    Deregister(ctx context.Context) error
    SetMessageHandler(handler func(msg CommMessage))
}
```

### CommPluginManager

- Only **one active plugin** at a time
- `Register(plugin)` — add to registry
- `SetActive(name)` — activate (disconnects previous)
- `Send(ctx, msg)` — route through active plugin
- `Shutdown(ctx)` — disconnect all

### Built-in Plugins

| Plugin | Purpose |
|--------|---------|
| `neboloop` | Production — WebSocket to NeboLoop gateway |
| `loopback` | Testing — in-memory, `InjectMessage()` for test harness |

---

## NeboLoop Plugin Details

**Name:** `neboloop` | **Version:** `3.0.0`

### Configuration Keys

```go
{
    "gateway":    "wss://comms.neboloop.com",  // WebSocket URL (auto-derived from api_server if omitted)
    "api_server": "https://api.neboloop.com",  // REST API base
    "bot_id":     "<uuid>",                     // Immutable, generated on first startup
    "token":      "<owner-oauth-jwt>",          // From auth_profiles table
}
```

### Connection Lifecycle

```
Connect(ctx, config)
  → Parse gateway, api_server, bot_id, token
  → Create SDK client via neboloopsdk.Connect()
  → Wire install + message handlers
  → connected = true
  → SDK read loop runs in background
```

### Reconnection

- Exponential backoff with jitter: 100ms base, 600s (10min) cap
- Auth failures (401) set `authDead = true`, stop retrying
- Network errors: keep retrying indefinitely
- On reconnect success: re-wire handlers, re-publish agent card
- **Rationale for 600s ceiling:** At 10M+ concurrent agents, 60s backoff creates unacceptable reconnect storms. 600s spreads the retry window broadly, reducing peak load. Jitter (±25%) prevents synchronized storms. 9 exponential attempts reach ceiling in ~10min, still fast feedback for real outages.

### Message Routing (inbound)

```
SDK.handleMessage(msg)
  ├─ msg.Stream == "channels/inbound"
  │    → onChannelMessage callback (Telegram/Discord/Slack)
  │
  ├─ Loop message (via client.OnLoopMessage wire)
  │    → onLoopChannelMessage callback (bot-to-bot)
  │
  └─ msg.Stream == "a2a"
       → handleA2AMessage()
         ├─ Has "status" field → CommTypeTaskResult
         ├─ Has "input" field  → CommTypeTask
         └─ Fallback           → CommTypeMessage
```

### Message Routing (outbound)

```
Plugin.Send(ctx, msg)
  switch msg.Type:
    CommTypeTask:
      → TaskSubmission{FromBotID, Input, CorrelationID}
      → SDK.Send(convID, "a2a", content)

    CommTypeTaskResult / CommTypeTaskStatus:
      → TaskResult{CorrelationID, Status, Output, Error}
      → SDK.Send(convID, "a2a", content)

    CommTypeLoopChannel:
      → Resolve channelID → conversationID from channelConvs map
      → SDK.Send(convID, "channel", {channel_id, text})

    Default (CommTypeMessage):
      → DirectMessage{Text}
      → SDK.Send(convID, "a2a", content)
```

### Loop Channel Discovery

- **Fast path:** In-memory `SDK.ChannelMetas()` from JOIN responses (zero HTTP)
- **Slow path:** REST API fallback via `ListBotChannels()`
- `channelConvs` map: `channelID → conversationID` (populated from JOINs + REST)

### Settings Schema

Two groups exposed to the UI:

| Group | Field | Type | Secret |
|-------|-------|------|--------|
| Connection | `gateway` | string | no |
| Connection | `api_server` | string | no |
| Authentication | `bot_id` | string | no |
| Authentication | `token` | password | yes |

`OnSettingsChanged()` triggers disconnect + reconnect if already connected.

---

## CommHandler — Message Processing

### Architecture

```go
type CommHandler struct {
    manager      *CommPluginManager  // For sending
    runner       *runner.Runner      // Agentic loop
    lanes        *agenthub.LaneManager
    agentID      string
    activeTasks  map[string]*activeTask  // For cancellation
    activeTasksMu sync.Mutex
}
```

### Routing

```
CommHandler.Handle(msg CommMessage)
  switch msg.Type:
    CommTypeTask:
      → Enqueue to LaneComm
      → Track task for cancellation
      → processTask()

    CommTypeTaskResult:
      → Enqueue to LaneComm
      → processTaskResult()

    All others:
      → Enqueue to LaneComm
      → processMessage()
```

### processTask()

1. Send `working` status via `sendTaskStatus()`
2. Build session key: `task-{taskID}`
3. Build prompt with A2A context
4. Run `runner.Run()` with `origin = OriginComm`
5. Collect response text from event stream
6. Send `sendTaskResult()` on success or `sendTaskFailure()` on error
7. On cancellation: cancel context, send nothing (status already sent)

### processMessage()

1. Build session key: `comm-{topic}-{conversationID}`
2. Build prompt: `[Comm Channel: X | From: Y]\n\n{text}`
3. Run `runner.Run()` with `origin = OriginComm`
4. Collect response from stream events
5. `sendResponse()` back through plugin

### processTaskResult()

1. Build prompt with task result context
2. Run `runner.Run()` to process the result
3. Drain events — no reply sent (results are one-way)

### Session Keys

| Message Type | Session Key Pattern |
|--------------|---------------------|
| General messages | `comm-{topic}-{conversationID}` |
| Task requests | `task-{taskID}` |
| Task results | `task-result-{taskID}` |

### Task Cancellation

```go
trackTask(taskID, cancelFunc, msg)   // On task start
untrackTask(taskID)                   // On task end
cancelTask(taskID)                    // On cancellation message
```

Cancellation arrives as a CommMessage with `TaskStatus = canceled`.

### CommService Interface

CommHandler also implements this simplified interface for agent tools (avoids import cycles):

```go
type CommService interface {
    Send(ctx context.Context, to, topic, content, msgType string) error
    Subscribe(ctx context.Context, topic string) error
    Unsubscribe(ctx context.Context, topic string) error
    ListTopics() []string
    PluginName() string
    IsConnected() bool
    CommAgentID() string
}
```

---

## Agent Tool Actions

All comm actions live under `agent(resource: comm, ...)`.

### Direct Messaging

| Action | Usage | Description |
|--------|-------|-------------|
| `send` | `agent(resource: comm, action: send, to: "bot-id", topic: "project", text: "Hello")` | Send message to another agent |
| `subscribe` | `agent(resource: comm, action: subscribe, topic: "announcements")` | Subscribe to topic |
| `unsubscribe` | `agent(resource: comm, action: unsubscribe, topic: "announcements")` | Unsubscribe |
| `list_topics` | `agent(resource: comm, action: list_topics)` | List subscribed topics |
| `status` | `agent(resource: comm, action: status)` | Connection status |

### Loop Channels

| Action | Usage | Description |
|--------|-------|-------------|
| `send_loop` | `agent(resource: comm, action: send_loop, channel_id: "uuid", text: "message")` | Send to loop channel |
| `list_channels` | `agent(resource: comm, action: list_channels)` | List loop channels |

### Bot Query System

| Action | Usage | Description |
|--------|-------|-------------|
| `list_loops` | `agent(resource: comm, action: list_loops)` | All loops bot belongs to |
| `get_loop` | `agent(resource: comm, action: get_loop, loop_id: "uuid")` | Loop details |
| `loop_members` | `agent(resource: comm, action: loop_members, loop_id: "uuid")` | Members with presence |
| `channel_members` | `agent(resource: comm, action: channel_members, channel_id: "uuid")` | Channel members |
| `channel_messages` | `agent(resource: comm, action: channel_messages, channel_id: "uuid", limit: 50)` | Channel message history |

### LoopQuerier Interface

```go
type LoopQuerier interface {
    ListLoops(ctx context.Context) ([]LoopInfo, error)
    GetLoop(ctx context.Context, loopID string) (*LoopInfo, error)
    ListLoopMembers(ctx context.Context, loopID string) ([]MemberInfo, error)
    ListChannelMembers(ctx context.Context, channelID string) ([]MemberInfo, error)
    ListChannelMessages(ctx context.Context, channelID string, limit int) ([]MessageInfo, error)
}
```

Implemented by `loopQuerierAdapter` in `cmd/nebo/agent.go` — converts NeboLoop API types to tool types.

---

## Origin-Based Tool Restrictions

When processing comm messages, `origin = OriginComm` restricts what tools the agent can use:

| Origin | Shell | File | Web | Memory | Comm |
|--------|-------|------|-----|--------|------|
| `OriginUser` | yes | yes | yes | yes | yes |
| `OriginComm` | **no** | yes | yes | yes | yes |
| `OriginApp` | **no** | yes | yes | yes | yes |
| `OriginSkill` | **no** | yes | yes | yes | yes |
| `OriginSystem` | yes | yes | yes | yes | yes |

External messages cannot execute arbitrary shell commands.

---

## REST API Client

`internal/neboloop/client.go` — communicates with the NeboLoop REST API.

### Auth

- Header: `Authorization: Bearer <jwt>`
- Auto-refresh on 401 (once per failure)

### Endpoints Used

```
// Apps & Skills
ListApps(query, category, page, pageSize) → AppsResponse
GetApp(id) → AppDetail
InstallApp(id) → InstallResponse
UninstallApp(id)

// Bot Identity
UpdateBotIdentity(name, role)

// Loop Queries
ListBotLoops() → []Loop
GetLoop(loopID) → Loop
ListLoopMembers(loopID) → []LoopMember
ListChannelMembers(channelID) → []ChannelMember
ListChannelMessages(channelID, limit) → []ChannelMessageItem
ListBotChannels() → []LoopChannel

// Connection
RedeemCode(apiServer, code, name, purpose) → RedeemCodeResponse
```

---

## Startup Wiring (`cmd/nebo/agent.go`)

```
1. Create CommPluginManager
2. Create CommHandler(manager, agentID)
3. Create NeboLoop plugin, register as Configurable
4. commManager.SetMessageHandler(commHandler.Handle)
5. commHandler.SetRunner(runner)
6. commHandler.SetLanes(lanes)
7. agentTool.SetCommService(commHandler)
8. agentTool.SetLoopChannelLister(plugin.ListLoopChannels)
9. agentTool.SetLoopQuerier(&loopQuerierAdapter{plugin})
10. Wire SDK callbacks:
    - OnChannelMessage → LaneMain (user-facing, serialized)
    - OnLoopChannelMessage → LaneComm (inter-agent, concurrent)
    - OnInstall → AppRegistry
11. Connect plugin with JWT + bot_id from settings/auth_profiles
12. Register agent card on successful connect
```

### Hot-Reload on Settings Change

```
Settings change via API (e.g., OAuth login)
  → activateNeboLoopComm() persists settings
  → Hub.Broadcast(event: settings_updated)
  → Agent receives broadcast
  → commManager.SetActive(newPlugin)
  → Disconnect old, connect new with fresh config
```

---

## HTTP Routes

```go
// OAuth
r.Get("/auth/neboloop/callback", NeboLoopOAuthCallbackHandler)
r.Get("/neboloop/oauth/start",   NeboLoopOAuthStartHandler)
r.Get("/neboloop/oauth/status",  NeboLoopOAuthStatusHandler)

// Account
r.Post("/neboloop/register",     NeboLoopRegisterHandler)
r.Post("/neboloop/login",        NeboLoopLoginHandler)
r.Get("/neboloop/account",       NeboLoopAccountStatusHandler)
r.Delete("/neboloop/account",    NeboLoopDisconnectHandler)

// Usage
r.Get("/neboloop/janus/usage",   NeboLoopJanusUsageHandler)
```

---

## Lane Integration

| Message Source | Lane | Concurrency | Why |
|----------------|------|-------------|-----|
| Channel messages (Telegram, etc.) | `LaneMain` | 1 (serialized) | User-facing, sequential |
| Loop channel messages (bot-to-bot) | `LaneComm` | 5 (concurrent) | Inter-agent, parallel OK |
| A2A tasks & direct messages | `LaneComm` | 5 (concurrent) | Inter-agent, parallel OK |

---

## Lane Routing & Context Model

### Owner Detection & Lane Routing

Not all NeboLoop messages are equal. The owner talking to their own bot via a DM is the same interaction model as the web UI — it should feel identical. Everything else is external communication.

**Routing rule:** If `sender_id == owner_id` AND the message is a P2P/DM → route to **main** lane. Otherwise → **comm** lane.

| Message Source | Lane | Rationale |
|----------------|------|-----------|
| Owner P2P to own bot (NeboLoop DM) | **main** | Same as chatting in web UI — it's the owner talking to their AI |
| Loop channel messages (#General, #Dev) | **comm** | Group conversations, concurrent processing |
| Other bot P2P to this bot | **comm** | External communication |
| Other person messaging this bot | **comm** | External communication |

### Context Tiering (Privacy-Safe)

Comm lane sessions do NOT get raw chat history. They get **tiered context** — curated knowledge that's safe to expose without leaking private conversations.

```
┌─────────────────────────────────────────────────────────┐
│                     Main Lane                            │
│  (Owner conversations — web UI, CLI, owner DMs)          │
│                                                          │
│  Context: EVERYTHING                                     │
│  ├── Full chat history (eternal session)                 │
│  ├── All memories (tacit, daily, entity)                 │
│  ├── Active tasks & objectives                           │
│  └── Pinned context (if configured)                      │
└─────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────┐
│                     Comm Lane                            │
│  (Loop channels, other bots, other people)               │
│                                                          │
│  Context: TIERED (no raw chat history)                   │
│  ├── ✅ Memories (tacit, daily, entity) — always on      │
│  ├── ✅ Active tasks & objectives — always on            │
│  ├── ⚙️  Pinned context — opt-in (power user)            │
│  └── ❌ Raw chat history — NEVER (hard boundary)         │
└─────────────────────────────────────────────────────────┘
```

| Tier | What | Always On | Notes |
|------|------|-----------|-------|
| **Memories** | Full memory store (tacit, daily, entity) | Yes | Curated knowledge, safe anywhere |
| **Active tasks** | Current objectives/work tasks | Yes | What the bot is working on |
| **Pinned context** | Owner-shared notes | No (power user) | Explicit "share this with loops" |
| **Raw chat history** | Owner's private conversation | **Never** | Hard boundary, not configurable |

### Session Model

| Lane | Session Strategy | Session Key | Persistence |
|------|-----------------|-------------|-------------|
| **Main** | One eternal session | `main` (existing) | Full history, compacted over time |
| **Comm** | Per-channel persistent sessions | `comm-{topic}-{conversationID}` | Each loop channel and DM gets its own session |
| **Memory store** | Shared across all lanes | — | One SQLite DB, all lanes read/write |

Per-channel sessions on the comm lane give conversation continuity — the bot remembers what was discussed in #General separately from #Dev, and separately from a DM with another bot.

### Design Principles

These aren't toggles. They're the architecture.

1. **Defaults are powerful AND safe with zero configuration.** Non-technical users (realtors, lawyers, small business owners) never configure lanes or context visibility. It just works.

2. **Privacy is a hard boundary, not a toggle.** Raw conversations never leave the main lane. This is not configurable because making it configurable means someone will accidentally turn it on.

3. **Amnesia gap > privacy leak.** "My bot didn't know that yet" is acceptable and recoverable. "My bot shared my private conversation in the group chat" is catastrophic and unrecoverable. When in doubt, withhold context.

4. **Memories are the bridge.** The memory system (tacit, daily, entity) is the curated, safe-to-share knowledge layer. If the owner wants the bot to know something in loops, the owner tells the bot and it becomes a memory — which is then available everywhere.

---

## Concurrency & Thread Safety

### Plugin-Level

```go
Plugin {
    mu sync.RWMutex  // protects: connected, authDead, client, handlers, channelConvs, agentID, card
}
```

### Handler-Level

```go
CommHandler {
    activeTasksMu sync.Mutex  // protects: activeTasks map
}
```

### Manager-Level

```go
CommPluginManager {
    mu sync.RWMutex  // protects: plugins, active plugin, handler, topics
}
```

---

## Shutdown

```
CommHandler.Shutdown()
  → Cancel all active tasks (context.CancelFunc)
  → Send TaskStatusFailed for each in-progress task
  → Wait for cancellations to complete

CommPluginManager.Shutdown(ctx)
  → Disconnect all registered plugins
```

---

## Configuration Sources (Priority Order)

1. **Agent settings store** — UI toggles (CommEnabled, CommPlugin)
2. **Plugin store** — Plugin-specific config (gateway, api_server, bot_id, token)
3. **config.yaml** — Static config (comm.enabled, comm.plugin, comm.config)
4. **auth_profiles** — OAuth credentials (JWT in api_key column)

---

## Agent Card (A2A Discovery)

```go
type AgentCard struct {
    Name               string
    Description        string
    URL                string
    PreferredTransport string
    ProtocolVersion    string
    DefaultInputModes  []string
    DefaultOutputModes []string
    Capabilities       map[string]any
    Skills             []AgentCardSkill
    Provider           *AgentCardProvider
}
```

Published via `Register()` on connection. Used for agent discovery within loops.

---

## Key Files

| File | Lines | Purpose |
|------|-------|---------|
| `internal/neboloop/client.go` | ~373 | REST API client |
| `internal/neboloop/types.go` | ~248 | REST API types |
| `internal/agent/comm/types.go` | ~119 | CommMessage, TaskStatus, AgentCard |
| `internal/agent/comm/handler.go` | ~417 | Message routing, task lifecycle, agentic loop |
| `internal/agent/comm/plugin.go` | ~30 | CommPlugin interface |
| `internal/agent/comm/manager.go` | ~216 | Plugin registry and routing |
| `internal/agent/comm/loopback.go` | ~131 | Testing plugin |
| `internal/agent/comm/neboloop/plugin.go` | ~828 | NeboLoop SDK wrapper |
| `internal/agent/tools/agent_tool.go` | — | Comm tool actions (CommService, LoopQuerier) |
| `cmd/nebo/agent.go` | — | Wiring, callbacks, loopQuerierAdapter |
| `internal/handler/neboloop/handlers.go` | — | Account management HTTP handlers |
| `internal/handler/neboloop/oauth.go` | — | OAuth PKCE flow |
