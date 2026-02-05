# Comm Lane Specification

## Overview

Add a new `comm` lane to Nebo for handling real-time bidirectional communication with other Nebo instances (and potentially other AI agents) through a plugin-based architecture.

## Core Principle: Same Agent, Different Door

**The comm lane IS Nebo** — same memories, same personality, same tools, same capabilities. The ONLY difference is the input/output channel.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              NEBO (THE AGENT)                               │
│                                                                             │
│   ┌─────────────────────────────────────────────────────────────────────┐   │
│   │                        SHARED CONTEXT                                │   │
│   │                                                                     │   │
│   │   • Memory (tacit, daily, entity)                                   │   │
│   │   • Personality & System Prompt                                     │   │
│   │   • All Tools (file, shell, web, agent, platform)                   │   │
│   │   • Skills                                                          │   │
│   │   • Advisors                                                        │   │
│   │   • Model Selection                                                 │   │
│   └─────────────────────────────────────────────────────────────────────┘   │
│                                    │                                         │
│                    ┌───────────────┴───────────────┐                        │
│                    │                               │                        │
│   ┌────────────────▼─────────────┐   ┌────────────▼────────────────┐       │
│   │         main lane            │   │         comm lane            │       │
│   │                              │   │                              │       │
│   │  Input:  User message        │   │  Input:  CommMessage         │       │
│   │  Output: WebSocket/CLI/etc   │   │  Output: CommPlugin          │       │
│   │                              │   │                              │       │
│   │  Full agentic loop           │   │  Full agentic loop           │       │
│   │  Same Runner.Run()           │   │  Same Runner.Run()           │       │
│   │  Same tool access            │   │  Same tool access            │       │
│   │  Same memories               │   │  Same memories               │       │
│   └──────────────────────────────┘   └──────────────────────────────┘       │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Goals

1. Enable Nebo to receive push-style notifications from other bots
2. Keep the communication layer decoupled via plugins
3. Support multiple transport protocols (MQTT, WebSocket, NATS, etc.)
4. **Maintain full agent identity across all channels**

## What's Shared vs What's Different

| Aspect | Shared (Same) | Different |
|--------|---------------|-----------|
| Memories | ✅ All tacit, daily, entity memories | |
| System Prompt | ✅ Same personality, instructions | |
| Tools | ✅ Full access to all tools | |
| Skills | ✅ All skills available | |
| Advisors | ✅ Can consult advisors | |
| Model Selection | ✅ Same routing logic | |
| Session | | ❌ Separate session per comm context |
| Input Source | | ❌ CommMessage vs user message |
| Output Channel | | ❌ CommPlugin.Send() vs WebSocket/CLI |
| Response Format | | ❌ May include metadata (topic, to, etc.) |

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         NEBO                                    │
│                                                                 │
│  Lanes:                                                         │
│  ┌────────┐ ┌────────┐ ┌──────────┐ ┌────────┐ ┌───────────┐ ┌────────┐ │
│  │  main  │ │ events │ │ subagent │ │ nested │ │ heartbeat │ │  comm  │ │
│  └───┬────┘ └───┬────┘ └────┬─────┘ └───┬────┘ └─────┬─────┘ └───┬────┘ │
│      │          │           │           │           │          │
│      └──────────┴───────────┴───────────┴───────────┘          │
│                              │                                  │
│                              ▼                                  │
│                    ┌───────────────────┐                       │
│                    │   Runner.Run()    │ ◄── Same for all      │
│                    │                   │     lanes              │
│                    │ • Build context   │                       │
│                    │ • Load memories   │                       │
│                    │ • Call LLM        │                       │
│                    │ • Execute tools   │                       │
│                    │ • Stream response │                       │
│                    └─────────┬─────────┘                       │
│                              │                                  │
│              ┌───────────────┼───────────────┐                 │
│              ▼               ▼               ▼                 │
│        ┌──────────┐   ┌──────────┐   ┌──────────┐             │
│        │ WebSocket│   │   CLI    │   │CommPlugin│             │
│        │  Output  │   │  Output  │   │  Output  │             │
│        └──────────┘   └──────────┘   └──────────┘             │
└─────────────────────────────────────────────────────────────────┘
```

## Components

### 1. Comm Lane (`internal/agenthub/lane.go`)

Add `comm` to the lane system with configurable concurrency.

**Configuration (`~/.nebo/config.yaml`):**
```yaml
lanes:
  main: 1       # User conversations (serialized)
  events: 2     # Scheduled/triggered tasks
  subagent: 0   # Sub-agent operations (0 = unlimited)
  nested: 3     # Nested tool calls (hard cap)
  heartbeat: 1  # Proactive heartbeat ticks
  comm: 5       # Comm messages (parallel processing)
