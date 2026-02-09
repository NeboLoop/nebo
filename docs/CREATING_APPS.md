# Creating Nebo Apps

This guide covers everything you need to build apps for Nebo. Apps communicate over gRPC via Unix sockets, run in a sandboxed environment, and support rich capabilities including tools, channels, gateways, UI panels, and inter-agent communication.

---

## App Directory Structure

Apps live in the `apps/` subdirectory of your Nebo data directory:

```
~/Library/Application Support/Nebo/apps/   # macOS
~/.config/nebo/apps/                        # Linux

apps/
  com.example.myapp/
    manifest.json       # Required: app metadata
    binary              # Required: executable (or named "app")
    signatures.json     # Required for NeboLoop distribution, optional in dev
    data/               # Auto-created: app's sandboxed storage
    logs/               # Auto-created: stdout.log, stderr.log
    ui/                 # Optional: static UI assets
```

---

## manifest.json

Every app requires a `manifest.json`:

```json
{
    "id": "com.example.myapp",
    "name": "My App",
    "version": "1.0.0",
    "description": "What this app does",
    "runtime": "local",
    "protocol": "grpc",
    "provides": ["tool:my_tool"],
    "permissions": ["network:outbound"],
    "settings": [
        {
            "key": "api_key",
            "title": "API Key",
            "type": "password",
            "required": true,
            "description": "Your API key for the service",
            "secret": true
        }
    ]
}
```

**Required fields:**

| Field | Description |
|-------|-------------|
| `id` | Reverse-domain identifier (e.g., `com.mycompany.myapp`) |
| `name` | Human-readable name |
| `version` | Semver string |
| `provides` | At least one capability (see below) |

**Optional fields:**

| Field | Default | Description |
|-------|---------|-------------|
| `runtime` | `"local"` | `"local"` or `"remote"` |
| `protocol` | `"grpc"` | Only `"grpc"` is supported |
| `permissions` | `[]` | What the app needs access to |
| `description` | `""` | Shown in the UI |
| `settings` | `[]` | Configurable settings (rendered in Settings UI) |

---

## Capabilities (provides)

Declare what your app provides:

| Capability | gRPC Service | Description |
|------------|-------------|-------------|
| `gateway` | `GatewayService` | LLM model routing (like Janus) |
| `tool:<name>` | `ToolService` | A named tool for the agent |
| `channel:<name>` | `ChannelService` | A messaging channel |
| `comm` | `CommService` | Inter-agent communication |
| `ui` | `UIService` | Structured UI panels |
| `schedule` | `ScheduleService` | Custom scheduling (replaces built-in cron) |
| `vision` | `ToolService` | Vision processing (uses ToolService) |
| `browser` | `ToolService` | Browser automation (uses ToolService) |

An app can provide multiple capabilities. For example, an app could provide both a tool and a UI panel.

---

## Permissions

Permissions control what the app can access. Deny by default — if not declared, the app can't use it.

| Prefix | Examples | Description |
|--------|----------|-------------|
| `network:` | `network:outbound`, `network:*` | Network access |
| `filesystem:` | `filesystem:read`, `filesystem:write` | File system access |
| `memory:` | `memory:read`, `memory:write` | Agent memory access |
| `session:` | `session:read` | Conversation sessions |
| `tool:` | `tool:shell`, `tool:file` | Use other tools |
| `shell:` | `shell:exec` | Shell command execution |
| `channel:` | `channel:send`, `channel:*` | Channel operations |
| `comm:` | `comm:send`, `comm:*` | Inter-agent comm |
| `model:` | `model:chat` | AI model access |
| `user:` | `user:token` | Receive user JWT in requests |
| `schedule:` | `schedule:create` | Cron job management |
| `database:` | `database:query` | Database access |
| `storage:` | `storage:read`, `storage:write` | Persistent storage |

Wildcard permissions are supported: `network:*` matches any `network:` permission check.

---

## Settings Fields

Settings appear in the Nebo UI under the app's settings panel. Nebo stores them in the database and sends them to your app via the `Configure` RPC when they change.

| Type | Description |
|------|-------------|
| `text` | Single-line text input |
| `password` | Masked text input |
| `toggle` | Boolean on/off switch |
| `select` | Dropdown with predefined options |
| `number` | Numeric input |
| `url` | URL input with validation |

```json
{
    "key": "region",
    "title": "Region",
    "type": "select",
    "required": true,
    "default": "us-east-1",
    "options": [
        {"label": "US East", "value": "us-east-1"},
        {"label": "EU West", "value": "eu-west-1"}
    ]
}
```

---

## Environment Variables

Your app process receives a sanitized environment. All secrets are stripped. You get:

| Variable | Value |
|----------|-------|
| `NEBO_APP_DIR` | App's installation directory |
| `NEBO_APP_SOCK` | Path to the Unix socket to create |
| `NEBO_APP_ID` | App ID from manifest |
| `NEBO_APP_NAME` | App name from manifest |
| `NEBO_APP_VERSION` | App version from manifest |
| `NEBO_APP_DATA` | Path to app's `data/` directory |
| `PATH` | System PATH (passthrough) |
| `HOME` | User home directory (passthrough) |
| `TMPDIR` | Temp directory (passthrough) |

**Critical:** Your binary must create a gRPC server listening on the Unix socket at `NEBO_APP_SOCK`. Nebo waits up to 10 seconds for this socket to appear.

---

## App Launch Sequence

1. Nebo reads `manifest.json` and validates it
2. Finds `binary` or `app` executable in the app directory
3. Checks for `.quarantined` marker (refuses to launch quarantined apps)
4. Revocation check (NeboLoop-distributed apps only)
5. Signature verification (NeboLoop-distributed apps only, skipped in dev)
6. Binary validation (rejects symlinks, non-executables, oversized files)
7. Cleans up stale socket from previous run
8. Creates `data/` directory for sandboxed storage
9. Sets up per-app log files (`logs/stdout.log`, `logs/stderr.log`)
10. Starts binary with sanitized environment and process group isolation
11. Waits for Unix socket to appear (exponential backoff, max 10 seconds)
12. Connects via gRPC over the Unix socket
13. Creates capability-specific gRPC clients based on `provides`
14. Runs health check
15. Registers capabilities with the agent (tools, gateway, comm, etc.)

