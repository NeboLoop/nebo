# Infrastructure Systems Deep-Dive

Comprehensive logic documentation for the Go infrastructure systems in `nebo/internal/`.
Source: `/Users/almatuck/workspaces/nebo/nebo/internal/`

---

## Table of Contents

1. [Message Bus (`msgbus/`)](#1-message-bus)
2. [Events System (`events/`)](#2-events-system)
3. [Keyring (`keyring/`)](#3-keyring)
4. [Credential Storage (`credential/`)](#4-credential-storage)
5. [Notification System (`notify/`)](#5-notification-system)
6. [Heartbeat Daemon (`daemon/`)](#6-heartbeat-daemon)
7. [Real-Time Hub (`realtime/`)](#7-real-time-hub)
8. [WebSocket Handler (`websocket/`)](#8-websocket-handler)

---

## 1. Message Bus

**Package:** `msgbus` | **Files:** `bus.go`, `message.go`, `sink.go`, `bus_test.go`

The message bus is the unified message coordinator. Every inbound message -- regardless of source (web UI, DM, cron, voice, CLI, etc.) -- flows through `Emit -> events.Subject -> route -> lane -> runner`, with output streamed back via the message's `ResponseSink`.

### 1.1 Core Types

#### `Source` (string enum)

```go
type Source string

const (
    SourceWebUI        Source = "web_ui"
    SourceCLI          Source = "cli"
    SourceOwnerDM      Source = "owner_dm"
    SourceExternalDM   Source = "external_dm"
    SourceLoopChannel  Source = "loop_channel"
    SourceLocalChannel Source = "local_channel"
    SourceVoice        Source = "voice"
    SourceHeartbeat    Source = "heartbeat"
    SourceCron         Source = "cron"
    SourceRecovery     Source = "recovery"
    SourceComm         Source = "comm"
    SourceIntro        Source = "intro"
)
```

12 distinct inbound sources. Every message is tagged with exactly one.

#### `InboundMessage` (the unified envelope)

```go
type InboundMessage struct {
    ID                string
    Source            Source
    SessionKey        string                         // Resolved BEFORE emit (e.g., "reminder-daily", "dm-abc123")
    Prompt            string
    System            string                         // Optional system prompt override
    UserID            string
    Origin            tools.Origin                   // Controls tool restrictions
    Lane              string                         // Target lane (e.g., "main", "events", "heartbeat")
    ModelOverride     string
    ForceSkill        string
    Channel           string                         // "web", "cli", "telegram"
    SkipMemoryExtract bool
    Metadata          map[string]any                 // Source-specific data
    PreExecute        func(ctx context.Context) error // Called inside lane, before runner.Run()
    Sink              ResponseSink                   // Routes response back to source
}
```

Key fields:
- `SessionKey` -- resolved by the adapter (caller) before emit, not by the bus.
- `Lane` -- required; determines which work queue processes this message.
- `PreExecute` -- optional hook for pre-flight setup within the lane's task context (e.g., marking recovery tasks as running).
- `Sink` -- required; the bus routes all streaming output through this interface.

### 1.2 Bus Struct and Initialization

```go
type Bus struct {
    subject   *events.Subject
    lanes     *agenthub.LaneManager
    runner    *runner.Runner
    sub       events.Subscription
    dmRelayer atomic.Value  // DMRelayer (lock-free, set-once at startup)
}
```

**Constructor:**

```go
func New(subject *events.Subject, lanes *agenthub.LaneManager) *Bus
```

- Creates a Bus and subscribes to the internal topic `"msgbus.inbound"` via `events.Subscribe(subject, topicInbound, b.route)`.
- The runner must be set separately via `SetRunner(r)` before any messages are emitted (late-binding pattern).

**Late-binding setters:**

```go
func (b *Bus) SetRunner(r *runner.Runner)
func (b *Bus) SetDMRelayer(r DMRelayer)   // atomic.Value store
```

`SetDMRelayer` uses `atomic.Value` for lock-free, set-once storage. When set, web UI companion chat responses are automatically relayed to NeboLoop as DMs.

### 1.3 Message Publishing

```go
func (b *Bus) Emit(msg InboundMessage) error
```

**Validation (all three required):**
1. `b.runner != nil` -- returns `"msgbus: runner not set"`
2. `msg.Sink != nil` -- returns `"msgbus: message has no sink"`
3. `msg.Lane != ""` -- returns `"msgbus: message has no lane"`

**Delivery:** Non-blocking. Calls `events.Emit(b.subject, topicInbound, msg)` which has a 5-second timeout via `select` with `time.After`.

### 1.4 Message Routing

```go
func (b *Bus) route(_ context.Context, msg InboundMessage) error
```

The route method is the event subscriber callback. It:
1. Discards the event handler's context (which has a deferred cancel).
2. Uses `context.Background()` instead -- the lane manages its own task lifecycle.
3. Calls `b.lanes.EnqueueAsync(ctx, msg.Lane, func(taskCtx) error { ... })` with a description string like `"web_ui: What is the weather..."`.

This is non-blocking; `EnqueueAsync` adds the task to the lane's queue.

### 1.5 Message Execution

```go
func (b *Bus) execute(ctx context.Context, msg InboundMessage) error
```

**DM Relay auto-wrapping:**
If `msg.Metadata["dm_relay"]` exists AND the DM relayer is connected AND has an owner conversation ID, the sink is wrapped in a `DMRelaySink`. The user's prompt is also relayed to the owner's NeboLoop conversation (non-blocking goroutine).

**Execution flow:**
1. Call `msg.PreExecute(ctx)` if set. On error, calls `sink.OnError()` and returns.
2. Call `b.runner.Run(ctx, &runner.RunRequest{...})` -- returns a `<-chan ai.StreamEvent`.
3. Range over the event channel:
   - `EventTypeText`: accumulate text in `strings.Builder`, call `sink.OnEvent()`.
   - `EventTypeMessage` (fallback): use `event.Message.Content` only if no text events arrived.
   - All events: forwarded to `sink.OnEvent()`.
4. After channel closes, call `sink.OnComplete(ctx, result.String())`.

### 1.6 ResponseSink Interface

```go
type ResponseSink interface {
    OnEvent(ctx context.Context, event ai.StreamEvent) error
    OnComplete(ctx context.Context, fullResult string) error
    OnError(ctx context.Context, err error) error
}
```

Every source type has its own sink implementation. The bus treats all sinks uniformly.

### 1.7 Sink Implementations

#### `HubFrameSink` -- Web UI / CLI chat

```go
type HubFrameSink struct {
    RequestID            string
    SendFrame            FrameSender
    SilentTool           func(tc *ai.ToolCall) bool
    ForwardToolResults   ToolResultForwarder
    OnResult             func(result string)
    Result               strings.Builder
}
```

Translates streaming events into WebSocket frames:
- `EventTypeText` -> `{"type":"stream", "id":requestID, "payload":{"chunk":text}}`
- `EventTypeToolCall` -> `{"type":"stream", "payload":{"tool":name, "tool_id":id, "input":input}}`
- `EventTypeToolResult` -> `{"type":"stream", "payload":{"tool_result":text, "tool_name":..., "tool_id":..., "image_url":...}}`
- `EventTypeThinking` -> `{"type":"stream", "payload":{"thinking":text}}`
- `EventTypeMessage` -> fallback to chunk if no text events arrived; forwards embedded `ToolResults` from CLI providers.
- `OnComplete` -> `{"type":"res", "id":requestID, "ok":true, "payload":{"result":fullResult}}`
- `OnError` -> `{"type":"res", "id":requestID, "ok":false, "error":err.Error()}`

Silent tool filtering: if `SilentTool` returns true for a tool call, both the tool call frame and its result frame are suppressed.

#### `OwnerDMSink` -- Bidirectional DM + Web UI broadcast

```go
type OwnerDMSink struct {
    SessionKey     string
    ConversationID string
    SendFrame      FrameSender
    SendDM         DMSender
    SendTyping     TypingSender
    Result         strings.Builder
}
```

Compound sink: streams events to web UI (as `"event"` frames with methods like `chat_stream`, `tool_start`, `tool_result`, `thinking`) AND sends the final result as a DM back through NeboLoop.

Frame format differs from HubFrameSink:
- Uses `"type":"event"` + `"method":"chat_stream"` (not `"type":"stream"`)
- Includes `"session_id"` and `"source":"dm"` in payload.

On complete: broadcasts `chat_complete` event, clears typing indicator, sends DM.

#### `ExternalDMSink` -- Private DM (no web UI)

```go
type ExternalDMSink struct {
    ConversationID string
    SendDM         DMSender
    SendTyping     TypingSender
    Result         strings.Builder
}
```

Accumulates text only (no streaming to UI). On complete, sends the final result as a DM. On error, sends the error message as a DM.

#### `LoopChannelSink` -- NeboLoop loop channel

```go
type LoopChannelSink struct {
    ChannelID      string
    ConversationID string
    SenderName     string
    Send           LoopChannelSender
    SendFrame      FrameSender         // Optional: broadcasts to web UI
    Result         strings.Builder
}
```

Accumulates text. On complete, optionally broadcasts a `loop_channel_message` event to the web UI, then sends to the loop channel via `Send()`.

#### `LocalChannelSink` -- Local app channel

```go
type LocalChannelSink struct {
    ChannelType string
    ChannelID   string
    Send        AppChannelSender
    Result      strings.Builder
}
```

Minimal: accumulates text, sends on complete via `Send(ctx, channelType, channelID, fullResult)`.

#### `CronSink` -- Cron/reminder output

```go
type CronSink struct {
    Name       string
    Message    string
    SendFrame  FrameSender
    Notify     func(title, body string)
    DeliverTo  *DeliverTarget
    OnResult   func(result string)
}

type DeliverTarget struct {
    Channel string
    To      string
    Send    AppChannelSender
}
```

On complete (three-step):
1. Native OS notification via `Notify("Nebo -- Reminder", message)`.
2. Web UI event: `{"type":"event","method":"reminder_complete","payload":{"name":..., "message":..., "result":...}}`.
3. Optional channel delivery via `DeliverTo.Send()`.
4. Post-completion hook via `OnResult(fullResult)`.

Ignores streaming events entirely.

#### `NullSink` -- Internal tasks (heartbeat, recovery)

```go
type NullSink struct {
    OnResult func(result string)
    Result   strings.Builder
}
```

Accumulates `EventTypeText` only. On complete, calls `OnResult` if set. Discards errors.

#### `RecoverySink` -- Task recovery lifecycle

```go
type RecoverySink struct {
    TaskID      string
    Description string
    MarkRunning func(ctx context.Context, id string) error
    MarkDone    func(ctx context.Context, id string) error
    MarkFailed  func(ctx context.Context, id string, reason string) error
}
```

Ignores events. On complete, calls `MarkDone(ctx, taskID)`. On error, calls `MarkFailed(ctx, taskID, err.Error())`.

#### `DMRelaySink` -- Bidirectional companion sync wrapper

```go
type DMRelaySink struct {
    Inner          ResponseSink
    ConversationID string
    SendDM         DMSender
}
```

Wraps any sink. Delegates all events to `Inner`. On complete, delegates to `Inner` first, then asynchronously relays the bot response to NeboLoop via `SendDM` (non-blocking goroutine).

### 1.8 Function Type Abstractions

```go
type FrameSender func(frame map[string]any) error
type DMSender func(ctx context.Context, conversationID, text string) error
type TypingSender func(ctx context.Context, conversationID string, typing bool)
type LoopChannelSender func(ctx context.Context, channelID, conversationID, text string) error
type AppChannelSender func(ctx context.Context, channelType, channelID, text string) error
type ToolResultForwarder func(requestID string, sendFrame FrameSender, toolResults json.RawMessage)
```

These abstractions prevent the msgbus package from importing cmd/nebo or neboloop types.

### 1.9 DMRelayer Interface

```go
type DMRelayer interface {
    SendDM(ctx context.Context, conversationID, text string) error
    RelayOwnerDM(ctx context.Context, conversationID, text string) error
    IsConnected() bool
    OwnerConversationID() string
}
```

Implemented by `neboloop.Plugin`. Enables bidirectional companion chat sync.

### 1.10 Cleanup

```go
func (b *Bus) Close()
```

Calls `b.sub.Unsubscribe()` to detach from the event topic.

### 1.11 Utility

```go
func truncatePrompt(s string, max int) string
```

Returns the first `max` bytes of `s` plus `"..."` if longer.

### 1.12 Test Coverage

File: `bus_test.go`

| Test | What it verifies |
|------|-----------------|
| `TestEmitRequiresRunner` | Emit returns `"msgbus: runner not set"` when runner is nil |
| `TestNullSinkAccumulatesResult` | NullSink accumulates EventTypeText and fires OnResult callback |
| `TestHubFrameSinkStreams` | HubFrameSink sends stream + thinking + res frames |
| `TestHubFrameSinkSkipsEmptyText` | Empty text events produce no frames |
| `TestHubFrameSinkSilentTool` | SilentTool filter suppresses tool call frames |
| `TestHubFrameSinkOnResultCallback` | OnResult callback fires on complete |
| `TestRecoverySinkLifecycle` | MarkDone on success, MarkFailed on error |
| `TestCronSinkNotifiesAndDelivers` | Notification sent, title/body correct, channel delivery |
| `TestOwnerDMSinkCompoundOutput` | Streams to web UI frames AND sends DM |
| `TestExternalDMSinkSendsDM` | Accumulates text, sends DM on complete |
| `TestLoopChannelSinkSendsToChannel` | Sends to correct channel ID |
| `TestTruncatePrompt` | Short strings pass through, long strings truncated with "..." |

---

## 2. Events System

**Package:** `events` | **Files:** `events.go`, `topics.go`

A generic, lock-free, in-process pub/sub system built on Go generics and atomics. The entire system is centered around the `Subject` type.

### 2.1 Core Types

#### `Subject`

```go
type Subject struct {
    subscribers atomic.Pointer[subscriberMap]  // Lock-free subscriber map
    cache       atomic.Pointer[[]event]        // Lock-free event cache (replay)
    nextSubID   int64                          // Atomic counter
    eventCount  int64                          // Atomic counter
    events      chan event                     // Single event channel (buffered)
    shutdown    chan struct{}                   // Shutdown signal
    config      subjectConfig                  // Read-only after creation
    closed      int32                          // Atomic flag for idempotent Close
    wg          sync.WaitGroup                 // Tracks eventLoop goroutine
}

type subscriberMap map[string]map[string]Subscription
// outer key: topic name
// inner key: subscription ID

type event struct {
    topic   string
    message any
}
```

#### `Subscription`

```go
type Subscription struct {
    Topic       string
    CreatedAt   int64
    Handler     HandlerFunc
    ID          string            // Format: "{topic}-{seqNum}"
    WantsReplay bool
    SentEvents  map[string]bool   // Replay dedup tracking
    Unsubscribe func()
}

type HandlerFunc func(context.Context, any) error
```

### 2.2 Configuration

```go
type subjectConfig struct {
    replayEnabled bool
    cacheSize     int
    bufferSize    int
    syncDelivery  bool
    logger        *slog.Logger
}
```

**Options:**

| Option | Default | Effect |
|--------|---------|--------|
| `WithBufferSize(size)` | 512 | Event channel buffer size |
| `WithReplay(cacheSize)` | disabled | Enables replay; caches last N events |
| `WithLogger(logger)` | nil | Structured logger for handler errors |
| `WithSyncDelivery()` | false (async) | Forces synchronous handler calls (useful for WebSocket writes) |

### 2.3 Constructor

```go
func NewSubject(opts ...SubjectOption) *Subject
```

1. Applies options over defaults (`bufferSize=512`).
2. Creates buffered `events` channel and `shutdown` channel.
3. Initializes atomic pointers (empty subscriber map, empty cache if replay enabled).
4. Spawns `eventLoop()` goroutine.

### 2.4 Event Loop

```go
func (s *Subject) eventLoop()
```

Single goroutine. Loop:
1. `select` on `shutdown` (return) or `events` channel (process).
2. Increment `eventCount` atomically.
3. If replay enabled, add to cache (copy-on-write).
4. Load subscribers atomically, iterate topic subscribers, deliver.

**Delivery modes:**
- **Async (default):** Each handler is called in its own goroutine.
- **Sync (`WithSyncDelivery`):** Handlers are called inline in the event loop goroutine. This serializes all handler calls, useful when handlers must not be called concurrently (e.g., WebSocket writes).

```go
func (s *Subject) sendToSubscriber(sub Subscription, evt event, sync bool)
```

Creates a `context.WithTimeout(context.Background(), 10*time.Second)` for each handler call. Errors are logged at Debug level if a logger is configured.

### 2.5 Publish (Emit)

```go
func Emit[T any](subject *Subject, topic string, value T) error
```

Generic function. Wraps the value in an `event{topic, value}` and sends to the subject's events channel. **5-second timeout** via `time.After` -- returns error if the channel is full and doesn't drain in time.

### 2.6 Subscribe

```go
func Subscribe[T any](subject *Subject, topic string, handler func(context.Context, T) error, replay ...bool) Subscription
```

1. Wraps the typed handler in a `HandlerFunc` with type assertion (`data.(T)`).
2. Generates a unique subscription ID: `"{topic}-{atomicSeqNum}"`.
3. Adds subscription via copy-on-write (`addSubscription`).
4. Sets up `Unsubscribe` closure that calls `removeSubscription(subID)`.
5. If replay enabled and `replay[0]` is true, replays cached events synchronously.

### 2.7 Copy-on-Write Subscriber Management

**Add:**
```go
func (s *Subject) addSubscription(sub Subscription)
```
CAS loop: load old map -> deep copy -> insert -> CompareAndSwap. Retries on CAS failure.

**Remove:**
```go
func (s *Subject) removeSubscription(subID string)
```
CAS loop: load -> copy -> find and delete by ID -> CompareAndSwap. If topic becomes empty, the topic key is also deleted.

**Deep copy:**
```go
func (s *Subject) copySubscribers(original subscriberMap) subscriberMap
```
Creates a new map with new inner maps, copying all subscriptions.

### 2.8 Replay Cache

```go
func (s *Subject) addToCache(evt event)
```
CAS loop: load old cache -> copy -> evict oldest if at capacity -> append -> CompareAndSwap.

```go
func (s *Subject) replayEvents(sub Subscription)
```
Iterates cache, delivers matching-topic events synchronously. Dedup via `sub.SentEvents` map keyed by `"{topic}-{message}"`.

### 2.9 Shutdown

```go
func Complete(s *Subject)
```

Idempotent via `atomic.CompareAndSwapInt32(&s.closed, 0, 1)`. Closes `shutdown` channel, waits for `wg.Wait()` with a 5-second timeout to prevent hanging.

### 2.10 Topics

```go
const TopicCDPBroadcast = "cdp.broadcast"

func CDPClientTopic(clientID string) string {
    return fmt.Sprintf("cdp.client.%s", clientID)
}
```

Additionally, the msgbus defines `topicInbound = "msgbus.inbound"` (internal to bus.go).

### 2.11 Thread Safety Model

The events system achieves lock-freedom for the hot path (event delivery) using:
- **Atomic pointers** for subscriber map and cache (reads are lock-free).
- **Copy-on-write** with CAS loops for mutations (subscribe/unsubscribe).
- **Single event loop goroutine** processes all events sequentially.
- **Atomic counters** for subscription IDs and event counts.

The only mutex is `sync.WaitGroup` used during shutdown.

---

## 3. Keyring

**Package:** `keyring` | **File:** `keyring.go`

Thin wrapper around `github.com/zalando/go-keyring` for OS keychain integration.

### 3.1 Constants

```go
const (
    serviceName = "nebo"
    accountName = "master-encryption-key"
)
```

All operations use service `"nebo"` and account `"master-encryption-key"`.

### 3.2 API

```go
func Get() ([]byte, error)
```
Retrieves the master encryption key from the OS keychain. The key is stored as hex-encoded string; this function decodes it to raw bytes via `hex.DecodeString`.

```go
func Set(key []byte) error
```
Stores the master encryption key. Encodes to hex string via `hex.EncodeToString` before writing to keychain.

```go
func Delete() error
```
Removes the master encryption key from the OS keychain.

```go
func Available() bool
```
Returns whether the OS keychain is functional:
1. If `NEBO_KEYRING_DISABLED=1` environment variable is set, returns `false` immediately (opt-in for headless/CI/Docker).
2. Otherwise, probes the keychain with a test write/read/delete cycle using service `"nebo-keyring-probe"` and account `"probe"`.

### 3.3 OS Keychain Backends

Via `github.com/zalando/go-keyring`:
- **macOS:** Keychain Services (via Security.framework)
- **Windows:** Credential Manager (via wincred)
- **Linux:** Secret Service D-Bus API (via freedesktop.org Secret Service)

### 3.4 Storage Format

The master encryption key is stored as a **hex-encoded string** in the OS keychain. All consumers receive/provide raw `[]byte`.

---

## 4. Credential Storage

**Package:** `credential` | **Files:** `credential.go`, `migrate.go`

Provides encryption-at-rest for all sensitive values stored in SQLite. Uses the master key from the keyring.

### 4.1 Module State

```go
var (
    encKey []byte       // Master encryption key
    mu     sync.RWMutex // Protects encKey
)

const encPrefix = "enc:"
```

### 4.2 Initialization

```go
func Init(key []byte)
```
Sets the master encryption key. Called once from ServiceContext at startup. Protected by write lock.

### 4.3 Encrypt / Decrypt

```go
func Encrypt(plaintext string) (string, error)
```
- Returns `""` for empty input.
- Reads `encKey` under read lock.
- Delegates to `mcpclient.EncryptString(plaintext, key)`.
- Prepends `"enc:"` prefix to ciphertext.

```go
func Decrypt(ciphertext string) (string, error)
```
- Returns `""` for empty input.
- Reads `encKey` under read lock.
- Strips `"enc:"` prefix (handles both prefixed and legacy non-prefixed ciphertext).
- Delegates to `mcpclient.DecryptString(raw, key)`.

```go
func IsEncrypted(s string) bool
```
Returns `strings.HasPrefix(s, "enc:")`.

### 4.4 Migration

```go
func Migrate(ctx context.Context, rawDB *sql.DB) error
```

Encrypts all plaintext credentials in the database. Runs in a **single transaction** -- rolls back entirely on failure. **Idempotent:** skips values that already have the `"enc:"` prefix.

**Tables migrated (in order):**

| Table | Column(s) | Special handling |
|-------|-----------|-----------------|
| `auth_profiles` | `api_key` | Skips empty and already-encrypted |
| `mcp_integration_credentials` | `credential_value` | Skips `oauth_token` type (already encrypted by MCP OAuth flow) |
| `app_oauth_grants` | `access_token`, `refresh_token` | Each column checked independently |
| `plugin_settings` | `setting_value` (where `is_secret = 1`) | Only secret settings |

**Algorithm for each table:**
1. `SELECT` all non-empty values.
2. Filter in Go: skip already-encrypted (`IsEncrypted()`), skip special types.
3. Collect rows to update.
4. For each row: encrypt value(s), `UPDATE` with encrypted value and `updated_at = unixepoch()`.
5. Return count of updated rows.

Logs total encrypted count on success.

---

## 5. Notification System

**Package:** `notify` | **File:** `notify.go`

Cross-platform native OS notifications.

### 5.1 API

```go
func Send(title, body string)
```

Sanitizes inputs, then dispatches based on `runtime.GOOS`:

| Platform | Command | Method |
|----------|---------|--------|
| `darwin` | `osascript -e 'display notification "body" with title "title"'` | AppleScript |
| `linux` | `notify-send title body` | libnotify / D-Bus |
| `windows` | PowerShell toast notification via `Windows.UI.Notifications` WinRT API | PowerShell |
| Other | Silent return | N/A |

Falls back silently on failure (logs error but does not propagate).

### 5.2 Sanitization

```go
func sanitize(s string) string
```

1. Replaces all `'` (single quote) with `'` (Unicode right single quotation mark) -- prevents shell injection in AppleScript.
2. Removes all `\` (backslash) characters.
3. Truncates to 256 characters + `"..."` if longer.

### 5.3 Windows Toast Notification (PowerShell)

```powershell
[Windows.UI.Notifications.ToastNotificationManager, ...] > $null
$template = [ToastNotificationManager]::GetTemplateContent([ToastTemplateType]::ToastText02)
$textNodes = $template.GetElementsByTagName('text')
$textNodes.Item(0).AppendChild($template.CreateTextNode('title'))
$textNodes.Item(1).AppendChild($template.CreateTextNode('body'))
$toast = [ToastNotification]::new($template)
[ToastNotificationManager]::CreateToastNotifier('Nebo').Show($toast)
```

Uses `ToastText02` template (title + body). Notifier app ID is `"Nebo"`.

---

## 6. Heartbeat Daemon

**Package:** `daemon` | **File:** `heartbeat.go`

Background daemon that enables proactive agent behavior via periodic ticks.

### 6.1 Configuration

```go
type HeartbeatConfig struct {
    Interval     time.Duration  // Default: 30 minutes
    InitialDelay time.Duration  // Default: 0 (run immediately)
    WorkspaceDir string         // Where to look for HEARTBEAT.md
    OnHeartbeat  func(ctx context.Context, prompt string) error
    OnCronFire   func(ctx context.Context, jobName, message string) error
    IsQuietHours func() bool    // nil = no quiet hours
}
```

### 6.2 Heartbeat Struct

```go
type Heartbeat struct {
    mu             sync.Mutex
    cfg            HeartbeatConfig
    ctx            context.Context
    stopCh         chan struct{}
    doneCh         chan struct{}
    running        bool
    wakeCh         chan string        // buffered(8), reason label
    events         []HeartbeatEvent
    eventsMu       sync.Mutex
    lastPromptHash uint64
}
```

### 6.3 HeartbeatEvent

```go
type HeartbeatEvent struct {
    Source    string     // "cron:daily-report", "app:weather"
    Summary  string
    Timestamp time.Time
}

const maxEvents = 20
```

Events are collected between ticks and included in the next heartbeat prompt.

### 6.4 Lifecycle

```go
func NewHeartbeat(cfg HeartbeatConfig) *Heartbeat
```
Sets default interval to 30 minutes. Creates channels.

```go
func (h *Heartbeat) Start(ctx context.Context)
```
Guarded by mutex. If not already running, sets `running = true` and spawns `go h.run(ctx)`.

```go
func (h *Heartbeat) Stop()
```
Guarded by mutex. Closes `stopCh`, waits for `doneCh` (blocking), sets `running = false`.

```go
func (h *Heartbeat) SetInterval(d time.Duration)
```
Guarded by mutex. If interval changed and daemon is running, stops and restarts with new interval (creates new channels).

### 6.5 Wake Mechanism

```go
func (h *Heartbeat) Wake(reason string)
```
Non-blocking send to `wakeCh` (buffered 8). If channel is full, the call is silently dropped (a tick is already pending).

Wake bypasses:
- Dedup check (prompt hash comparison).
- Quiet hours.

### 6.6 Event Queue

```go
func (h *Heartbeat) Enqueue(event HeartbeatEvent)
```
Thread-safe (separate `eventsMu`). Appends event. If buffer exceeds `maxEvents` (20), drops oldest events.

```go
func (h *Heartbeat) drainEvents() []HeartbeatEvent
```
Returns all queued events and clears the buffer.

### 6.7 Main Loop

```go
func (h *Heartbeat) run(ctx context.Context)
```

1. **Initial delay:** If configured, waits before first tick (allows agent to connect).
2. **Clock-aligned loop:**
   - Calculates next aligned time via `nextAlignedTime(now, interval)`.
   - Example: for a 5m interval at 00:03, fires at 00:05; for 30m at 14:12, fires at 14:30.
   - `select` on: `ctx.Done`, `stopCh`, `wakeCh` (immediate tick), `time.After(next)`.
3. **Quiet hours:** Timer ticks check `IsQuietHours()`; if true, skip. Wake ticks bypass quiet hours.

```go
func nextAlignedTime(now time.Time, interval time.Duration) time.Time {
    return now.Truncate(interval).Add(interval)
}
```

### 6.8 Tick Logic

```go
func (h *Heartbeat) tick(ctx context.Context, woken bool)
```

1. Load `HEARTBEAT.md` tasks.
2. Drain queued events.
3. If both empty, return (nothing to do).
4. Format prompt via `FormatHeartbeatPrompt(tasks, events)`.
5. **Dedup:** FNV-1a hash of prompt. If identical to `lastPromptHash` AND not woken, skip.
6. Call `cfg.OnHeartbeat(ctx, prompt)`.

### 6.9 HEARTBEAT.md Loading

```go
func (h *Heartbeat) loadHeartbeatFile() string
```

Search order:
1. `{WorkspaceDir}/HEARTBEAT.md`
2. `{DataDir}/HEARTBEAT.md` (via `defaults.DataDir()`)

Returns first file found (trimmed), or empty string.

### 6.10 Prompt Formatting

```go
func FormatHeartbeatPrompt(tasks string, events []HeartbeatEvent) string
```

Produces a structured prompt:

```
You are running a scheduled heartbeat check. Review the following proactive tasks...

## HEARTBEAT.md Tasks

{tasks}

## Recent Events

These events occurred since the last heartbeat:
- **{source}** ({HH:MM}): {summary}

---

For each task or event:
1. Check if the condition/trigger applies right now
2. If yes, take action (use tools as needed)
3. If the task says to notify the user, use the message tool

If no tasks need attention, respond with "HEARTBEAT_OK" and nothing else.
If you take action, briefly summarize what you did.
```

### 6.11 Quiet Hours

```go
func IsInQuietHours(start, end string, now time.Time) bool
```

- `start` and `end` are `"HH:MM"` strings.
- Handles overnight ranges (e.g., 22:00-07:00): returns true if `now >= start || now < end`.
- Same-day ranges (e.g., 09:00-17:00): returns true if `now >= start && now < end`.
- Returns false for empty or unparseable strings.

```go
func parseHHMM(s string) (int, bool)
```
Parses "HH:MM" to minutes since midnight. Validates 0-23 hours, 0-59 minutes.

### 6.12 Prompt Hashing

```go
func hashPrompt(s string) uint64
```
FNV-1a 64-bit hash. Used for dedup (skip tick if prompt unchanged).

---

## 7. Real-Time Hub

**Package:** `realtime` | **Files:** `hub.go`, `client.go`, `chat.go`, `rewrite_handler.go`

The real-time hub manages **browser WebSocket connections** (distinct from the agent hub in `agenthub/`). It broadcasts events from the agent to all connected browser clients.

### 7.1 Hub

```go
type Hub struct {
    clients    map[*Client]bool
    broadcast  chan []byte    // buffered 64
    register   chan *Client
    unregister chan *Client
    mu         sync.RWMutex
}
```

**Constructor:**
```go
func NewHub() *Hub
```

**Run loop:**
```go
func (h *Hub) Run(ctx context.Context)
```
Select on `ctx.Done`, `register`, `unregister`, `broadcast`:
- **Register:** Adds client to map under write lock.
- **Unregister:** Removes client, closes its `send` channel, under write lock.
- **Broadcast:** Iterates all clients under read lock, sends to each client's `send` channel. If buffer full, logs warning and drops message (does NOT disconnect the client).

**Broadcast methods:**
```go
func (h *Hub) Broadcast(message *Message) error
```
JSON-marshals the message and sends to broadcast channel.

```go
func (h *Hub) BroadcastToUser(userID string, message *Message) error
```
Targeted: only sends to clients with matching `UserID`.

```go
func (h *Hub) GetClientCount() int
```
Returns count under read lock.

### 7.2 Message Type

```go
type Message struct {
    Type      string                 `json:"type"`
    Channel   string                 `json:"channel,omitempty"`
    Data      map[string]interface{} `json:"data,omitempty"`
    Timestamp time.Time              `json:"timestamp"`
    UserID    string                 `json:"userId,omitempty"`
}
```

### 7.3 Client

```go
type Client struct {
    conn   *websocket.Conn
    send   chan []byte       // buffered 256
    hub    *Hub
    ID     string
    UserID string
    ctx    context.Context
    cancel context.CancelFunc
    closed atomic.Bool       // Lock-free closed flag
}
```

**Constants:**

| Constant | Value | Purpose |
|----------|-------|---------|
| `writeWait` | 10s | Max time to write a message |
| `pongWait` | 60s | Max time between pong messages |
| `pingPeriod` | 54s (pongWait * 9/10) | Ping interval |
| `maxMessageSize` | 32768 (32KB) | Max inbound message size |

**Constructor:**
```go
func NewClient(conn *websocket.Conn, hub *Hub, id, userID string) *Client
```
Creates context with cancel, buffered send channel (256).

**Read pump:**
```go
func (c *Client) readPump()
```
- Deferred: unregisters from hub, closes conn, cancels context.
- Sets read limit (32KB), read deadline (pongWait), pong handler (resets deadline).
- Loops: reads message, calls `handleTextMessage(msg)`.
- On error: logs unexpected close errors, breaks loop.

**Write pump:**
```go
func (c *Client) writePump()
```
- Ticker at `pingPeriod` (54s).
- Select on `send` channel, ticker, ctx.Done.
- Messages: sets write deadline, sends as TextMessage. Each message is its own WebSocket frame.
- Ping: sets write deadline, sends PingMessage.
- If send channel closed (hub closed it): sends CloseMessage.

**Message sending:**
```go
func (c *Client) SendMessage(msg *Message) error
```
- Uses `defer/recover` to handle race condition where channel closes between check and send.
- Checks `closed` atomic bool.
- JSON-marshals, non-blocking send to `send` channel.
- Returns `ErrClientClosed` or `ErrClientSendBufferFull`.

**Close:**
```go
func (c *Client) Close()
```
Uses `atomic.Bool.Swap(true)` -- only first caller proceeds. Cancels context, closes send channel, closes WebSocket conn.

**Entry point:**
```go
func ServeWS(hub *Hub, conn *websocket.Conn, clientID, userID string)
```
Creates client, registers with hub, spawns `writePump()` and `readPump()` goroutines.

### 7.4 Client Message Handling

```go
func (c *Client) handleMessage(msg *Message)
```

Routes by `msg.Type`:

| Type | Handler |
|------|---------|
| `"ping"` | `handlePing` -- sends pong response |
| `"rewrite"` | `handleRewrite` -- delegates to `rewriteHandler` |
| `"chat"` | `handleChat` -- delegates to `chatHandler` |
| `"approval_response"` | `handleApprovalResponse` -- delegates to `approvalResponseHandler` |
| `"request_introduction"` | `handleRequestIntroduction` -- delegates to `requestIntroductionHandler` |
| `"check_stream"` | `handleCheckStream` -- delegates to `checkStreamHandler` |
| `"cancel"` | `handleCancel` -- delegates to `cancelHandler` |
| `"session_reset"` | `handleSessionReset` -- delegates to `sessionResetHandler` |
| `"ask_response"` | `handleAskResponse` -- delegates to `askResponseHandler` |

All handlers are package-level `MessageHandler` variables set via `Set*Handler()` functions. If a handler is not registered, an error is logged.

### 7.5 Message Handler Registration

```go
type MessageHandler func(c *Client, msg *Message)

func SetRewriteHandler(handler MessageHandler)
func SetChatHandler(handler MessageHandler)
func SetApprovalResponseHandler(handler MessageHandler)
func SetRequestIntroductionHandler(handler MessageHandler)
func SetCheckStreamHandler(handler MessageHandler)
func SetCancelHandler(handler MessageHandler)
func SetSessionResetHandler(handler MessageHandler)
func SetAskResponseHandler(handler MessageHandler)
```

Package-level variables, set once during initialization.

### 7.6 ChatContext

```go
type ChatContext struct {
    hub              *agenthub.Hub
    svcCtx           *svc.ServiceContext
    pending          map[string]*pendingRequest    // requestID -> request info
    pendingMu        sync.RWMutex
    activeSessions   map[string]string             // sessionID -> requestID
    activeSessionsMu sync.RWMutex
    pendingApprovals map[string]string             // approvalID -> agentID
    pendingApprovalsMu sync.RWMutex
    pendingAsks      map[string]string             // requestID -> agentID
    pendingAsksMu    sync.RWMutex
    clientHub        *Hub
}
```

**pendingRequest:**
```go
type pendingRequest struct {
    client           *Client
    sessionID        string
    userID           string
    prompt           string
    createdAt        time.Time
    streamedContent  string
    isNewChat        bool
    toolCalls        []toolCallInfo
    thinking         string
    contentBlocks    []contentBlock
    messageID        string
    cleanSentLen     int  // Length of UTF-8-safe content already sent
}
```

**Content block types:**
```go
type contentBlock struct {
    Type          string          // "text", "tool", "image", or "ask"
    Text          string
    ToolCallIndex *int
    ImageURL      string
    AskRequestID  string
    AskPrompt     string
    AskWidgets    json.RawMessage
    AskResponse   string
}
```

### 7.7 ChatContext Initialization

```go
func NewChatContext(svcCtx *svc.ServiceContext, clientHub *Hub) (*ChatContext, error)
```

```go
func (c *ChatContext) SetHub(hub *agenthub.Hub)
```
Registers four handlers on the agent hub:
1. `hub.SetResponseHandler(c.handleAgentResponse)` -- receives agent responses and streams.
2. `hub.SetApprovalHandler(c.handleApprovalRequest)` -- receives tool approval requests.
3. `hub.SetAskHandler(c.handleAskRequest)` -- receives interactive user prompts.
4. `hub.SetEventHandler(c.handleAgentEvent)` -- receives broadcast events (lane updates, etc.).

### 7.8 Chat Handler Registration

```go
func RegisterChatHandler(chatCtx *ChatContext)
```

Registers all client-side message handlers:
- `chat` -> `handleChatMessage` (goroutine)
- `approval_response` -> `handleApprovalResponse` (goroutine)
- `request_introduction` -> `handleRequestIntroduction` (goroutine)
- `check_stream` -> `handleCheckStream` (goroutine)
- `cancel` -> `handleCancel` (goroutine)
- `session_reset` -> `handleSessionReset` (NOT a goroutine -- synchronous)
- `ask_response` -> `handleAskResponse` (goroutine)

### 7.9 Chat Message Flow

```go
func handleChatMessage(c *Client, msg *Message, chatCtx *ChatContext)
```

1. Extract `session_id`, `prompt`, `companion` (bool), `system` from `msg.Data`.
2. Wait for agent to connect (polls every 250ms, up to 5 seconds).
3. Session resolution:
   - **Empty session_id + companion mode:** `GetOrCreateCompanionChat` (single companion chat per user).
   - **Empty session_id + non-companion:** Create new chat with UUID, generate initial title from prompt (truncated to 50 chars), send `chat_created` event to client.
   - **Non-empty session_id:** Use as-is.
4. Create `pendingRequest` with `requestID = "chat-{unixNano}"`.
5. Track in `pending` map and `activeSessions` map.
6. Send `"run"` frame to agent via hub: `{type:"req", id:requestID, method:"run", params:{session_key, prompt, user_id, system}}`.

### 7.10 Agent Response Handling

```go
func (c *ChatContext) handleAgentResponse(agentID string, frame *agenthub.Frame)
```

**Stream frames (`frame.Type == "stream"`):**
- `payload["chunk"]` (text): Accumulates in `streamedContent`. Uses UTF-8 rune boundary detection to avoid splitting multi-byte characters. Delta sent to client via `sendChatStream`. Content blocks tracked.
- `payload["tool"]` (tool start): Flushes any buffered text first, then tracks tool call info (ID, name, input, status="running"), appends tool content block, triggers partial save, sends `sendToolStart` to client.
- `payload["tool_result"]`: Updates matching tool call (by ID, or first running as fallback), appends image content block if `image_url` present, sends `sendToolResult` and `sendImage` to client.
- `payload["thinking"]`: Accumulates thinking content, sends `sendThinking` to client.

**Response frames (`frame.Type == "res"`):**
1. Remove from `pending` and `activeSessions`.
2. Flush remaining buffered content.
3. If `!frame.OK`: send error to client.
4. If prompt is empty and streamed content exists: this is a title generation response -- update chat title in DB.
5. Otherwise: update chat timestamp, request title generation for new chats (goroutine), send `sendChatComplete` to client.

### 7.11 Agent Event Handling

```go
func (c *ChatContext) handleAgentEvent(agentID string, frame *agenthub.Frame)
```

Forwards agent events (lane updates, DM events, etc.) to **all** connected browser clients via `clientHub.Broadcast`. The frame's `Method` becomes the message `Type`.

### 7.12 Approval Flow

**Agent -> Clients:**
```go
func (c *ChatContext) handleApprovalRequest(agentID, requestID, toolName string, input json.RawMessage)
```
Tracks pending approval (`requestID -> agentID`). Broadcasts `approval_request` to all clients.

**Client -> Agent:**
```go
func (c *ChatContext) handleApprovalResponse(msg *Message)
```
Extracts `request_id`, `approved` (bool), `always` (bool). Looks up agentID, sends via `hub.SendApprovalResponseWithAlways`.

### 7.13 Ask Flow (Interactive Prompts)

**Agent -> Clients:**
```go
func (c *ChatContext) handleAskRequest(agentID, requestID, prompt string, widgets json.RawMessage)
```
Tracks pending ask. Appends `"ask"` content block to active pending request. Broadcasts `ask_request` to all clients.

**Client -> Agent:**
```go
func (c *ChatContext) handleAskResponse(msg *Message)
```
Extracts `request_id`, `value`. Updates ask content block with response. Sends via `hub.SendAskResponse`.

### 7.14 Cancel Flow

```go
func handleCancel(c *Client, msg *Message, chatCtx *ChatContext)
```

1. Send cancel frame to agent (best-effort; continues even if no agent).
2. Clean up `activeSessions` and `pending` maps.
3. **Always** broadcast `chat_cancelled` to all UI clients (so frontend can reset loading state).

### 7.15 Session Reset

```go
func handleSessionReset(c *Client, msg *Message, chatCtx *ChatContext)
```

1. Extract `session_id` (= chatID = sessionKey, e.g., `"companion-default"`).
2. Delete messages from `chat_messages` table.
3. Reset session metadata (lookup by name, then `ResetSession`).
4. Send `session_reset` result to client with `ok: true/false`.

### 7.16 Stream Resumption

```go
func handleCheckStream(c *Client, msg *Message, chatCtx *ChatContext)
```

When a client reconnects (e.g., page refresh), it sends `check_stream` with its session ID. If there's an active stream:
1. Updates the client reference in `pending` (so new chunks go to the new client).
2. Sends all accumulated content via `sendStreamStatus(c, sessionID, true, content)`.

### 7.17 Title Generation

```go
func (c *ChatContext) requestTitleGeneration(agentID, sessionID, userPrompt, assistantResponse string)
```

Sends a `generate_title` request to the agent with a prompt like:
```
Generate a short, descriptive title (3-6 words) for this conversation...
```

Tracked as a pending request with empty prompt (signals title request on response).

### 7.18 Introduction Request

```go
func handleRequestIntroduction(c *Client, msg *Message, chatCtx *ChatContext)
```

Waits for agent, registers pending request with marker prompt `"__introduction__"`, sends `introduce` frame to agent.

### 7.19 Rewrite Handler

```go
type RewriteHandler struct {
    svcCtx *svc.ServiceContext
}

func NewRewriteHandler(svcCtx *svc.ServiceContext) *RewriteHandler
func (h *RewriteHandler) Register()
func (h *RewriteHandler) handleMessage(c *Client, msg *Message)
```

Placeholder implementation. Currently echoes the message back as `{"type":"echo", ...}`.

### 7.20 Helper Functions

```go
func waitForAgent(hub *agenthub.Hub, timeout time.Duration) *agenthub.AgentConnection
```
Polls `hub.GetAnyAgent()` every 250ms for up to `timeout`.

```go
func extractStringOrJSON(v any) string
```
If string, returns directly. If map/slice, re-marshals to JSON string.

```go
func sendToClient(c *Client, msg *Message)
```
Nil-safe. Uses `c.SendMessage(msg)` with error classification (closed, buffer full, other).

### 7.21 Client -> Server Message Types

| Type | Data fields | Behavior |
|------|-------------|----------|
| `ping` | none | Returns `pong` |
| `chat` | `session_id`, `prompt`, `companion`, `system` | Routes to agent |
| `rewrite` | varies | Echo (placeholder) |
| `approval_response` | `request_id`, `approved`, `always` | Forwards to agent |
| `request_introduction` | `session_id` | Agent introduces itself |
| `check_stream` | `session_id` | Resumes active stream |
| `cancel` | `session_id` | Cancels active request |
| `session_reset` | `session_id` | Deletes messages, resets session |
| `ask_response` | `request_id`, `value` | Forwards user answer to agent |

### 7.22 Server -> Client Message Types

| Type | Data fields | Source |
|------|-------------|--------|
| `pong` | none | Ping response |
| `chat_created` | `session_id` | New chat created |
| `chat_stream` | `session_id`, `content` | Streaming text chunk |
| `chat_complete` | `session_id` | Stream finished |
| `chat_cancelled` | `session_id` | Request cancelled |
| `tool_start` | `session_id`, `tool`, `tool_id`, `input` | Tool execution started |
| `tool_result` | `session_id`, `result`, `tool_name`, `tool_id` | Tool execution finished |
| `image` | `session_id`, `image_url` | Tool produced an image |
| `thinking` | `session_id`, `content` | Extended thinking content |
| `error` | `session_id`, `error` | Error message |
| `stream_status` | `session_id`, `active`, `content` | Stream resumption status |
| `session_reset` | `session_id`, `ok` | Reset result |
| `approval_request` | `request_id`, `tool`, `input` | Tool approval needed |
| `ask_request` | `request_id`, `prompt`, `widgets` | Interactive prompt |
| (broadcast events) | varies by method | Agent events forwarded |

---

## 8. WebSocket Handler

**Package:** `websocket` | **Files:** `handler.go`, `handler_test.go`

HTTP handler that upgrades connections to WebSocket and manages authentication.

### 8.1 Upgrader

```go
var upgrader = websocket.Upgrader{
    ReadBufferSize:  1024,
    WriteBufferSize: 1024,
    CheckOrigin: func(r *http.Request) bool {
        origin := r.Header.Get("Origin")
        return origin == "" || middleware.IsLocalhostOrigin(origin)
    },
}
```

Only allows connections from localhost origins (or no origin header).

### 8.2 Handler

```go
func Handler(hub *realtime.Hub, accessSecret string) http.HandlerFunc
```

Authentication is **optional** -- this is a localhost-only single-user app. All HTTP API routes are public. If JWT is provided, it is validated; otherwise the connection is allowed as `"local"`.

**Flow:**

1. Generate `clientID = "client-{uuid8}"`.
2. Try pre-upgrade auth via `extractUserIDFromJWT(r, accessSecret)` -- checks:
   - `Authorization: Bearer {token}` header.
   - `nebo_token` cookie.
3. Upgrade to WebSocket regardless.
4. If pre-upgrade auth succeeded: send `auth_ok`, call `ServeWS`.
5. If no pre-upgrade auth: call `handlePostConnect(conn, secret)`.

### 8.3 Post-Connect Authentication

```go
func handlePostConnect(conn *websocket.Conn, secret string) string
```

**Auth deadline:** 5 seconds.

1. Set read deadline to 5 seconds.
2. Read first message.
   - **Timeout/disconnect:** Allow as `"local"`, send `auth_ok`.
   - **Unparseable JSON:** Allow as `"local"`, send `auth_ok`.
   - **`type: "auth"` with token:** Validate JWT. If valid, return `claims.Sub`. If invalid, allow as `"local"`.
   - **Any other type (e.g., `"connect"`):** Allow as `"local"`.
3. Reset read deadline.

### 8.4 Auth OK Message

```go
func sendAuthOK(conn *websocket.Conn, userID string)
```

Sends:
```json
{
    "type": "auth_ok",
    "data": {"user_id": "..."},
    "timestamp": "2024-01-01T00:00:00Z"
}
```

### 8.5 JWT Extraction

```go
func extractUserIDFromJWT(r *http.Request, secret string) string
```

1. Check `Authorization: Bearer {token}` header.
2. Check `nebo_token` cookie.
3. Validate via `middleware.ValidateJWTClaims(token, secret)`.
4. Return `claims.Sub` (user ID) or empty string.

### 8.6 Test Coverage

File: `handler_test.go`

| Test | What it verifies |
|------|-----------------|
| `TestHandler_ValidBearerToken` | Pre-upgrade auth via Authorization header succeeds (101 Switching Protocols) |
| `TestHandler_ValidCookieToken` | Pre-upgrade auth via `nebo_token` cookie succeeds |
| `TestHandler_PostConnectAuth` | Post-connect auth: client sends `{type:"auth", data:{token:...}}`, receives `auth_ok` |
| `TestHandler_PostConnectAuth_ForgedToken` | Forged JWT (wrong secret) results in connection close |
| `TestHandler_PostConnectAuth_NoMessage` | No auth message within deadline results in connection close |
| `TestHandler_LegacyUserIdOnly` | `?userId=` query param is NOT a valid auth mechanism; connection closes |

Test helper:
```go
func mintTestToken(claims jwt.MapClaims, secret string) string
func setupTestServer(t *testing.T) (*httptest.Server, func())
```

Uses `testSecret = "test-secret-key-12345"` and `jwt.SigningMethodHS256`.

---

## Cross-System Integration Map

```
                                    +----------------+
                                    |  OS Keychain   |
                                    |  (keyring/)    |
                                    +-------+--------+
                                            |
                                    Get master key
                                            |
                                    +-------v--------+
                                    |  credential/   |
                                    |  Init(key)     |
                                    |  Encrypt/      |
                                    |  Decrypt       |
                                    |  Migrate       |
                                    +----------------+
                                            |
                              Used by: ServiceContext startup
                                            |
+----------+    +----------+    +-----------v---------+
| Browser  |    | websocket|    |     realtime/       |
| Client   +--->+ handler  +--->+  Hub (clients)      |
|          |    | (auth,   |    |  ChatContext         |
|          |<---+ upgrade) |<---+  (pending, sessions)|
+----------+    +----------+    +-----------+---------+
                                            |
                            handleAgentResponse / handleAgentEvent
                                            |
                                    +-------v--------+
                                    |   agenthub/    |
                                    |   Hub (agent)  |
                                    +-------+--------+
                                            |
                              Agent WS frame dispatch
                                            |
                                    +-------v--------+
                                    |    msgbus/     |
                                    |  Bus.Emit()   |
                                    +-------+--------+
                                            |
                                    events.Emit
                                            |
                                    +-------v--------+
                                    |    events/     |
                                    |  Subject       |
                                    |  (pub/sub)     |
                                    +-------+--------+
                                            |
                                Bus.route (subscriber)
                                            |
                                    +-------v--------+
                                    |  LaneManager   |
                                    |  EnqueueAsync  |
                                    +-------+--------+
                                            |
                                    Bus.execute
                                            |
                                    +-------v--------+
                                    |    Runner      |
                                    |  runner.Run()  |
                                    +-------+--------+
                                            |
                              StreamEvents via Sink
                                            |
            +--+--+--+--+--+--+--+--+------+
            |  |  |  |  |  |  |  |  |
            v  v  v  v  v  v  v  v  v
     HubFrame  OwnerDM  ExternalDM  LoopChannel
     Sink      Sink     Sink        Sink
               LocalChannel  Cron   Null  Recovery
               Sink          Sink   Sink  Sink

+----------------+          +----------------+
|   daemon/      |          |   notify/      |
|  Heartbeat     +--------->+  Send()        |
|  (tick loop)   |          |  (OS native)   |
+----------------+          +----------------+
        |
        | OnHeartbeat callback
        v
  Bus.Emit (SourceHeartbeat)
```

### Data Flow Summary

1. **Browser -> Server:** WebSocket upgrade via `websocket/handler.go` (auth optional). Client registered in `realtime/Hub`.
2. **Client message:** Parsed by `realtime/client.go`, routed to handler by type.
3. **Chat message:** `realtime/chat.go` creates pending request, sends `"run"` frame to agent via `agenthub/Hub`.
4. **Agent processes:** Via `msgbus/Bus` -> `events/Subject` -> Lane -> `runner/Runner`.
5. **Agent responds:** Streams via appropriate `ResponseSink` implementation.
6. **Web UI sink:** Sends frames through `agenthub/Hub` -> `realtime/ChatContext.handleAgentResponse` -> `realtime/Hub.Broadcast` -> Client WebSocket.
7. **DM sinks:** Send via NeboLoop SDK.
8. **Cron sinks:** Native notification via `notify/Send()` + web UI event + optional channel delivery.
9. **Heartbeat:** Periodic ticks via `daemon/Heartbeat`, dispatched through `msgbus/Bus` like any other message.
10. **Credentials:** Master key from OS keychain (`keyring/`) initializes `credential/` package at startup. All DB secrets encrypted at rest.

---

## Rust Mapping Notes

This section documents the Rust equivalents (or gaps) for the Go infrastructure systems described above.

### Message Bus (`msgbus/`)

**Rust status: No direct equivalent. Replaced by a simpler architecture.**

The Go `msgbus` package is a heavyweight pub/sub coordinator that routes every inbound message through `events.Subject` to lanes and then to the runner. In Rust, this indirection is eliminated:

- **`crates/server/src/handlers/ws.rs`** — The `ClientHub` struct (`broadcast::Sender<HubEvent>`) replaces the `realtime/Hub` broadcast layer. It uses `tokio::sync::broadcast` (buffer size 256) instead of Go's goroutine-per-subscriber model.
- **`crates/server/src/handlers/ws.rs::handle_client_ws()`** — Handles WebSocket messages directly in a `tokio::select!` loop (client messages + hub broadcasts). There is no intermediate `ChatContext` or `pendingRequest` tracking layer; the handler calls `Runner::run()` inline and streams events back to the client.
- **`crates/agent/src/runner.rs`** — The `Runner` is called directly from WebSocket handlers and the scheduler, not via a message bus.
- **`crates/agent/src/lanes.rs`** — The `LaneManager` exists and provides per-lane task queuing with concurrency limits, equivalent to Go's `agenthub.LaneManager`. Tasks are enqueued directly from handlers rather than via a bus routing step.
- **ResponseSink pattern:** Not ported. Rust uses `tokio::sync::mpsc` channels and direct streaming via `axum::extract::ws::WebSocket`. The Go sink abstraction (HubFrameSink, OwnerDMSink, CronSink, etc.) is replaced by per-handler streaming logic.
- **DMRelayer:** Not yet ported. NeboLoop DM relay is handled in `crates/comm/` but there is no automatic bidirectional companion sync wrapper equivalent to `DMRelaySink`.

**Key difference:** Go uses `Emit -> Subject -> route -> lane -> runner` (4 hops). Rust uses `handler -> lane -> runner` (2 hops). The event-based decoupling is traded for simpler direct calls.

### Events System (`events/`)

**Rust status: Implemented, with a different design.**

The Go `events` package is a generic, lock-free, in-process pub/sub system (`Subject` with atomic pointers, copy-on-write subscriber maps, replay cache). The Rust version is purpose-built for workflow event dispatch rather than general pub/sub:

- **`crates/tools/src/events.rs`** — `EventBus` struct backed by `tokio::sync::mpsc::UnboundedSender<Event>`. Provides `emit()` for best-effort event delivery. The `Event` type carries `source` (String), `payload` (serde_json::Value), `origin` (String), and `timestamp` (u64). Much simpler than Go's generic `events.Subject`.
- **`crates/workflow/src/events.rs`** — `EventDispatcher` struct that consumes events from the `EventBus` receiver, matches them against role-owned `EventSubscription` entries (pattern matching with wildcard suffix support, e.g. `"email.*"`), and triggers workflow runs via `WorkflowManager::run()`. Spawns a single tokio task via `EventDispatcher::spawn()`.
- **`crates/tools/src/emit_tool.rs`** — `EmitTool` agent tool that allows workflow activities to fire events into the `EventBus`. Always injected for workflow activities.
- **Wired in `AppState`** — Both `event_bus: tools::EventBus` and `event_dispatcher: Arc<workflow::events::EventDispatcher>` are fields on `crates/server/src/state.rs::AppState`.

**What is NOT ported from Go `events/`:**
- No generic type-parameterized pub/sub (`Subscribe[T]`, `Emit[T]`). Rust uses a single concrete `Event` type.
- No replay cache or `WantsReplay` option.
- No copy-on-write atomic subscriber management (Rust uses `tokio::sync::RwLock` instead).
- No `WithSyncDelivery` option (all delivery is async via tokio tasks).
- No `WithBufferSize` configuration (unbounded channel).
- No topic-based routing (Go uses arbitrary topic strings; Rust events have a `source` field matched by the dispatcher).
- No `Subject`-level shutdown protocol. The bus drops naturally when the sender is dropped.

**CDP/browser topics** (`TopicCDPBroadcast`, `CDPClientTopic`): Not applicable. Browser automation in Rust uses `crates/browser/` with a different WebSocket-based CDP communication model.

### Keyring (`keyring/`)

**Rust status: Fully implemented.**

- **`crates/auth/src/keyring.rs`** — Uses the `keyring` crate (Rust equivalent of `github.com/zalando/go-keyring`). Same service/account naming: `SERVICE_NAME = "nebo"`, `ACCOUNT_NAME = "master-encryption-key"`.
- **API:** `available() -> bool`, `get() -> Option<String>`, `set(key: &str) -> Result<(), String>`, `delete() -> Result<(), String>`.
- **Difference from Go:** The Rust `get()` returns the key as a raw `String` (not hex-decoded bytes). The Go version stores hex-encoded and decodes to `[]byte`. The Rust version stores and retrieves the plaintext key string directly.
- **Difference from Go:** No `NEBO_KEYRING_DISABLED` environment variable check. The Rust `available()` function probes the keyring directly by attempting to read the master key entry. Go uses a separate write/read/delete probe cycle with a test entry.
- **OS backends:** Same cross-platform coverage via the `keyring` crate: macOS Keychain Services, Windows Credential Manager, Linux Secret Service D-Bus API.

### Credential Storage (`credential/`)

**Rust status: Fully implemented.**

- **`crates/auth/src/credential.rs`** — Global `OnceLock<mcp::crypto::Encryptor>` (Rust equivalent of Go's `sync.RWMutex`-protected `encKey []byte`). Same `"enc:"` prefix convention.
- **API:** `init(encryptor)`, `is_initialized() -> bool`, `encrypt(plaintext) -> Result<String, String>`, `decrypt(value) -> Result<String, String>`, `is_encrypted(value) -> bool`.
- **Encryption backend:** `crates/mcp/src/crypto.rs` — `Encryptor` struct using AES-256-GCM (via `aes-gcm` crate). Provides `encrypt()`, `decrypt()`, `encrypt_b64()`, `decrypt_b64()`, `from_passphrase()`, `generate()`. The Go version delegates to `mcpclient.EncryptString/DecryptString`.
- **Migration:** Not yet ported. Go's `credential.Migrate()` encrypts all plaintext credentials across 4 tables in a single transaction. The Rust codebase does not have an equivalent migration function for existing plaintext values.

### Keychain Tool (Agent-facing)

**Rust status: Implemented (not present in Go `internal/` — this is Rust-only).**

- **`crates/tools/src/keychain_tool.rs`** — `KeychainTool` is an agent-accessible tool (implements `DynTool`) for cross-platform credential storage management. Actions: `get`, `find`, `add`, `delete`. Platform-specific implementations via `#[cfg(target_os)]`: macOS (`security` CLI), Linux (`secret-tool` / libsecret), Windows (`cmdkey`).
- This is distinct from the internal `keyring` module — the keychain tool gives the *agent* access to the user's OS credential store, while `auth::keyring` is for Nebo's own master encryption key.

### Notification System (`notify/`)

**Rust status: Fully implemented.**

- **`crates/notify/src/lib.rs`** — `pub fn send(title, body)` with the same sanitization and platform dispatch as Go. macOS uses AppleScript, Linux uses `notify-send`, Windows uses PowerShell toast.

### Heartbeat Daemon (`daemon/`)

**Rust status: Partially covered by the scheduler.**

- **`crates/server/src/scheduler.rs`** — A cron scheduler loop that polls enabled `cron_jobs` every 60 seconds. Handles both `bash` and `agent` task types. This covers the cron firing portion of Go's `daemon/Heartbeat`.
- **`crates/tools/src/event_tool.rs`** — `EventTool` provides agent-accessible CRUD for scheduled tasks (create, list, delete, pause, resume, run, history). Equivalent to Go's cron job management.
- **What is NOT ported:**
  - `HEARTBEAT.md` file loading and heartbeat prompt formatting.
  - Clock-aligned tick intervals (Rust uses a fixed 60s poll).
  - Wake mechanism (non-blocking immediate tick bypass).
  - Quiet hours checking.
  - Event queue (collecting events between ticks for inclusion in heartbeat prompt).
  - Prompt dedup via FNV-1a hashing.

### Real-Time Hub (`realtime/`)

**Rust status: Implemented with simpler architecture.**

- **`crates/server/src/handlers/ws.rs`** — `ClientHub` (broadcast channel) + `handle_client_ws()` (per-connection tokio task). Replaces Go's `realtime/Hub` + `Client` + `ChatContext` + handler registration pattern.
- **Supported client message types:** `chat`, `cancel`, `approval_response`, `ask_response`, `session_reset`, `ping` (via WebSocket protocol ping/pong).
- **Supported server event types:** `connected`, `chat_stream`, `chat_complete`, `tool_call`, `tool_result`, `thinking`, `error`, `chat_cancelled`, `approval_request`, `ask`, `session_reset`.
- **What is NOT ported:**
  - `check_stream` (stream resumption on reconnect).
  - Title generation request flow (auto-titling new chats).
  - Introduction request handler.
  - Rewrite handler.
  - `DMRelaySink` / bidirectional companion sync.
  - Silent tool filtering.
  - Content block tracking and partial save.

### WebSocket Handler (`websocket/`)

**Rust status: Implemented differently.**

- **`crates/server/src/handlers/ws.rs`** — Axum's built-in WebSocket upgrade (`axum::extract::WebSocketUpgrade`) replaces the Go `gorilla/websocket` upgrader + custom auth flow.
- **Auth:** The Rust WebSocket endpoint is currently unauthenticated (localhost-only, single-user desktop app). Go's pre-upgrade JWT auth, post-connect auth, and cookie-based auth are not ported.
- **Ping/pong:** Handled by Axum/tokio-tungstenite at the protocol level rather than application-level ping/pong messages.

---

*Last updated: 2026-03-10*