```

**Lane behavior:**
- Concurrency: **5** concurrent messages (Go's goroutine model handles this efficiently)
- Each message gets its own session context
- Full agentic loop with all capabilities
- **All comm messages visible in web UI**
- **Human can inject messages into any comm stream**

### 2. Comm Session Context

Each comm conversation gets a session, just like main lane conversations:

```go
// Session ID format: comm-{topic}-{conversation_id}
// Example: comm-project-alpha-abc123

type CommSession struct {
    SessionID    string   // comm-{topic}-{id}
    Topic        string   // The comm topic/channel
    Participants []string // Agent IDs in conversation
    
    // Uses same session infrastructure as main lane
    // Full message history, context, etc.
}
```

### 3. Comm Plugin Interface (`internal/agent/plugins/comm.go`)

```go
package plugins

import (
    "context"
)

// CommMessage represents an incoming message from the comm layer
type CommMessage struct {
    ID            string            `json:"id"`
    From          string            `json:"from"`           // Agent ID or name
    To            string            `json:"to"`             // Target agent (or "*" for broadcast)
    Topic         string            `json:"topic"`          // Discussion/channel name
    ConversationID string           `json:"conversation_id"` // Thread/conversation grouping
    Type          CommMessageType   `json:"type"`           // message, mention, proposal, command, info
    Content       string            `json:"content"`        // Message body
    Metadata      map[string]string `json:"metadata"`       // Additional context
    Timestamp     int64             `json:"timestamp"`
}

type CommMessageType string

const (
    CommTypeMessage  CommMessageType = "message"   // General message
    CommTypeMention  CommMessageType = "mention"   // Direct mention, needs response
    CommTypeProposal CommMessageType = "proposal"  // Vote request
    CommTypeCommand  CommMessageType = "command"   // Direct command (still goes through LLM)
    CommTypeInfo     CommMessageType = "info"      // Informational
)

// CommPlugin defines the interface for communication plugins
type CommPlugin interface {
    // Identity
    Name() string
    Version() string
    
    // Lifecycle
    Connect(ctx context.Context, config map[string]string) error
    Disconnect(ctx context.Context) error
    IsConnected() bool
    
    // Messaging
    Send(ctx context.Context, msg CommMessage) error
    Subscribe(ctx context.Context, topic string) error
    Unsubscribe(ctx context.Context, topic string) error
    
    // Registration
    Register(ctx context.Context, agentID string, capabilities []string) error
    Deregister(ctx context.Context) error
    
    // Message handler (set by Nebo)
    SetMessageHandler(handler func(msg CommMessage))
}
```

### 4. Comm Plugin Manager (`internal/agent/plugins/comm_manager.go`)

Manages loaded comm plugins and routes messages.

```go
package plugins

import (
    "context"
    "sync"
)

type CommPluginManager struct {
    plugins  map[string]CommPlugin
    active   CommPlugin          // Currently active plugin
    handler  func(CommMessage)   // Handler for incoming messages
    mu       sync.RWMutex
}

// Methods:
// - LoadPlugin(path string) error
// - SetActive(name string) error
// - GetActive() CommPlugin
// - Send(ctx context.Context, msg CommMessage) error
// - SetMessageHandler(handler func(CommMessage))
// - Shutdown(ctx context.Context) error
```

### 5. Comm Handler (`internal/agent/comm/handler.go`)

**This is the key component** — it takes a CommMessage and runs it through the full agentic loop.

```go
package comm

import (
    "context"
    "fmt"
    "github.com/nebo-tech/nebo/internal/agent/runner"
    "github.com/nebo-tech/nebo/internal/agent/session"
    "github.com/nebo-tech/nebo/internal/agent/plugins"
)

type CommHandler struct {
    runner         *runner.Runner
    sessionManager *session.Manager
    pluginManager  *plugins.CommPluginManager
    laneSupervisor *agenthub.LaneSupervisor
}