---

## Tool App

Implement the `ToolService` gRPC service. Declare `"provides": ["tool:my_tool_name"]` in your manifest.

**Proto definition:**

```protobuf
service ToolService {
    rpc HealthCheck(HealthCheckRequest) returns (HealthCheckResponse);
    rpc Name(Empty) returns (NameResponse);
    rpc Description(Empty) returns (DescriptionResponse);
    rpc Schema(Empty) returns (SchemaResponse);
    rpc Execute(ExecuteRequest) returns (ExecuteResponse);
    rpc RequiresApproval(Empty) returns (ApprovalResponse);
    rpc Configure(SettingsMap) returns (Empty);
}
```

**Complete example in Go — a calculator tool app:**

```go
package main

import (
    "context"
    "encoding/json"
    "fmt"
    "net"
    "os"
    "os/signal"
    "syscall"

    pb "github.com/nebolabs/nebo/internal/apps/pb"
    "google.golang.org/grpc"
)

type calculatorServer struct {
    pb.UnimplementedToolServiceServer
}

func (s *calculatorServer) HealthCheck(ctx context.Context, req *pb.HealthCheckRequest) (*pb.HealthCheckResponse, error) {
    return &pb.HealthCheckResponse{Healthy: true, Name: "calculator", Version: "1.0.0"}, nil
}

func (s *calculatorServer) Name(ctx context.Context, req *pb.Empty) (*pb.NameResponse, error) {
    return &pb.NameResponse{Name: "calculator"}, nil
}

func (s *calculatorServer) Description(ctx context.Context, req *pb.Empty) (*pb.DescriptionResponse, error) {
    return &pb.DescriptionResponse{
        Description: "Performs arithmetic calculations. Supports add, subtract, multiply, divide.",
    }, nil
}

func (s *calculatorServer) Schema(ctx context.Context, req *pb.Empty) (*pb.SchemaResponse, error) {
    schema := map[string]interface{}{
        "type": "object",
        "properties": map[string]interface{}{
            "operation": map[string]interface{}{
                "type":        "string",
                "enum":        []string{"add", "subtract", "multiply", "divide"},
                "description": "The arithmetic operation to perform",
            },
            "a": map[string]interface{}{
                "type":        "number",
                "description": "First operand",
            },
            "b": map[string]interface{}{
                "type":        "number",
                "description": "Second operand",
            },
        },
        "required": []string{"operation", "a", "b"},
    }
    data, _ := json.Marshal(schema)
    return &pb.SchemaResponse{Schema: data}, nil
}

type CalcInput struct {
    Operation string  `json:"operation"`
    A         float64 `json:"a"`
    B         float64 `json:"b"`
}

func (s *calculatorServer) Execute(ctx context.Context, req *pb.ExecuteRequest) (*pb.ExecuteResponse, error) {
    var input CalcInput
    if err := json.Unmarshal(req.Input, &input); err != nil {
        return &pb.ExecuteResponse{Content: "Invalid input: " + err.Error(), IsError: true}, nil
    }

    var result float64
    switch input.Operation {
    case "add":
        result = input.A + input.B
    case "subtract":
        result = input.A - input.B
    case "multiply":
        result = input.A * input.B
    case "divide":
        if input.B == 0 {
            return &pb.ExecuteResponse{Content: "Division by zero", IsError: true}, nil
        }
        result = input.A / input.B
    default:
        return &pb.ExecuteResponse{
            Content: fmt.Sprintf("Unknown operation: %s", input.Operation),
            IsError: true,
        }, nil
    }

    return &pb.ExecuteResponse{
        Content: fmt.Sprintf("%g %s %g = %g", input.A, input.Operation, input.B, result),
    }, nil
}

func (s *calculatorServer) RequiresApproval(ctx context.Context, req *pb.Empty) (*pb.ApprovalResponse, error) {
    return &pb.ApprovalResponse{RequiresApproval: false}, nil
}

func (s *calculatorServer) Configure(ctx context.Context, req *pb.SettingsMap) (*pb.Empty, error) {
    fmt.Printf("[calculator] Settings updated: %v\n", req.Values)
    return &pb.Empty{}, nil
}

func main() {
    sockPath := os.Getenv("NEBO_APP_SOCK")
    if sockPath == "" {
        fmt.Fprintln(os.Stderr, "NEBO_APP_SOCK not set")
        os.Exit(1)
    }

    os.Remove(sockPath)

    listener, err := net.Listen("unix", sockPath)
    if err != nil {
        fmt.Fprintf(os.Stderr, "Failed to listen: %v\n", err)
        os.Exit(1)
    }

    server := grpc.NewServer()
    pb.RegisterToolServiceServer(server, &calculatorServer{})

    go func() {
        sigCh := make(chan os.Signal, 1)
        signal.Notify(sigCh, syscall.SIGTERM, syscall.SIGINT)
        <-sigCh
        server.GracefulStop()
    }()

    fmt.Printf("[calculator] Listening on %s\n", sockPath)
    if err := server.Serve(listener); err != nil {
        fmt.Fprintf(os.Stderr, "Server error: %v\n", err)
        os.Exit(1)
    }
}
```

**manifest.json:**

```json
{
    "id": "com.example.calculator",
    "name": "Calculator",
    "version": "1.0.0",
    "description": "Arithmetic calculator tool for the agent",
    "runtime": "local",
    "protocol": "grpc",
    "provides": ["tool:calculator"],
    "permissions": []
}
```

**Build and install:**

```bash
go build -o binary ./cmd/calculator
mkdir -p ~/Library/Application\ Support/Nebo/apps/com.example.calculator
cp binary ~/Library/Application\ Support/Nebo/apps/com.example.calculator/
cp manifest.json ~/Library/Application\ Support/Nebo/apps/com.example.calculator/
```

---

## Channel App

