# Creating Nebo Apps

## What Nebo Is

Nebo is a personal operating system — an always-running AI agent that manages your digital life. Think of Nebo the way you think of an iPhone: a powerful platform that becomes transformative through its apps.

On its own, Nebo is a capable agent with memory, scheduling, web browsing, file management, and shell access. But the real unlock is Apps. Just as the iPhone's power comes from the App Store ecosystem, Nebo's power comes from the apps that extend what it can do.

## What Apps Are

Nebo Apps are self-contained, precompiled units of incredible functionality. Each app gives Nebo a new superpower — calendar management, email triage, project tracking, home automation, financial analysis — anything a developer can imagine.

**Apps provide incredible power in a very safe manner.** That's the goal. Every app runs in a sandbox with deny-by-default permissions. It can only access what its manifest declares and the user approves. An app that manages your calendar can't read your files. An app that browses the web can't execute shell commands. The permission boundary is absolute.

**The interface is 100% conversational.** Users don't interact with apps through separate screens or dashboards. They talk to Nebo, and Nebo uses the app's tools to get things done. "What's on my calendar tomorrow?" — Nebo calls the calendar app. "Create a meeting with Sarah at 3pm" — Nebo calls the calendar app. The user never leaves the conversation.

Apps can optionally provide a **UI panel** — a supplementary, glanceable dashboard that shows status at a glance (today's schedule, connection status, a quick summary). But the UI is never the primary interface. It's a complement to the conversation, not a replacement for it.

**Apps replace and extend Nebo's built-in capabilities.** Nebo ships with basic platform tools (local calendar access, screenshots, etc.). When you install a calendar app, it replaces the built-in calendar tool and becomes a superset — local + cloud + aggregation + availability checking. The agent seamlessly uses whichever tool is registered, whether built-in or app-provided.

## Compiled-Only Policy

Nebo enforces a strict compiled-only binary policy. All apps must be native compiled executables.

| Status | Languages |
|--------|-----------|
| **Supported** | Go (recommended), Rust, C/C++, Zig |
| **Rejected** | Python, Node.js, Ruby, Java, .NET, shell scripts |

**Rationale:**

- **AI self-modification prevention** — An agent with shell access could modify an interpreted script's behavior at runtime. Compiled binaries are immutable after signing.
- **Signature integrity** — Ed25519 signatures cover raw binary bytes. A signed binary cannot be modified without invalidating the signature. Scripts can be modified between signature verification and execution.
- **Sandbox enforcement** — Binary format validation (ELF/Mach-O/PE magic bytes) is a fast, reliable gate. Detecting all possible shebang interpreters is a whack-a-mole game.

**What happens:** Nebo validates binary format at launch time. Binaries with a shebang (`#!`) are rejected immediately with a clear error:

```
binary is a script (shebang #! detected) — only compiled native binaries are allowed
```

## How Apps Work

Apps communicate over gRPC via Unix sockets, run in a sandboxed environment, and support rich capabilities including tools, channels, gateways, UI panels, and inter-agent communication.

---

## Quick Start with the SDK

Official SDKs handle all gRPC server setup, signal handling, and protocol bridging. You just implement handler interfaces. Full API documentation at [developer.neboloop.com](https://developer.neboloop.com).

**Go** (recommended):

```bash
go get github.com/nebolabs/nebo-sdk-go
```

**Rust:**

```toml
[dependencies]
nebo-sdk = "0.1"
```

**C/C++:**

```bash
# Add as a CMake subdirectory or copy headers from sdk/c/include/
```

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

**Store integration:** Your settings schema is included in the NeboLoop app store listing and in the install notification payload. This means:
- The store shows "Requires configuration" pre-install if you have required settings
- After install, Nebo immediately prompts the user with your settings form
- Apps with unconfigured required settings show a "Needs Setup" badge in the UI

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
6. Binary validation (rejects symlinks, scripts, non-executables, oversized files)
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

Declare `"provides": ["tool:my_tool_name"]` in your manifest.

### The STRAP Pattern

All Nebo tool apps **must** use the **STRAP (Single Tool Resource Action Pattern)** for their schema and execution routing. This is the same pattern Nebo uses internally to consolidate 35+ individual tools into 4 domain tools — reducing LLM context overhead by ~80%.

**Read the full article:** [Reduced MCP Tools 96→10: The STRAP Pattern](https://almatuck.com/articles/reduced-mcp-tools-96-to-10-strap-pattern)

**Core idea:** Instead of registering multiple tools (`get_events`, `check_availability`, `suggest_slots`), register ONE tool with `action` (and optionally `resource`) fields that route to the right handler.

**Structure:** `tool_name(action: "do_something", ...params)`

For tools with multiple resource types, add a `resource` field:

**Structure:** `tool_name(resource: "thing", action: "do_something", ...params)`

**Schema rules:**

1. Always include an `action` field as a required string enum
2. Add a `resource` field (required string enum) only if your tool manages multiple distinct resource types
3. All other fields are action-specific parameters
4. The `action` enum description should list all available actions

**Single-resource example (calendar):**

```
calendar(action: "get_events", start: "2025-01-15T09:00:00Z", end: "2025-01-16T00:00:00Z")
calendar(action: "next_event")
calendar(action: "check_availability", start: "...", end: "...")
calendar(action: "suggest_slots", duration_minutes: 30, preferred_time: "morning")
```

**Multi-resource example (project manager):**

```
project(resource: "task", action: "create", title: "Ship v2", assignee: "alice")
project(resource: "task", action: "list", status: "open")
project(resource: "milestone", action: "create", name: "Beta", deadline: "2025-03-01")
project(resource: "milestone", action: "list")
```

**Why STRAP matters:**

- LLMs learn the `action` routing pattern once and generalize across all operations
- Tool definitions consume ~6% of context — STRAP cuts that by 80%
- New operations are just enum additions, not new tool registrations
- Works identically across Claude, GPT, Gemini, and local models

### Go Example — Calculator Tool

```go
package main

import (
    "context"
    "encoding/json"
    "fmt"
    "log"

    nebo "github.com/nebolabs/nebo-sdk-go"
)

type Calculator struct{}

func (c *Calculator) Name() string        { return "calculator" }
func (c *Calculator) Description() string { return "Performs arithmetic calculations." }

func (c *Calculator) Schema() json.RawMessage {
    return nebo.NewSchema("add", "subtract", "multiply", "divide").
        Number("a", "First operand", true).
        Number("b", "Second operand", true).
        Build()
}

func (c *Calculator) Execute(_ context.Context, input json.RawMessage) (string, error) {
    var in struct {
        Action string  `json:"action"`
        A      float64 `json:"a"`
        B      float64 `json:"b"`
    }
    if err := json.Unmarshal(input, &in); err != nil {
        return "", fmt.Errorf("invalid input: %w", err)
    }

    var result float64
    switch in.Action {
    case "add":
        result = in.A + in.B
    case "subtract":
        result = in.A - in.B
    case "multiply":
        result = in.A * in.B
    case "divide":
        if in.B == 0 {
            return "", fmt.Errorf("division by zero")
        }
        result = in.A / in.B
    default:
        return "", fmt.Errorf("unknown action: %s", in.Action)
    }

    return fmt.Sprintf("%g %s %g = %g", in.A, in.Action, in.B, result), nil
}

func main() {
    app, err := nebo.New()
    if err != nil {
        log.Fatal(err)
    }
    app.RegisterTool(&Calculator{})
    log.Fatal(app.Run())
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
go build -o binary .
mkdir -p ~/Library/Application\ Support/Nebo/apps/com.example.calculator
cp binary manifest.json ~/Library/Application\ Support/Nebo/apps/com.example.calculator/
```

---

## Channel App

Declare `"provides": ["channel:my_channel"]` and `"permissions": ["channel:send"]` (or `"channel:*"`).

Channel apps bridge external messaging platforms (Telegram, Discord, Slack, etc.) to Nebo's agent. When a user sends a message on the external platform, your app streams it to Nebo via `Receive`. When the agent wants to reply, Nebo calls your app's `Send`.

**How it works:**

1. Nebo calls `ID()` to get your channel's unique identifier (e.g., `"telegram"`, `"discord"`)
2. Nebo calls `Connect()` with config from your app's settings (API tokens, bot tokens, etc.)
3. Nebo opens a `Receive()` stream — your app sends inbound messages whenever a user messages the bot
4. Inbound messages are routed to the agent's main conversation lane
5. When the agent (or cron jobs) want to send a message, Nebo calls `Send()` with the v1 message envelope
6. On shutdown, Nebo calls `Disconnect()`

### v1 Message Envelope

All channel messages — inbound and outbound — use the common message envelope. This is the shared contract between Nebo, NeboLoop, and every channel plugin.

```json
{
  "message_id": "01953f8a-...",
  "channel_id": "telegram:12345",
  "sender": { "name": "Alex", "role": "COO", "bot_id": "uuid" },
  "text": "Q3 numbers look good...",
  "attachments": [{ "type": "image", "url": "https://...", "filename": "chart.png", "size": 45000 }],
  "reply_to": "01953f89-...",
  "actions": [{ "label": "Approve", "callback_id": "approve_q3" }],
  "platform_data": null,
  "timestamp": "2026-02-12T15:10:00Z"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `message_id` | UUID v7 | Time-ordered unique ID. Required for `reply_to` references. |
| `channel_id` | `{type}:{platform_id}` | Route-able channel identifier (e.g., `telegram:12345`, `discord:98765`) |
| `sender` | object | Bot identity — `name`, `role`, `bot_id`. Enriched by the NeboLoop broker from cached identity; your app doesn't need to set this on outbound messages. |
| `text` | string | Message body |
| `attachments` | array | Files, images, audio. Each has `type`, `url`, `filename`, `size`. |
| `reply_to` | UUID v7 | Parent message ID for threading. `null` for top-level messages. |
| `actions` | array | Interactive buttons/keyboards. Each has `label` and `callback_id`. |
| `platform_data` | bytes | Opaque passthrough for platform-specific features — Telegram inline keyboards, Discord embeds, iMessage balloon effects. Your plugin interprets this; the agent and broker pass it through untouched. |
| `timestamp` | ISO 8601 | Publisher sets this. For inbound messages, use the platform's original timestamp, not bridge time. |

**Two-layer design:** The common fields (text, attachments, reply_to, actions) cover 90% of agent-initiated messaging. Platform-specific features go in `platform_data` — your plugin maps them to/from the native platform format. This means you can build a basic channel plugin using only the common fields, and add rich platform features incrementally.

**Sender identity:** The `sender` block contains the bot's name and role (relationship dynamic — e.g., "Friend", "COO", "Mentor"). The NeboLoop broker enriches outbound messages with sender identity from its cached bot record. Your channel plugin uses this to format the display name however the platform expects ("Alex/COO" in Slack, "Alex — COO" in Discord embeds, etc.).

### MQTT Topics

Channel messages are routed over per-channel MQTT subtopics for isolation:

```
neboloop/bot/{botID}/channels/{channelType}/inbound    # messages from users → agent
neboloop/bot/{botID}/channels/{channelType}/outbound   # messages from agent → channel
```

Each channel type gets its own queue, so a slow consumer on one channel (e.g., rate-limited WhatsApp API) can't back up another. Nebo subscribes with wildcards — backward-compatible with legacy topics.

### Go Example — Telegram Channel

```go
package main

import (
    "context"
    "encoding/json"
    "fmt"
    "log"
    "time"

    "github.com/google/uuid"
    tgbot "github.com/go-telegram/bot"
    nebo "github.com/nebolabs/nebo-sdk-go"
)

type Telegram struct {
    bot      *tgbot.Bot
    messages chan nebo.ChannelEnvelope
    cancel   context.CancelFunc
}

func (t *Telegram) ID() string { return "telegram" }

func (t *Telegram) Connect(_ context.Context, config map[string]string) error {
    token := config["bot_token"]
    if token == "" {
        return fmt.Errorf("bot_token is required")
    }

    botCtx, cancel := context.WithCancel(context.Background())
    t.cancel = cancel

    bot, err := tgbot.New(token, tgbot.WithDefaultHandler(func(bCtx context.Context, b *tgbot.Bot, update *tgbot.Update) {
        if update.Message == nil {
            return
        }

        env := nebo.ChannelEnvelope{
            MessageID: uuid.Must(uuid.NewV7()).String(),
            ChannelID: fmt.Sprintf("telegram:%d", update.Message.Chat.ID),
            Text:      update.Message.Text,
            Timestamp: time.Unix(int64(update.Message.Date), 0).UTC(),
        }

        // Handle reply threading
        if update.Message.ReplyToMessage != nil {
            env.ReplyTo = fmt.Sprintf("%d", update.Message.ReplyToMessage.MessageID)
        }

        // Handle photo attachments
        if len(update.Message.Photo) > 0 {
            best := update.Message.Photo[len(update.Message.Photo)-1]
            env.Attachments = []nebo.Attachment{{
                Type: "image",
                URL:  fmt.Sprintf("tg://file/%s", best.FileID),
                Size: int64(best.FileSize),
            }}
        }

        t.messages <- env
    }))
    if err != nil {
        return err
    }

    t.bot = bot
    go bot.Start(botCtx)
    return nil
}

func (t *Telegram) Disconnect(_ context.Context) error {
    if t.cancel != nil {
        t.cancel()
    }
    return nil
}

func (t *Telegram) Send(ctx context.Context, env nebo.ChannelEnvelope) (string, error) {
    if t.bot == nil {
        return "", fmt.Errorf("not connected")
    }

    // Extract platform chat ID from channel_id ("telegram:12345" → "12345")
    chatID := env.ChannelID[len("telegram:"):]

    params := &tgbot.SendMessageParams{
        ChatID: chatID,
        Text:   env.Text,
    }

    // Handle reply threading
    if env.ReplyTo != "" {
        params.ReplyParameters = &tgbot.ReplyParameters{MessageID: env.ReplyTo}
    }

    // Handle inline keyboard actions
    if len(env.Actions) > 0 {
        var buttons [][]tgbot.InlineKeyboardButton
        for _, a := range env.Actions {
            buttons = append(buttons, []tgbot.InlineKeyboardButton{
                {Text: a.Label, CallbackData: a.CallbackID},
            })
        }
        params.ReplyMarkup = &tgbot.InlineKeyboardMarkup{InlineKeyboard: buttons}
    }

    // Handle platform-specific data (e.g., custom Telegram inline keyboards)
    if env.PlatformData != nil {
        var pd map[string]json.RawMessage
        if err := json.Unmarshal(env.PlatformData, &pd); err == nil {
            if kb, ok := pd["inline_keyboard"]; ok {
                var markup tgbot.InlineKeyboardMarkup
                if err := json.Unmarshal(kb, &markup); err == nil {
                    params.ReplyMarkup = &markup
                }
            }
        }
    }

    msg, err := t.bot.SendMessage(ctx, params)
    if err != nil {
        return "", err
    }
    return fmt.Sprintf("%d", msg.MessageID), nil
}

func (t *Telegram) Receive(_ context.Context) (<-chan nebo.ChannelEnvelope, error) {
    return t.messages, nil
}

func main() {
    app, err := nebo.New()
    if err != nil {
        log.Fatal(err)
    }
    app.RegisterChannel(&Telegram{
        messages: make(chan nebo.ChannelEnvelope, 100),
    })
    log.Fatal(app.Run())
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

**Message routing:** Inbound envelopes from `Receive()` are delivered to the agent's conversation lane. The agent processes them like any other user message. The agent can reply using the `message` tool (`action: send, channel: telegram, to: <chat_id>, text: <response>`), or cron jobs can deliver results to channels automatically. The NeboLoop broker enriches outbound envelopes with the bot's `sender` identity (name + role) before routing to your plugin.

**Send return value:** `Send()` returns a `message_id` (the platform's native message ID) so Nebo can track it for threading via `reply_to`. If your platform doesn't have message IDs, generate a UUID v7.

---

## Comm App

Declare `"provides": ["comm"]` and `"permissions": ["comm:*"]`.

**CommMessage format:**

| Field | Description |
|-------|-------------|
| `from` | Sender agent ID |
| `to` | Recipient agent ID |
| `topic` | Message topic/channel |
| `type` | `"message"`, `"mention"`, `"proposal"`, `"command"`, `"info"`, `"task"` |
| `content` | Message body |

**Key behaviors:**

- `Register` announces this agent on the network with its capabilities
- `Subscribe/Unsubscribe` manage topic subscriptions
- `Receive` streams inbound messages (server-streaming, same as channels)
- Only one comm app can be active at a time

---

## Gateway App

A gateway app routes LLM requests to models. This is how Janus (Nebo's cloud AI gateway) works. Declare `"provides": ["gateway"]` and `"permissions": ["network:outbound", "user:token"]`.

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

**StreamUpdates** is a server-streaming RPC that pushes new views whenever your app's state changes. Use this for live-updating dashboards.

### Go Example — Counter UI

```go
package main

import (
    "context"
    "fmt"
    "log"
    "sync/atomic"

    nebo "github.com/nebolabs/nebo-sdk-go"
)

type Counter struct {
    count atomic.Int64
}

func (c *Counter) GetView(_ context.Context, _ string) (*nebo.View, error) {
    return nebo.NewView("counter-main", "Counter").
        Heading("count", fmt.Sprintf("Count: %d", c.count.Load()), "h1").
        Button("increment", "Increment", "primary").
        Button("decrement", "Decrement", "secondary").
        Button("reset", "Reset", "ghost").
        Build(), nil
}

func (c *Counter) OnEvent(_ context.Context, event nebo.UIEvent) (*nebo.UIEventResult, error) {
    switch event.BlockID {
    case "increment":
        c.count.Add(1)
    case "decrement":
        c.count.Add(-1)
    case "reset":
        c.count.Store(0)
    }
    view, _ := c.GetView(context.Background(), "")
    return &nebo.UIEventResult{
        View:  view,
        Toast: fmt.Sprintf("Count is now %d", c.count.Load()),
    }, nil
}

func main() {
    app, err := nebo.New()
    if err != nil {
        log.Fatal(err)
    }
    app.RegisterUI(&Counter{})
    log.Fatal(app.Run())
}
```

---

## Schedule App

A schedule app replaces Nebo's built-in cron scheduler. When installed, all scheduling operations (create, list, delete, trigger, etc.) are routed to your app instead of the default robfig/cron engine. This lets you build alternative schedulers — Google Calendar integration, timezone-aware scheduling, natural language schedules, rate-limited execution, etc.

Declare `"provides": ["schedule"]` and `"permissions": ["schedule:create"]` (or `"schedule:*"`).

**How it works:**

Nebo uses a **SchedulerManager** internally. On startup, the built-in cron engine handles everything. When your schedule app launches, Nebo detects the `schedule` capability and routes all scheduling through your app instead. If your app crashes or is uninstalled, Nebo automatically falls back to the built-in engine.

**Key behaviors:**

- Your app **owns the schedule state** — Nebo doesn't store schedules in its own DB when a schedule app is active
- `Create`, `Get`, `List`, `Update`, `Delete`, `Enable`, `Disable` — standard CRUD, your app is the source of truth
- `Trigger` — manually fire a schedule (used by the "Run Now" button in the UI)
- `History` — execution history for the UI's history panel
- `Triggers` — the live stream that tells Nebo when to execute tasks
- Schedules are addressed by **name** (not ID) for CLI ergonomics

**How triggering works:**

The `Triggers` RPC is a **server-streaming RPC**. Your app keeps this stream open and sends a `ScheduleTrigger` message whenever a schedule fires. Nebo reads from this stream and routes the triggered task to its events lane for execution.

The `ScheduleTrigger` message is **denormalized** — it contains everything Nebo needs (task type, command/message, delivery config) so it doesn't need a follow-up lookup. This is the same pattern used by channel apps' `Receive` stream.

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

Register both handlers with the SDK:

```go
app, _ := nebo.New()
app.RegisterTool(&dashboardTool{})
app.RegisterUI(&dashboardUI{})
log.Fatal(app.Run())
```

---

## Rust

The Rust SDK uses async traits and tonic for gRPC:

```rust
use async_trait::async_trait;
use nebo_sdk::{NeboApp, NeboError, SchemaBuilder};
use nebo_sdk::tool::ToolHandler;
use serde::Deserialize;
use serde_json::Value;

struct Calculator;

#[derive(Deserialize)]
struct Input { action: String, a: f64, b: f64 }

#[async_trait]
impl ToolHandler for Calculator {
    fn name(&self) -> &str { "calculator" }
    fn description(&self) -> &str { "Performs arithmetic calculations." }
    fn schema(&self) -> Value {
        SchemaBuilder::new(&["add", "subtract", "multiply", "divide"])
            .number("a", "First operand", true)
            .number("b", "Second operand", true)
            .build()
    }
    async fn execute(&self, input: Value) -> Result<String, NeboError> {
        let i: Input = serde_json::from_value(input)
            .map_err(|e| NeboError::Execution(e.to_string()))?;
        let r = match i.action.as_str() {
            "add" => i.a + i.b,
            "subtract" => i.a - i.b,
            "multiply" => i.a * i.b,
            "divide" => {
                if i.b == 0.0 {
                    return Err(NeboError::Execution("division by zero".into()));
                }
                i.a / i.b
            }
            other => return Err(NeboError::Execution(format!("unknown action: {other}"))),
        };
        Ok(format!("{} {} {} = {}", i.a, i.action, i.b, r))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    NeboApp::new()?.register_tool(Calculator).run().await?;
    Ok(())
}
```

Install: `cargo add nebo-sdk`

---

## C/C++

The C SDK provides a pure C API with function pointer-based handlers:

```c
#include <nebo/nebo.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int calculator_execute(const char *input_json, char **output, int *is_error) {
    /* Parse JSON, compute result, set *output */
    *output = strdup("2 add 3 = 5");
    *is_error = 0;
    return 0;
}

int main(void) {
    const char *actions[] = {"add", "subtract", "multiply", "divide", NULL};
    nebo_schema_builder_t *sb = nebo_schema_new(actions);
    nebo_schema_number(sb, "a", "First operand", 1);
    nebo_schema_number(sb, "b", "Second operand", 1);
    char *schema = nebo_schema_build(sb);
    nebo_schema_free(sb);

    nebo_tool_handler_t calculator = {
        .name = "calculator",
        .description = "Performs arithmetic calculations.",
        .schema = schema,
        .execute = calculator_execute,
    };

    nebo_app_t *app = nebo_app_new();
    nebo_app_register_tool(app, &calculator);
    int ret = nebo_app_run(app);
    free(schema);
    return ret;
}
```

Build with CMake:

```bash
mkdir build && cd build && cmake .. && make
```

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

Proto files live in `proto/apps/`. Channel proto is v1; others are v0. The SDKs ship with pre-generated code, so you don't need protoc for normal development.

| File | Service | Key RPCs |
|------|---------|----------|
| `v0/common.proto` | (messages only) | HealthCheckRequest/Response, SettingsMap, UserContext, Empty |
| `v0/tool.proto` | `ToolService` | Name, Description, Schema, Execute, RequiresApproval, Configure |
| `v1/channel.proto` | `ChannelService` | ID, Connect, Disconnect, Send, Receive (stream), Configure |
| `v0/comm.proto` | `CommService` | Name, Version, Connect, Disconnect, Send, Subscribe, Register, Receive (stream), Configure |
| `v0/gateway.proto` | `GatewayService` | HealthCheck, Stream (stream), Poll, Cancel, Configure |
| `v0/ui.proto` | `UIService` | HealthCheck, GetView, SendEvent, StreamUpdates (stream), Configure |
| `v0/schedule.proto` | `ScheduleService` | HealthCheck, Create, Get, List, Update, Delete, Enable, Disable, Trigger, History, Triggers (stream), Configure |

Every service includes `HealthCheck` and `Configure` RPCs. The SDK handles both automatically.

### v1 Channel Proto Types

The v1 channel proto adds rich messaging support:

| Message Type | Fields | Description |
|--------------|--------|-------------|
| `ChannelSendRequest` | `message_id`, `channel_id`, `sender`, `text`, `attachments`, `reply_to`, `actions`, `platform_data` | Outbound message envelope |
| `ChannelSendResponse` | `message_id` | Platform message ID for threading |
| `InboundMessage` | `message_id`, `channel_id`, `text`, `attachments`, `reply_to`, `actions`, `platform_data`, `timestamp` | Inbound message envelope |
| `MessageSender` | `name`, `role`, `bot_id` | Bot identity (enriched by broker) |
| `Attachment` | `type`, `url`, `filename`, `size` | File/image/audio attachment |
| `MessageAction` | `label`, `callback_id` | Interactive button/keyboard action |

Proto3 additive — existing apps that only use `channel_id` + `text` still work without changes.

---

## Bot Identity and Roles

Nebo bots have a three-axis identity model that shapes how they behave and present themselves:

- **`creature`** — What the bot is (archetype/competency): "Quick-witted strategist", "Meticulous researcher"
- **`role`** — How it relates to the user (relationship dynamic): "Friend", "COO", "Son", "Mentor", "Coach"
- **`vibe`** — Its energy (communication style): "Chill but opinionated", "Warm and encouraging"

**Why this matters for channel apps:** When your channel plugin receives an outbound message, the `sender` field contains the bot's `name` and `role`. Use this to format the display name appropriately for your platform:

- Slack: "Alex/COO" in the message header
- Discord: "Alex — COO" in the embed author field
- Telegram: "Alex (COO)" in the bot name

The role isn't a job title — it's a relationship descriptor. "Friend", "Son", and "Mentor" are valid roles alongside "COO" and "DevLead". Your plugin should display whatever the user has set without filtering or interpreting it.

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
2. Install the SDK (`go get github.com/nebolabs/nebo-sdk-go`)
3. Implement the handler interface for your capability (`ToolHandler`, `ChannelHandler`, etc.)
4. Register the handler and call `app.Run()`
5. Build a native binary (`go build -o binary .`)
6. Place binary + manifest.json in `apps/<your-app-id>/` directory
7. Nebo auto-discovers and launches it