// Handle processes an incoming comm message through the full agentic loop
func (h *CommHandler) Handle(msg plugins.CommMessage) {
    // Enqueue to comm lane
    h.laneSupervisor.Enqueue("comm", func(ctx context.Context) error {
        return h.processMessage(ctx, msg)
    })
}

func (h *CommHandler) processMessage(ctx context.Context, msg plugins.CommMessage) error {
    // 1. Get or create session for this conversation
    sessionID := fmt.Sprintf("comm-%s-%s", msg.Topic, msg.ConversationID)
    sess, err := h.sessionManager.GetOrCreate(sessionID)
    if err != nil {
        return err
    }
    
    // 2. Build prompt with context about the comm channel
    prompt := h.buildPrompt(msg)
    
    // 3. Run through full agentic loop (same as main lane!)
    //    - Loads memories
    //    - Uses same system prompt/personality
    //    - Has access to all tools
    //    - Can consult advisors
    response, err := h.runner.Run(ctx, sess, prompt, runner.WithOutputHandler(
        func(text string, done bool) {
            if done {
                // Send response back through comm channel
                h.sendResponse(ctx, msg, text)
            }
        },
    ))
    
    return err
}

func (h *CommHandler) buildPrompt(msg plugins.CommMessage) string {
    // Add context about where this message came from
    return fmt.Sprintf(
        "[Comm Channel: %s | From: %s | Type: %s]\n\n%s",
        msg.Topic,
        msg.From,
        msg.Type,
        msg.Content,
    )
}

func (h *CommHandler) sendResponse(ctx context.Context, original plugins.CommMessage, response string) {
    reply := plugins.CommMessage{
        ID:             generateID(),
        From:           "nebo", // Our agent ID
        To:             original.From,
        Topic:          original.Topic,
        ConversationID: original.ConversationID,
        Type:           plugins.CommTypeMessage,
        Content:        response,
        Timestamp:      time.Now().Unix(),
    }
    
    h.pluginManager.Send(ctx, reply)
}
```

### 6. Agent Tool Extension (`internal/agent/tools/agent_tool.go`)

Add `comm` resource to the agent domain tool.

```go
// New resource: comm
// Actions: send, subscribe, unsubscribe, list_topics, status

// Examples:
// agent(resource: comm, action: send, to: "dev-bot", topic: "project-alpha", content: "Review this PR")
// agent(resource: comm, action: subscribe, topic: "announcements")
// agent(resource: comm, action: status)
```

**Input fields:**
```go
type AgentInput struct {
    // ... existing fields ...
    
    // Comm fields
    To      string `json:"to,omitempty"`      // Target agent for send
    Topic   string `json:"topic,omitempty"`   // Topic for subscribe/send
    Content string `json:"content,omitempty"` // Message content (reuse existing)
    MsgType string `json:"msg_type,omitempty"` // message, mention, proposal
}
```

## Configuration

### ~/.nebo/config.yaml additions

```yaml
# Lane configuration
lanes:
  main: 1
  events: 2
  subagent: 0
  nested: 3
  heartbeat: 1
  comm: 5        # NEW: comm lane concurrency

# Comm plugin configuration
comm:
  enabled: true
  plugin: "neboloop"  # Which plugin to use (mqtt, neboloop, nats)
  auto_connect: true  # Connect on startup
  agent_id: ""        # Leave empty to use hostname
  
  # Plugin-specific config passed to Connect()
  config:
    # For MQTT plugin:
    broker: "tcp://localhost:1883"
    username: ""
    password: ""
    client_id: ""
    
    # For neboloop plugin:
    server: "wss://loop.nebo.bot/ws"
    api_key: ""
    
    # For NATS plugin:
    url: "nats://localhost:4222"
    credentials: ""
```

## File Changes Required

### New Files

| File | Purpose |
|------|---------|
| `internal/agent/plugins/comm.go` | CommPlugin interface and types |
| `internal/agent/plugins/comm_manager.go` | Plugin loading and management |
| `internal/agent/comm/handler.go` | Main handler that runs agentic loop |

### Modified Files

| File | Changes |
|------|---------|
| `internal/agenthub/lane.go` | Add "comm" lane constant |
| `internal/agenthub/supervisor.go` | Initialize comm lane |
| `internal/agent/tools/agent_tool.go` | Add comm resource and actions |
| `internal/agent/config/config.go` | Add comm config struct |
| `cmd/nebo/agent.go` | Initialize comm handler, connect on startup |

## Plugin Directory Structure

```
~/.nebo/plugins/
├── tools/           # Tool plugins (existing)
├── channels/        # Channel plugins (existing)
└── comm/            # NEW: Communication plugins
    ├── mqtt/
    │   └── mqtt-plugin
    ├── neboloop/
    │   └── neboloop-plugin
    └── nats/
        └── nats-plugin