Implement the `ChannelService` gRPC service. Declare `"provides": ["channel:my_channel"]` and `"permissions": ["channel:send"]` (or `"channel:*"`).

Channel apps bridge external messaging platforms (Telegram, Discord, Slack, etc.) to Nebo's agent. When a user sends a message on the external platform, your app streams it to Nebo via `Receive`. When the agent wants to reply, Nebo calls your app's `Send` RPC.

**Proto definition:**

```protobuf
service ChannelService {
    rpc HealthCheck(HealthCheckRequest) returns (HealthCheckResponse);
    rpc ID(Empty) returns (IDResponse);
    rpc Connect(ChannelConnectRequest) returns (ChannelConnectResponse);
    rpc Disconnect(Empty) returns (ChannelDisconnectResponse);
    rpc Send(ChannelSendRequest) returns (ChannelSendResponse);
    rpc Receive(Empty) returns (stream InboundMessage);  // Server streaming
    rpc Configure(SettingsMap) returns (Empty);
}
```

**How it works:**

1. Nebo calls `ID()` to get your channel's unique identifier (e.g., `"telegram"`, `"discord"`)
2. Nebo calls `Connect()` with config from your app's settings (API tokens, bot tokens, etc.)
3. Nebo opens a `Receive()` stream — your app sends `InboundMessage` whenever a user messages the bot
4. Inbound messages are routed to the agent's main conversation lane
5. When the agent (or cron jobs) want to send a message, Nebo calls `Send()` with the channel ID and text
6. On shutdown, Nebo calls `Disconnect()`

**Key points:**

- `ID` must return a stable, unique identifier for this channel type
- `Connect` receives a `map<string, string>` config populated from your app's settings
- `Send` sends a message to a specific chat/channel/room identified by `channel_id`
- `Receive` is a **server-streaming RPC** — keep the stream open and push `InboundMessage` whenever a user messages the bot
- Return empty string in error fields for success
- The agent can send messages to your channel via the `message` tool or cron delivery

**Message formats:**

```protobuf
message ChannelSendRequest {
    string channel_id = 1;  // Chat/channel/room to send to
    string text = 2;        // Message text
}

message InboundMessage {
    string channel_id = 1;  // Chat/channel/room identifier
    string user_id = 2;     // Who sent the message
    string text = 3;        // Message content
    string metadata = 4;    // JSON-encoded extra data (optional)
}
```

**Complete example in Go — a Telegram channel app:**

```go
package main

import (
    "context"
    "fmt"
    "net"
    "os"
    "os/signal"
    "sync"
    "syscall"

    tgbot "github.com/go-telegram/bot"
    pb "github.com/nebolabs/nebo/internal/apps/pb"
    "google.golang.org/grpc"
)

type telegramServer struct {
    pb.UnimplementedChannelServiceServer
    mu       sync.Mutex
    bot      *tgbot.Bot
    messages chan *pb.InboundMessage
    cancel   context.CancelFunc
}

func (s *telegramServer) HealthCheck(ctx context.Context, req *pb.HealthCheckRequest) (*pb.HealthCheckResponse, error) {
    return &pb.HealthCheckResponse{Healthy: true, Name: "telegram", Version: "1.0.0"}, nil
}

func (s *telegramServer) ID(ctx context.Context, req *pb.Empty) (*pb.IDResponse, error) {
    return &pb.IDResponse{Id: "telegram"}, nil
}

func (s *telegramServer) Connect(ctx context.Context, req *pb.ChannelConnectRequest) (*pb.ChannelConnectResponse, error) {
    token := req.Config["bot_token"]
    if token == "" {
        return &pb.ChannelConnectResponse{Error: "bot_token is required"}, nil
    }

    botCtx, cancel := context.WithCancel(context.Background())
    s.cancel = cancel

    bot, err := tgbot.New(token, tgbot.WithDefaultHandler(func(bCtx context.Context, b *tgbot.Bot, update *tgbot.Update) {
        if update.Message == nil {
            return
        }
        s.messages <- &pb.InboundMessage{
            ChannelId: fmt.Sprintf("%d", update.Message.Chat.ID),
            UserId:    fmt.Sprintf("%d", update.Message.From.ID),
            Text:      update.Message.Text,
        }
    }))
    if err != nil {
        return &pb.ChannelConnectResponse{Error: err.Error()}, nil
    }

    s.bot = bot
    go bot.Start(botCtx)
    return &pb.ChannelConnectResponse{}, nil
}

func (s *telegramServer) Disconnect(ctx context.Context, req *pb.Empty) (*pb.ChannelDisconnectResponse, error) {
    if s.cancel != nil {
        s.cancel()
    }
    return &pb.ChannelDisconnectResponse{}, nil
}

func (s *telegramServer) Send(ctx context.Context, req *pb.ChannelSendRequest) (*pb.ChannelSendResponse, error) {
    if s.bot == nil {
        return &pb.ChannelSendResponse{Error: "not connected"}, nil
    }
    _, err := s.bot.SendMessage(ctx, &tgbot.SendMessageParams{
        ChatID: req.ChannelId,
        Text:   req.Text,
    })
    if err != nil {
        return &pb.ChannelSendResponse{Error: err.Error()}, nil
    }
    return &pb.ChannelSendResponse{}, nil
}

func (s *telegramServer) Receive(req *pb.Empty, stream pb.ChannelService_ReceiveServer) error {
    for {
        select {
        case msg := <-s.messages:
            if err := stream.Send(msg); err != nil {
                return err
            }
        case <-stream.Context().Done():
            return nil
        }
    }
}

func (s *telegramServer) Configure(ctx context.Context, req *pb.SettingsMap) (*pb.Empty, error) {
    fmt.Printf("[telegram] Settings updated: %v\n", req.Values)
    return &pb.Empty{}, nil
}

func main() {
    sockPath := os.Getenv("NEBO_APP_SOCK")
    if sockPath == "" {
        fmt.Fprintln(os.Stderr, "NEBO_APP_SOCK not set")
        os.Exit(1)
    }

    os.Remove(sockPath)

    listener, err := net.Listen("unix", sockPath)
    if err != nil {
        fmt.Fprintf(os.Stderr, "Failed to listen: %v\n", err)
        os.Exit(1)
    }

    srv := grpc.NewServer()
    pb.RegisterChannelServiceServer(srv, &telegramServer{
        messages: make(chan *pb.InboundMessage, 100),
    })

    go func() {
        sigCh := make(chan os.Signal, 1)
        signal.Notify(sigCh, syscall.SIGTERM, syscall.SIGINT)
        <-sigCh
        srv.GracefulStop()
    }()

    fmt.Printf("[telegram] Listening on %s\n", sockPath)
    if err := srv.Serve(listener); err != nil {
        fmt.Fprintf(os.Stderr, "Server error: %v\n", err)
        os.Exit(1)
    }
}
```

**manifest.json:**

```json
{
    "id": "com.example.telegram",
    "name": "Telegram",
    "version": "1.0.0",
    "description": "Telegram messaging channel for Nebo",
    "runtime": "local",
    "protocol": "grpc",
    "provides": ["channel:telegram"],
    "permissions": ["channel:send", "network:outbound"],
    "settings": [
        {
            "key": "bot_token",
            "title": "Bot Token",
            "type": "password",
            "required": true,
            "description": "Telegram bot token from @BotFather",
            "secret": true
        }
    ]
}
```

**Message routing:** Inbound messages from `Receive()` are delivered to the agent's main conversation lane. The agent processes them like any other user message. The agent can reply using the `message` tool (`action: send, channel: telegram, to: <chat_id>, text: <response>`), or cron jobs can deliver results to channels automatically.

---

## Comm App

Implement the `CommService` gRPC service. Declare `"provides": ["comm"]` and `"permissions": ["comm:*"]`.

**Proto definition:**

```protobuf
service CommService {
    rpc HealthCheck(HealthCheckRequest) returns (HealthCheckResponse);
    rpc Name(Empty) returns (CommNameResponse);
    rpc Version(Empty) returns (CommVersionResponse);
    rpc Connect(CommConnectRequest) returns (CommConnectResponse);
    rpc Disconnect(Empty) returns (CommDisconnectResponse);
    rpc IsConnected(Empty) returns (CommIsConnectedResponse);
    rpc Send(CommSendRequest) returns (CommSendResponse);
    rpc Subscribe(CommSubscribeRequest) returns (CommSubscribeResponse);
    rpc Unsubscribe(CommUnsubscribeRequest) returns (CommUnsubscribeResponse);
    rpc Register(CommRegisterRequest) returns (CommRegisterResponse);
    rpc Deregister(Empty) returns (CommDeregisterResponse);
    rpc Receive(Empty) returns (stream CommMessage);  // Server streaming
    rpc Configure(SettingsMap) returns (Empty);
}
```

**CommMessage format:**

```protobuf
message CommMessage {
    string id = 1;
    string from = 2;              // Sender agent ID
    string to = 3;                // Recipient agent ID
    string topic = 4;             // Message topic/channel
    string conversation_id = 5;   // Thread identifier
    string type = 6;              // "message", "mention", "proposal", "command", "info", "task"
    string content = 7;           // Message body
    map<string, string> metadata = 8;
    int64 timestamp = 9;          // Unix milliseconds
    bool human_injected = 10;     // True if message was injected by a human
    string human_id = 11;         // Human's identifier
}
```

**Key behaviors:**

- `Register` announces this agent on the network with its capabilities
- `Subscribe/Unsubscribe` manage topic subscriptions
- `Receive` streams inbound messages (server-streaming, same as channels)
- Only one comm app can be active at a time

---

## Gateway App