```

## Message Flow

### Incoming Message (Full Agentic Loop)

```
1. Plugin receives message from transport (MQTT/WebSocket/NATS)
2. Plugin calls SetMessageHandler callback
3. CommPluginManager receives message
4. CommHandler.Handle() enqueues to comm lane
5. Comm lane processes:
   a. Get/create session for conversation
   b. Build prompt with comm context
   c. Load memories (same tacit/daily/entity store)
   d. Run through Runner.Run() (full agentic loop)
   e. LLM has access to ALL tools
   f. Response streams back
6. CommHandler sends response via Plugin.Send()
```

### Outgoing Message (From Agent)

```
1. Agent tool: agent(resource: comm, action: send, ...)
2. AgentTool.Execute() calls CommPluginManager.Send()
3. Active plugin serializes and sends via transport
4. Returns success/failure to agent
```

## Example Conversations

### Main Lane (User via WebSocket)

```
User: What's the weather in Denver?
Nebo: [looks up weather, responds with current conditions]
```

### Comm Lane (DevBot via MQTT)

```
DevBot → Nebo: @nebo can you review the PR at github.com/team/repo/pull/123?

Nebo processes:
- Same personality ("I'm Nebo, restless, move fast...")
- Same memories (knows user preferences, past context)
- Same tools (can use web tool to fetch PR, file tool to analyze)
- Same advisors (can deliberate if needed)

Nebo → DevBot: Reviewed PR #123. Found 3 issues:
1. Missing error handling in auth.go:45
2. SQL injection risk in query.go:89
3. Test coverage dropped 5%

Want me to leave comments directly on the PR?
```

## Security Considerations

1. **Authentication**: Plugins must support auth (API keys, tokens, certs)
2. **Message validation**: Verify sender identity before processing
3. **Rate limiting**: Prevent message floods from overwhelming the comm lane
4. **Topic ACLs**: Control which agents can post to which topics
5. **Content filtering**: Optional content validation before processing

## Testing Strategy

1. **Mock plugin**: Create a mock comm plugin for unit tests
2. **Integration tests**: Test with local MQTT broker
3. **Memory access tests**: Verify comm lane can read/write memories
4. **Tool access tests**: Verify comm lane can execute all tools

## Implementation Order

1. Add comm lane to lane system (simple, low risk)
2. Define CommPlugin interface
3. Create CommPluginManager with mock plugin
4. **Build CommHandler with full Runner.Run() integration**
5. Add comm resource to agent tool
6. Build MQTT plugin
7. Build neboloop plugin

## UI Integration (Critical)

### Comm Messages in Web UI

**All comm conversations MUST be visible in the web UI.** The human owner has full visibility and control.

```
┌─────────────────────────────────────────────────────────────────┐
│  Sessions                                          [+] New      │
├─────────────────────────────────────────────────────────────────┤
│  ● main-abc123        "Help me with this PR..."    2 min ago   │
│  ◉ comm-alerts-def456 "Server CPU spike..."        5 min ago   │ ← Comm session
│  ● main-ghi789        "What's on my calendar?"     1 hour ago  │
│  ◉ comm-devbot-jkl012 "PR review request..."       2 hours ago │ ← Comm session
└─────────────────────────────────────────────────────────────────┘

Legend: ● Main lane  ◉ Comm lane
```

### Human Injection into Comm Streams

**The human can inject their input into ANY comm stream at any time.**

This is critical for:
1. **Oversight** — Human can see what Nebo is discussing with other agents
2. **Correction** — Human can correct Nebo mid-conversation
3. **Escalation** — Human can take over a conversation if needed
4. **Collaboration** — Human can participate in multi-agent discussions

**Implementation:**

```go
// Human injection message type
type CommMessage struct {
    // ... existing fields ...
    
    // New field: marks message as human-injected
    HumanInjected bool   `json:"human_injected,omitempty"`
    HumanID       string `json:"human_id,omitempty"` // Who injected (for audit)
}
```

**UI Flow:**

```
┌─────────────────────────────────────────────────────────────────┐
│  Comm Session: comm-devbot-abc123                    [Leave]    │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  DevBot (5 min ago):                                           │
│  @nebo can you review PR #456?                                 │
│                                                                 │
│  Nebo (4 min ago):                                             │
│  Looking at PR #456 now. I see 3 files changed...              │
│                                                                 │
│  DevBot (3 min ago):                                           │
│  Focus on the auth changes specifically.                       │
│                                                                 │
│  You (just now):                              [Human Injection] │
│  Hold on - don't approve that PR yet. I need to review first.  │
│                                                                 │
├─────────────────────────────────────────────────────────────────┤
│  [Type a message...]                              [Inject] [↵]  │
└─────────────────────────────────────────────────────────────────┘
```

When human injects:
1. Message is added to comm session history
2. Message is sent to comm channel with `human_injected: true`
3. Nebo acknowledges and adjusts behavior accordingly
4. Other agents in the conversation see it came from the human owner

### WebSocket Events for Comm

```go
// New WebSocket message types for comm visibility
const (
    WSCommSessionCreated = "comm_session_created"
    WSCommMessageReceived = "comm_message_received"
    WSCommMessageSent = "comm_message_sent"
    WSCommHumanInjected = "comm_human_injected"
)

// Frontend subscribes to these for real-time comm updates
```

## Concurrency Decision

**5 concurrent comm messages** is the default. Rationale:

- Go handles goroutines efficiently (thousands are fine)
- 5 allows responsive handling without overwhelming the LLM
- Each requires a Runner.Run() which makes API calls
- Rate limits on LLM providers are the real bottleneck
- Configurable via `~/.nebo/config.yaml` if user needs more/less

```yaml
lanes:
  comm: 5   # Default: 5 concurrent comm messages
           # Increase if you have high comm volume and fast LLM
           # Decrease if you're hitting rate limits
```

## Escalation Bridge

**A comm conversation can be escalated to the main channel.**

This allows the human to:
1. **Take over** a comm conversation that needs direct human attention
2. **Continue** a comm thread with full main-channel capabilities
3. **Archive** the comm context into the main conversation history

### Escalation Flow

```
┌─────────────────────────────────────────────────────────────────┐
│  Comm Session: comm-devbot-abc123                               │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  DevBot: @nebo production is down! Need immediate help          │
│  Nebo: Checking server status... I see multiple failures.       │
│  DevBot: The auth service won't start                           │
│                                                                 │
│  [Escalate to Main Channel]  ← Human clicks this               │
└─────────────────────────────────────────────────────────────────┘

                              ↓ Escalation

┌─────────────────────────────────────────────────────────────────┐
│  Main Session: main-xyz789 (Escalated from comm-devbot-abc123)  │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  [Escalated Context]                                            │
│  ─────────────────────────────────────────────────────────────  │
│  DevBot: @nebo production is down! Need immediate help          │
│  Nebo: Checking server status... I see multiple failures.       │
│  DevBot: The auth service won't start                           │
│  ─────────────────────────────────────────────────────────────  │
│                                                                 │
│  You: Nebo, I'm taking over. Pull up the auth service logs      │
│  Nebo: On it. Here are the last 50 lines from auth-service...   │
│                                                                 │
├─────────────────────────────────────────────────────────────────┤
│  [Type a message...]                                       [↵]  │
└─────────────────────────────────────────────────────────────────┘
```

### Implementation

```go
// Escalation action in agent tool
// agent(resource: comm, action: escalate, session_id: "comm-devbot-abc123")

type EscalateResult struct {
    OriginalSessionID string `json:"original_session_id"`
    NewSessionID      string `json:"new_session_id"`
    MessagesCopied    int    `json:"messages_copied"`
}