A gateway app routes LLM requests to models. This is how Janus (Nebo's cloud AI gateway) works. Declare `"provides": ["gateway"]` and `"permissions": ["network:outbound", "user:token"]`.

**Proto definition:**

```protobuf
service GatewayService {
    rpc HealthCheck(HealthCheckRequest) returns (HealthCheckResponse);
    rpc Stream(GatewayRequest) returns (stream GatewayEvent);
    rpc Poll(PollRequest) returns (PollResponse);
    rpc Cancel(CancelRequest) returns (CancelResponse);
    rpc Configure(SettingsMap) returns (Empty);
}
```

**GatewayRequest** contains messages, tools, system prompt, max tokens, temperature, and a `UserContext` (JWT token if `user:token` permission is granted).

**GatewayEvent** types streamed back:

| Type | Content | Description |
|------|---------|-------------|
| `"text"` | Text chunk | Streaming text token |
| `"tool_call"` | JSON: `{"id","name","arguments"}` | Model wants to call a tool |
| `"thinking"` | Text chunk | Extended thinking/reasoning |
| `"error"` | Error message | Something went wrong |
| `"done"` | Empty | Stream is complete |

The `model` field in each event tells Nebo which model actually handled the request (informational).

---

## UI App

A UI app renders structured panels in the Nebo web interface. Declare `"provides": ["ui"]`.

**Proto definition:**

```protobuf
service UIService {
    rpc HealthCheck(HealthCheckRequest) returns (HealthCheckResponse);
    rpc GetView(GetViewRequest) returns (UIView);
    rpc SendEvent(UIEvent) returns (UIEventResponse);
    rpc StreamUpdates(Empty) returns (stream UIView);
    rpc Configure(SettingsMap) returns (Empty);
}
```

**UIView** contains an ordered list of **UIBlocks**. These are pre-built components rendered by Nebo (no custom HTML/CSS/JS):

| Block Type | Fields Used | Description |
|------------|-------------|-------------|
| `text` | `text` | Body text paragraph |
| `heading` | `text`, `variant` (h1/h2/h3) | Section heading |
| `input` | `value`, `placeholder`, `hint`, `disabled` | Text input field |
| `button` | `text`, `variant` (primary/secondary/ghost/error), `disabled` | Clickable button |
| `select` | `value`, `options[]`, `disabled` | Dropdown selector |
| `toggle` | `value` ("true"/"false"), `text`, `disabled` | On/off toggle switch |
| `divider` | — | Horizontal separator |
| `image` | `src`, `alt` | Image display |

**UIEvent** is sent when a user interacts with a block:

```protobuf
message UIEvent {
    string view_id = 1;   // Which view
    string block_id = 2;  // Which block was interacted with
    string action = 3;    // "click", "change", "submit"
    string value = 4;     // New value (for inputs, selects, toggles)
}
```

**UIEventResponse** can return an updated `UIView` to replace the current one, a `toast` message to show the user, or an `error`.

**StreamUpdates** is a server-streaming RPC that pushes new views whenever your app's state changes. Use this for live-updating dashboards.

**Example — a counter UI app:**

```go
func (s *counterServer) GetView(ctx context.Context, req *pb.GetViewRequest) (*pb.UIView, error) {
    return &pb.UIView{
        ViewId: "counter-main",
        Title:  "Counter",
        Blocks: []*pb.UIBlock{
            {BlockId: "count", Type: "heading", Text: fmt.Sprintf("Count: %d", s.count), Variant: "h1"},
            {BlockId: "increment", Type: "button", Text: "Increment", Variant: "primary"},
            {BlockId: "decrement", Type: "button", Text: "Decrement", Variant: "secondary"},
            {BlockId: "reset", Type: "button", Text: "Reset", Variant: "ghost"},
        },
    }, nil
}

func (s *counterServer) SendEvent(ctx context.Context, req *pb.UIEvent) (*pb.UIEventResponse, error) {
    switch req.BlockId {
    case "increment":
        s.count++
    case "decrement":
        s.count--
    case "reset":
        s.count = 0
    }
    view, _ := s.GetView(ctx, &pb.GetViewRequest{})
    return &pb.UIEventResponse{View: view, Toast: fmt.Sprintf("Count is now %d", s.count)}, nil
}
```

---

## Schedule App

A schedule app replaces Nebo's built-in cron scheduler. When installed, all scheduling operations (create, list, delete, trigger, etc.) are routed to your app instead of the default robfig/cron engine. This lets you build alternative schedulers — Google Calendar integration, timezone-aware scheduling, natural language schedules, rate-limited execution, etc.

Declare `"provides": ["schedule"]` and `"permissions": ["schedule:create"]` (or `"schedule:*"`).

**How it works:**

Nebo uses a **SchedulerManager** internally. On startup, the built-in cron engine handles everything. When your schedule app launches, Nebo detects the `schedule` capability and routes all scheduling through your app instead. If your app crashes or is uninstalled, Nebo automatically falls back to the built-in engine.

**Proto definition:**

```protobuf
service ScheduleService {
    rpc HealthCheck(HealthCheckRequest) returns (HealthCheckResponse);
    rpc Create(CreateScheduleRequest) returns (ScheduleResponse);
    rpc Get(GetScheduleRequest) returns (ScheduleResponse);
    rpc List(ListSchedulesRequest) returns (ListSchedulesResponse);
    rpc Update(UpdateScheduleRequest) returns (ScheduleResponse);
    rpc Delete(DeleteScheduleRequest) returns (DeleteScheduleResponse);
    rpc Enable(ScheduleNameRequest) returns (ScheduleResponse);
    rpc Disable(ScheduleNameRequest) returns (ScheduleResponse);
    rpc Trigger(ScheduleNameRequest) returns (TriggerResponse);
    rpc History(ScheduleHistoryRequest) returns (ScheduleHistoryResponse);
    rpc Triggers(Empty) returns (stream ScheduleTrigger);  // App → Nebo trigger stream
    rpc Configure(SettingsMap) returns (Empty);
}
```

**Key messages:**

```protobuf
message Schedule {
    string id = 1;
    string name = 2;
    string expression = 3;       // Cron expression
    string task_type = 4;        // "bash" or "agent"
    string command = 5;          // Shell command (for bash tasks)
    string message = 6;          // Agent prompt (for agent tasks)
    string deliver = 7;          // JSON: {"channel":"telegram","to":"123"}
    bool enabled = 8;
    string last_run = 9;         // RFC3339
    string next_run = 10;        // RFC3339
    int64 run_count = 11;
    string last_error = 12;
    string created_at = 13;      // RFC3339
    map<string, string> metadata = 14;
}

message ScheduleTrigger {
    string schedule_id = 1;
    string name = 2;
    string task_type = 3;
    string command = 4;
    string message = 5;
    string deliver = 6;
    string fired_at = 7;         // RFC3339
    map<string, string> metadata = 8;
}
```

**How triggering works:**

The `Triggers` RPC is a **server-streaming RPC**. Your app keeps this stream open and sends a `ScheduleTrigger` message whenever a schedule fires. Nebo reads from this stream and routes the triggered task to its events lane for execution.

The `ScheduleTrigger` message is **denormalized** — it contains everything Nebo needs (task type, command/message, delivery config) so it doesn't need a follow-up lookup. This is the same pattern used by channel apps' `Receive` stream.

**Key behaviors:**

- Your app **owns the schedule state** — Nebo doesn't store schedules in its own DB when a schedule app is active
- `Create`, `Get`, `List`, `Update`, `Delete`, `Enable`, `Disable` — standard CRUD, your app is the source of truth
- `Trigger` — manually fire a schedule (used by the "Run Now" button in the UI)
- `History` — execution history for the UI's history panel
- `Triggers` — the live stream that tells Nebo when to execute tasks
- `Configure` — called when settings change (same as all other app types)
- Schedules are addressed by **name** (not ID) for CLI ergonomics

**Complete example in Go — a timezone-aware scheduler:**

```go
package main

import (
    "context"
    "fmt"
    "net"
    "os"
    "os/signal"
    "sync"
    "syscall"
    "time"

    pb "github.com/nebolabs/nebo/internal/apps/pb"
    "google.golang.org/grpc"
)

type scheduleServer struct {
    pb.UnimplementedScheduleServiceServer
    mu        sync.Mutex
    schedules map[string]*pb.Schedule
    triggers  chan *pb.ScheduleTrigger
    nextID    int
}

func (s *scheduleServer) HealthCheck(ctx context.Context, req *pb.HealthCheckRequest) (*pb.HealthCheckResponse, error) {
    return &pb.HealthCheckResponse{Healthy: true, Name: "tz-scheduler", Version: "1.0.0"}, nil
}

func (s *scheduleServer) Create(ctx context.Context, req *pb.CreateScheduleRequest) (*pb.ScheduleResponse, error) {
    s.mu.Lock()
    defer s.mu.Unlock()

    if _, exists := s.schedules[req.Name]; exists {
        return &pb.ScheduleResponse{Error: "schedule already exists: " + req.Name}, nil
    }

    s.nextID++
    sched := &pb.Schedule{
        Id:         fmt.Sprintf("%d", s.nextID),
        Name:       req.Name,
        Expression: req.Expression,
        TaskType:   req.TaskType,
        Command:    req.Command,
        Message:    req.Message,
        Deliver:    req.Deliver,
        Enabled:    true,
        CreatedAt:  time.Now().Format(time.RFC3339),
        Metadata:   req.Metadata,
    }
    s.schedules[req.Name] = sched

    // Start your scheduling logic here (e.g., parse expression, set timers)

    return &pb.ScheduleResponse{Schedule: sched}, nil
}

func (s *scheduleServer) Get(ctx context.Context, req *pb.GetScheduleRequest) (*pb.ScheduleResponse, error) {
    s.mu.Lock()
    defer s.mu.Unlock()

    sched, ok := s.schedules[req.Name]
    if !ok {
        return &pb.ScheduleResponse{Error: "not found: " + req.Name}, nil
    }
    return &pb.ScheduleResponse{Schedule: sched}, nil
}

func (s *scheduleServer) List(ctx context.Context, req *pb.ListSchedulesRequest) (*pb.ListSchedulesResponse, error) {
    s.mu.Lock()
    defer s.mu.Unlock()

    var items []*pb.Schedule
    for _, sched := range s.schedules {
        if req.EnabledOnly && !sched.Enabled {
            continue
        }
        items = append(items, sched)
    }
    return &pb.ListSchedulesResponse{Schedules: items, Total: int64(len(items))}, nil
}

func (s *scheduleServer) Delete(ctx context.Context, req *pb.DeleteScheduleRequest) (*pb.DeleteScheduleResponse, error) {
    s.mu.Lock()
    defer s.mu.Unlock()

    if _, ok := s.schedules[req.Name]; !ok {
        return &pb.DeleteScheduleResponse{Error: "not found: " + req.Name}, nil
    }
    delete(s.schedules, req.Name)
    return &pb.DeleteScheduleResponse{Success: true}, nil
}

func (s *scheduleServer) Enable(ctx context.Context, req *pb.ScheduleNameRequest) (*pb.ScheduleResponse, error) {
    s.mu.Lock()
    defer s.mu.Unlock()

    sched, ok := s.schedules[req.Name]
    if !ok {
        return &pb.ScheduleResponse{Error: "not found: " + req.Name}, nil
    }
    sched.Enabled = true
    return &pb.ScheduleResponse{Schedule: sched}, nil
}

func (s *scheduleServer) Disable(ctx context.Context, req *pb.ScheduleNameRequest) (*pb.ScheduleResponse, error) {
    s.mu.Lock()
    defer s.mu.Unlock()

    sched, ok := s.schedules[req.Name]
    if !ok {
        return &pb.ScheduleResponse{Error: "not found: " + req.Name}, nil
    }
    sched.Enabled = false
    return &pb.ScheduleResponse{Schedule: sched}, nil
}

func (s *scheduleServer) Trigger(ctx context.Context, req *pb.ScheduleNameRequest) (*pb.TriggerResponse, error) {
    s.mu.Lock()
    sched, ok := s.schedules[req.Name]
    s.mu.Unlock()

    if !ok {
        return &pb.TriggerResponse{Error: "not found: " + req.Name}, nil
    }

    // Push a trigger event so Nebo executes the task
    s.triggers <- &pb.ScheduleTrigger{
        ScheduleId: sched.Id,
        Name:       sched.Name,
        TaskType:   sched.TaskType,
        Command:    sched.Command,
        Message:    sched.Message,
        Deliver:    sched.Deliver,
        FiredAt:    time.Now().Format(time.RFC3339),
    }

    return &pb.TriggerResponse{Success: true, Output: "triggered"}, nil
}

func (s *scheduleServer) Triggers(req *pb.Empty, stream pb.ScheduleService_TriggersServer) error {
    for {
        select {
        case trigger := <-s.triggers:
            if err := stream.Send(trigger); err != nil {
                return err
            }
        case <-stream.Context().Done():
            return nil
        }
    }
}

func (s *scheduleServer) History(ctx context.Context, req *pb.ScheduleHistoryRequest) (*pb.ScheduleHistoryResponse, error) {
    // Return execution history from your storage
    return &pb.ScheduleHistoryResponse{Entries: nil, Total: 0}, nil
}

func (s *scheduleServer) Update(ctx context.Context, req *pb.UpdateScheduleRequest) (*pb.ScheduleResponse, error) {
    s.mu.Lock()
    defer s.mu.Unlock()

    sched, ok := s.schedules[req.Name]
    if !ok {
        return &pb.ScheduleResponse{Error: "not found: " + req.Name}, nil
    }
    if req.Expression != "" {
        sched.Expression = req.Expression
    }
    if req.TaskType != "" {
        sched.TaskType = req.TaskType
    }
    if req.Command != "" {
        sched.Command = req.Command
    }
    if req.Message != "" {
        sched.Message = req.Message
    }
    return &pb.ScheduleResponse{Schedule: sched}, nil
}

func (s *scheduleServer) Configure(ctx context.Context, req *pb.SettingsMap) (*pb.Empty, error) {
    fmt.Printf("[tz-scheduler] Settings updated: %v\n", req.Values)
    return &pb.Empty{}, nil
}

func main() {
    sockPath := os.Getenv("NEBO_APP_SOCK")
    if sockPath == "" {
        fmt.Fprintln(os.Stderr, "NEBO_APP_SOCK not set")
        os.Exit(1)
    }

    os.Remove(sockPath)

    listener, err := net.Listen("unix", sockPath)
    if err != nil {
        fmt.Fprintf(os.Stderr, "Failed to listen: %v\n", err)
        os.Exit(1)
    }

    srv := grpc.NewServer()
    pb.RegisterScheduleServiceServer(srv, &scheduleServer{
        schedules: make(map[string]*pb.Schedule),
        triggers:  make(chan *pb.ScheduleTrigger, 100),
    })

    go func() {
        sigCh := make(chan os.Signal, 1)
        signal.Notify(sigCh, syscall.SIGTERM, syscall.SIGINT)
        <-sigCh
        srv.GracefulStop()
    }()

    fmt.Printf("[tz-scheduler] Listening on %s\n", sockPath)
    if err := srv.Serve(listener); err != nil {
        fmt.Fprintf(os.Stderr, "Server error: %v\n", err)
        os.Exit(1)
    }
}
```

**manifest.json:**

```json
{
    "id": "com.example.tz-scheduler",
    "name": "Timezone Scheduler",
    "version": "1.0.0",
    "description": "Timezone-aware task scheduler for Nebo",
    "runtime": "local",
    "protocol": "grpc",
    "provides": ["schedule"],
    "permissions": ["schedule:create"],
    "settings": [
        {
            "key": "default_timezone",
            "title": "Default Timezone",
            "type": "select",
            "default": "America/New_York",
            "options": [
                {"label": "Eastern", "value": "America/New_York"},
                {"label": "Central", "value": "America/Chicago"},
                {"label": "Mountain", "value": "America/Denver"},
                {"label": "Pacific", "value": "America/Los_Angeles"},
                {"label": "UTC", "value": "UTC"}
            ]
        }
    ]
}
```

**Important:** Only one schedule app can be active at a time. When your app is installed, it takes over all scheduling. When uninstalled or stopped, Nebo reverts to the built-in cron engine automatically.

---

## Multi-Capability Apps

An app can provide multiple capabilities. For example, a dashboard app that provides both a tool and a UI:

```json
{
    "id": "com.example.dashboard",
    "name": "Dashboard",
    "version": "1.0.0",
    "provides": ["tool:dashboard_query", "ui"],
    "permissions": ["network:outbound"]
}
```

Register both services on the same gRPC server:

```go
server := grpc.NewServer()
pb.RegisterToolServiceServer(server, &dashboardToolServer{})
pb.RegisterUIServiceServer(server, &dashboardUIServer{})
```

---

## Writing Apps in Other Languages

The app system uses gRPC, so you can write apps in any language with gRPC support. Copy the proto files from `proto/apps/v1/` and generate your language's stubs.

### Python Example

```bash
pip install grpcio grpcio-tools
python -m grpc_tools.protoc -I. --python_out=. --grpc_python_out=. proto/apps/v1/*.proto
```

```python
import os
import sys
import json
import grpc
from concurrent import futures
from proto.apps.v1 import tool_pb2, tool_pb2_grpc, common_pb2

class MyToolServicer(tool_pb2_grpc.ToolServiceServicer):
    def HealthCheck(self, request, context):
        return common_pb2.HealthCheckResponse(healthy=True, name="my-tool", version="1.0.0")

    def Name(self, request, context):
        return tool_pb2.NameResponse(name="my_python_tool")

    def Description(self, request, context):
        return tool_pb2.DescriptionResponse(description="A tool written in Python")

    def Schema(self, request, context):
        schema = json.dumps({"type": "object", "properties": {"query": {"type": "string"}}}).encode()
        return tool_pb2.SchemaResponse(schema=schema)

    def Execute(self, request, context):
        input_data = json.loads(request.input)
        result = f"You said: {input_data.get('query', '')}"
        return tool_pb2.ExecuteResponse(content=result, is_error=False)

    def RequiresApproval(self, request, context):
        return tool_pb2.ApprovalResponse(requires_approval=False)

    def Configure(self, request, context):
        print(f"Settings updated: {dict(request.values)}")
        return common_pb2.Empty()

def main():
    sock_path = os.environ.get("NEBO_APP_SOCK")
    if not sock_path:
        print("NEBO_APP_SOCK not set", file=sys.stderr)
        sys.exit(1)

    server = grpc.server(futures.ThreadPoolExecutor(max_workers=4))
    tool_pb2_grpc.add_ToolServiceServicer_to_server(MyToolServicer(), server)
    server.add_insecure_port(f"unix://{sock_path}")
    server.start()
    print(f"[my-tool] Listening on {sock_path}")
    server.wait_for_termination()

if __name__ == "__main__":
    main()
```

For Python apps, the `binary` in the app directory should be a shell script wrapper:

```bash
#!/bin/bash
exec python3 "$NEBO_APP_DIR/main.py"
```

Make it executable: `chmod +x binary`.

---

## Packaging as .napp

For distribution through NeboLoop, package your app as a `.napp` file (a tar.gz archive):

```bash
cd com.example.myapp/
tar -czf myapp-1.0.0.napp manifest.json binary signatures.json
```

**Required files in the archive:**

| File | Max Size | Description |
|------|----------|-------------|
| `manifest.json` | 1 MB | App metadata |
| `binary` or `app` | 500 MB | Executable |
| `signatures.json` | 1 MB | Ed25519 signatures |

**Optional files:**

| File | Max Size | Description |
|------|----------|-------------|
| `ui/*` | 5 MB each | Static UI assets |

**Security rules during extraction:**
- No path traversal (`../` rejected)
- No symlinks (rejected)
- No absolute paths (rejected)
- Only allowlisted filenames accepted

### Signatures (NeboLoop-distributed apps)

NeboLoop signs apps with Ed25519. The `signatures.json` format:

```json
{
    "key_id": "a1b2c3d4",
    "manifest_sig": "<base64 signature of raw manifest.json bytes>",
    "binary_sha256": "<hex SHA256 of binary>",
    "binary_sig": "<base64 signature of raw binary bytes>"
}
```

In dev mode (no NeboLoop URL configured), signature verification is skipped entirely. You don't need `signatures.json` for local development.

---

## Dev Workflow

### Local development (no packaging required)

1. Create your app directory under `apps/`:
   ```bash
   mkdir -p ~/Library/Application\ Support/Nebo/apps/com.example.myapp
   ```

2. Write your `manifest.json` and code

3. Build your binary:
   ```bash
   go build -o ~/Library/Application\ Support/Nebo/apps/com.example.myapp/binary .
   ```

4. Nebo auto-detects the new directory (or restart Nebo)

5. After code changes, rebuild the binary — the file watcher will restart the app

### Viewing logs

App stdout/stderr are captured in per-app log files:

```bash
tail -f ~/Library/Application\ Support/Nebo/apps/com.example.myapp/logs/stdout.log
tail -f ~/Library/Application\ Support/Nebo/apps/com.example.myapp/logs/stderr.log
```

### Persistent data

Use the `data/` directory (also available as `NEBO_APP_DATA` env var) for any files your app needs to persist. This directory survives app updates and quarantine.

### Debugging tips

- If your app doesn't start, check `logs/stderr.log` for errors
- Make sure the binary is executable (`chmod +x binary`)
- Make sure you're listening on the Unix socket path from `NEBO_APP_SOCK`
- The socket must be ready within 10 seconds of launch
- Use `NEBO_APP_DATA` for storage, not hardcoded paths
- Print to stderr for debug logging (captured in `logs/stderr.log`)

### Verifying

```bash
# List installed apps
nebo apps list

# Check running apps (via Settings > Status page in the web UI)
# Or check the apps directory for running sockets
ls ~/Library/Application\ Support/Nebo/apps/*/app.sock
```

### Uninstalling

```bash
nebo apps uninstall com.example.myapp
```

---

## Proto File Reference

All proto files live in `proto/apps/v1/`. Copy these into your project if writing in a non-Go language.

| File | Service | Key RPCs |
|------|---------|----------|
| `common.proto` | (messages only) | HealthCheckRequest/Response, SettingsMap, UserContext, Empty |
| `tool.proto` | `ToolService` | Name, Description, Schema, Execute, RequiresApproval, Configure |
| `channel.proto` | `ChannelService` | ID, Connect, Disconnect, Send, Receive (stream), Configure |
| `comm.proto` | `CommService` | Name, Version, Connect, Disconnect, Send, Subscribe, Register, Receive (stream), Configure |
| `gateway.proto` | `GatewayService` | HealthCheck, Stream (stream), Poll, Cancel, Configure |
| `ui.proto` | `UIService` | HealthCheck, GetView, SendEvent, StreamUpdates (stream), Configure |
| `schedule.proto` | `ScheduleService` | HealthCheck, Create, Get, List, Update, Delete, Enable, Disable, Trigger, History, Triggers (stream), Configure |

Every service includes `HealthCheck` and `Configure` RPCs. Always implement both.

---

## Publishing to NeboLoop

Once your app works locally, you can publish it through NeboLoop for distribution to all Nebo users.

### Developer Account Setup

1. Register a NeboLoop account at `POST /api/v1/auth/register` (email + password)
2. Log in to get a JWT token: `POST /api/v1/auth/login`
3. Create a developer account: `POST /api/v1/developer/accounts`

All subsequent API calls use the JWT as a Bearer token.

### Submission Flow

```
draft → pending_review → [approved] → published
                       → [rejected] → draft (fix and resubmit)
```

**Step 1: Create the app**

```bash
curl -X POST https://neboloop.com/api/v1/developer/apps \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "My App",
    "version": "1.0.0",
    "description": "What it does",
    "category": "productivity",
    "visibility": "public"
  }'
```

Returns an app ID. The app starts in `draft` status.

**Step 2: Upload platform binaries**

```bash
curl -X POST https://neboloop.com/api/v1/developer/apps/{id}/binaries \
  -H "Authorization: Bearer $TOKEN" \
  -F "binary=@myapp-darwin-arm64" \
  -F "platform=darwin-arm64"
```

Upload one binary per platform you support (e.g., `darwin-arm64`, `darwin-amd64`, `linux-amd64`, `linux-arm64`).

**Step 3: Submit for review**

```bash
curl -X POST https://neboloop.com/api/v1/developer/apps/{id}/submit \
  -H "Authorization: Bearer $TOKEN"
```

This moves the app to `pending_review`.

### Review Process

1. **Automated scan** — binaries are scanned via VirusTotal (near-instant)
2. **Admin review** — NeboLoop team reviews metadata, manifest, and scan results
3. **On approval** — NeboLoop signs the app with Ed25519 and publishes it
4. **On rejection** — you receive a reason and can fix and resubmit

Target review time: <24 hours.

### Signing

You submit unsigned binaries. NeboLoop signs on approval — NeboLoop is the sole signing authority. Nebo instances verify the signature on install using NeboLoop's public key.

### Pushing Updates

To update a published app:

1. Update the app's version: `PUT /api/v1/developer/apps/{id}` with the new version
2. Upload new binaries for each platform
3. Submit for review again

Each update goes through the same review process. Previous versions remain available until the new version is approved. Full binary upload required (no delta mechanism).

### Store Metadata

Beyond what's in your `manifest.json`, the store accepts:

| Field | Description |
|-------|-------------|
| `category` | e.g., productivity, communication, developer-tools |
| `icon` | App icon URL |
| `visibility` | `public` or `private` |
| `description` | Can be longer/richer than the manifest description |

Screenshots, changelogs, and pricing are planned but not yet available.

---

## Minimal App Checklist

1. Create `manifest.json` with `id`, `name`, `version`, `provides`
2. Write binary that:
   - Reads `NEBO_APP_SOCK` from environment
   - Creates gRPC server on that Unix socket
   - Implements the required service(s) for your capability
   - Implements `HealthCheck` (return `healthy: true`)
   - Handles `SIGTERM` for graceful shutdown
3. Make binary executable (`chmod +x`)
4. Place in `apps/<your-app-id>/` directory
5. Nebo auto-discovers and launches it