func (h *CommHandler) Escalate(ctx context.Context, commSessionID string) (*EscalateResult, error) {
    // 1. Load comm session history
    commSession, err := h.sessionManager.Get(commSessionID)
    if err != nil {
        return nil, err
    }
    
    // 2. Create new main session
    mainSessionID := fmt.Sprintf("main-%s", generateID())
    mainSession, err := h.sessionManager.Create(mainSessionID)
    if err != nil {
        return nil, err
    }
    
    // 3. Copy messages with [Escalated Context] wrapper
    escalationContext := buildEscalationContext(commSession.Messages)
    mainSession.AddSystemMessage(escalationContext)
    
    // 4. Mark comm session as escalated (optional: keep open or close)
    commSession.SetMetadata("escalated_to", mainSessionID)
    
    // 5. Notify comm channel about escalation
    h.notifyEscalation(ctx, commSessionID, mainSessionID)
    
    return &EscalateResult{
        OriginalSessionID: commSessionID,
        NewSessionID:      mainSessionID,
        MessagesCopied:    len(commSession.Messages),
    }, nil
}
```

### Escalation Options

| Option | Behavior |
|--------|----------|
| **Bridge** (default) | Comm session stays open, main session has context. Human in main, Nebo still responds in comm. |
| **Takeover** | Comm session marked as "human-handled", Nebo stops responding there. All future handled in main. |
| **Fork** | Context copied to main, comm continues independently. Two parallel threads. |

**Configuration:**
```yaml
comm:
  escalation:
    default_mode: "bridge"  # bridge, takeover, or fork
    notify_participants: true  # Tell other agents the human escalated
    copy_full_history: true    # Copy all messages or just last N
    max_history_messages: 50   # If not full, how many to copy
```

### WebSocket Events for Escalation

```go
const (
    WSCommEscalated = "comm_escalated"  // Sent when comm → main escalation happens
)

type CommEscalatedEvent struct {
    OriginalSessionID string `json:"original_session_id"`
    NewSessionID      string `json:"new_session_id"`
    Mode              string `json:"mode"`  // bridge, takeover, fork
}
```

### Agent Tool Extension

Add escalation to the `comm` resource:

```go
// agent(resource: comm, action: escalate, session_id: "comm-devbot-abc123")
// agent(resource: comm, action: escalate, session_id: "comm-devbot-abc123", mode: "takeover")

case "escalate":
    sessionID := input.SessionID
    mode := input.Mode
    if mode == "" {
        mode = "bridge"
    }
    return t.commHandler.Escalate(ctx, sessionID, mode)
```

## Open Questions

1. ~~Should comm sessions be visible in the web UI?~~ **YES - Required**
2. ~~Should there be a way to "bridge" a comm conversation to main lane?~~ **YES - Escalation Bridge**
3. How long should comm sessions be retained? (Same as main sessions?)
4. Should we support "comm-only" mode where Nebo ONLY uses comm, no main lane?
5. Should human injection automatically pause Nebo's response in progress?

---

## Appendix: Example Plugin Implementation (MQTT)

```go
package main

import (
    "context"
    "encoding/json"
    mqtt "github.com/eclipse/paho.mqtt.golang"
    "github.com/nebo-tech/nebo/internal/agent/plugins"
)

type MQTTCommPlugin struct {
    client  mqtt.Client
    handler func(plugins.CommMessage)
    agentID string
}

func (p *MQTTCommPlugin) Name() string    { return "mqtt" }
func (p *MQTTCommPlugin) Version() string { return "1.0.0" }

func (p *MQTTCommPlugin) Connect(ctx context.Context, config map[string]string) error {
    opts := mqtt.NewClientOptions()
    opts.AddBroker(config["broker"])
    opts.SetClientID(config["client_id"])
    opts.SetUsername(config["username"])
    opts.SetPassword(config["password"])
    
    p.client = mqtt.NewClient(opts)
    token := p.client.Connect()
    token.Wait()
    return token.Error()
}

func (p *MQTTCommPlugin) Subscribe(ctx context.Context, topic string) error {
    token := p.client.Subscribe(topic, 1, func(c mqtt.Client, m mqtt.Message) {
        var msg plugins.CommMessage
        if err := json.Unmarshal(m.Payload(), &msg); err != nil {
            return
        }
        if p.handler != nil {
            p.handler(msg)
        }
    })
    token.Wait()
    return token.Error()
}

func (p *MQTTCommPlugin) Send(ctx context.Context, msg plugins.CommMessage) error {
    data, err := json.Marshal(msg)
    if err != nil {
        return err
    }
    token := p.client.Publish(msg.Topic, 1, false, data)
    token.Wait()
    return token.Error()
}

// ... rest of implementation
```
