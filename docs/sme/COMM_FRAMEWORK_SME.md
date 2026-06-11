# Communication Plugin Framework — SME Reference

Comprehensive Subject Matter Expert document covering the Nebo communication
plugin framework: plugin architecture, wire protocol, NeboAI gateway integration,
message routing, conversation management, and cross-system interactions.

**Status:** Current (Rust implementation) | **Last updated:** 2026-06-05

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [Crate Structure](#2-crate-structure)
3. [CommPlugin Trait](#3-commplugin-trait)
4. [ChannelProvider Trait](#4-channelprovider-trait)
5. [Plugin Manager](#5-plugin-manager)
6. [Wire Protocol](#6-wire-protocol)
7. [Frame Codec](#7-frame-codec)
8. [Compression](#8-compression)
9. [ULID Generation](#9-ulid-generation)
10. [Message Deduplication](#10-message-deduplication)
11. [NeboAI Plugin](#11-neboai-plugin)
12. [Connection Lifecycle](#12-connection-lifecycle)
13. [Conversation Management](#13-conversation-management)
14. [Read Loop](#14-read-loop)
15. [Write Loop](#15-write-loop)
16. [Message Routing — Inbound](#16-message-routing--inbound)
17. [Message Routing — Outbound](#17-message-routing--outbound)
18. [Agent Spaces](#18-agent-spaces)
19. [NeboAI REST API Client](#19-neboai-rest-api-client)
20. [API Type System](#20-api-type-system)
21. [Loopback Plugin](#21-loopback-plugin)
22. [Server Integration](#22-server-integration)
23. [Chat Dispatch Integration](#23-chat-dispatch-integration)
24. [Tool Integration — Loop Tool](#24-tool-integration--loop-tool)
25. [Reconnection & Resilience](#25-reconnection--resilience)
26. [Token Rotation & Security](#26-token-rotation--security)
27. [DevLog — Traffic Inspection](#27-devlog--traffic-inspection)
28. [Error Handling](#28-error-handling)
29. [Configuration](#29-configuration)
30. [File Manifest](#30-file-manifest)

---

## 1. Architecture Overview

The comm framework provides a plugin-based communication layer that connects Nebo
desktop instances to the NeboAI cloud network. It handles real-time messaging
between bots, channels, agent spaces, and human users.

```
+------------------------------------------------------------------+
|  Nebo Desktop (local process)                                     |
|                                                                   |
|  +------------------+     +-------------------------------+       |
|  | Server (Axum)    |     | Agent Runner                  |       |
|  |                  |     |                               |       |
|  | handlers/        |     | orchestrator.rs               |       |
|  |   neboai.rs    |     | runner.rs                     |       |
|  |   agents.rs      |     +--------+----------------------+       |
|  |   chat.rs        |              |                              |
|  +-------+----------+              |                              |
|          |                         | CommMessage                  |
|          | activate_neboai()     | (via handler callback)       |
|          v                         v                              |
|  +-------+--------------------------------------------+           |
|  | PluginManager                                      |           |
|  |                                                    |           |
|  |  plugins: HashMap<String, Arc<dyn CommPlugin>>     |           |
|  |  active: Option<Arc<dyn CommPlugin>>               |           |
|  |  handler: Option<MessageHandler>                   |           |
|  |  topics: Vec<String>                               |           |
|  +-------+--------------------------------------------+           |
|          |                                                        |
|          | delegates to active plugin                             |
|          v                                                        |
|  +-------+---------------------+  +-------------------+           |
|  | NeboAIPlugin (active)     |  | LoopbackPlugin    |           |
|  |                             |  | (testing only)    |           |
|  |  WebSocket + Binary Framing |  +-------------------+           |
|  |  Read Loop  (tokio::spawn)  |                                  |
|  |  Write Loop (tokio::spawn)  |                                  |
|  |  Join Processor (tokio task)|                                  |
|  +-------+---------------------+                                  |
|          |                                                        |
+----------|--------------------------------------------------------+
           | tokio-tungstenite WebSocket (wss://)
           v
+----------+---------------------+
| NeboAI Comms Gateway         |
|                                |
|  Binary framing (47-byte hdr)  |
|  JWT auth (bot credentials)    |
|  Conversation multiplexing     |
|  Seq-based ordering + replay   |
|  Zstd compression              |
+--------------------------------+
```

### Key Design Decisions

- **Single active plugin model** — only one comm plugin is active at a time. The
  `PluginManager` routes all operations through it. Future plugins (Slack, Discord)
  would replace or co-exist via provider routing.
- **Binary framing over WebSocket** — not JSON-over-WS. A 47-byte binary header
  carries metadata (type, flags, msg_id, conversation_id, seq) with a JSON payload.
  This enables compression, deduplication, and ordering at the framing level.
- **Lock-free connected flag** — `AtomicBool` for `is_connected()` avoids holding
  async locks on the hot path. The read loop sets it false on disconnect.
- **Snapshot-then-release pattern** — the manager clones the `Arc<dyn CommPlugin>`
  before doing any I/O, releasing the `RwLock` so other callers are never blocked
  during network operations.

---

## 2. Crate Structure

**Crate:** `nebo-comm` (workspace member under `crates/comm/`)

```
crates/comm/
  Cargo.toml
  src/
    lib.rs           — Module declarations, CommPlugin + ChannelProvider traits, re-exports
    types.rs         — CommMessage, CommError, AgentCard, MessageHandler, ManagerStatus
    manager.rs       — PluginManager: registry, activation, routing, shutdown
    neboai.rs      — NeboAIPlugin: WebSocket connection, read/write loops, join processor
    loopback.rs      — LoopbackPlugin: in-memory test plugin
    frame.rs         — 47-byte binary header codec (encode/decode)
    wire.rs          — JSON payload types (ConnectPayload, DeliveryPayload, JoinPayload, etc.)
    compress.rs      — Zstd compression/decompression (threshold: >1KB)
    ulid.rs          — Monotonic ULID generator (16-byte IDs)
    dedup.rs         — Sliding-window message deduplication (1000 IDs / 5 min)
    devlog.rs        — Human-readable traffic log for `tail -f` debugging
    api.rs           — NeboAIApi: REST client for marketplace, loops, channels, billing
    api_types.rs     — DTOs for REST API responses (apps, skills, workflows, loops, billing)
```

### Dependencies

| Dependency            | Purpose                                     |
|-----------------------|---------------------------------------------|
| `tokio`               | Async runtime, channels, timers             |
| `tokio-tungstenite`   | WebSocket client (binary frames)            |
| `tokio-util`          | CancellationToken for graceful shutdown      |
| `futures`             | SplitSink/SplitStream for WS read/write     |
| `serde` / `serde_json`| JSON serialization for payloads             |
| `zstd`                | Frame payload compression                   |
| `reqwest`             | HTTP client for REST API                    |
| `getrandom`           | CSPRNG for ULID random component            |
| `napp`                | Plugin manifest types, platform detection   |
| `tracing`             | Structured logging                          |
| `thiserror`           | Error type derivation                       |

---

## 3. CommPlugin Trait

The central trait that all communication transport plugins implement. Defined in
`lib.rs`:

```rust
#[async_trait::async_trait]
pub trait CommPlugin: Send + Sync {
    // Identity
    fn name(&self) -> &str;
    fn version(&self) -> &str;

    // Lifecycle
    async fn connect(&self, config: HashMap<String, String>) -> Result<(), CommError>;
    async fn disconnect(&self) -> Result<(), CommError>;
    fn is_connected(&self) -> bool;

    // Messaging
    async fn send(&self, msg: CommMessage) -> Result<(), CommError>;
    async fn subscribe(&self, topic: &str) -> Result<(), CommError>;
    async fn unsubscribe(&self, topic: &str) -> Result<(), CommError>;

    // A2A Registration
    async fn register(&self, agent_id: &str, card: &AgentCard) -> Result<(), CommError>;
    async fn deregister(&self) -> Result<(), CommError>;

    // Handler (set by PluginManager)
    fn set_message_handler(&self, handler: MessageHandler);

    // Loop/Channel queries (default: not supported)
    async fn list_channels(&self) -> Result<Vec<LoopChannelInfo>, CommError>;
    async fn list_loops(&self) -> Result<Vec<LoopInfo>, CommError>;
    async fn get_loop_info(&self, loop_id: &str) -> Result<LoopInfo, CommError>;
    async fn list_channel_messages(&self, channel_id: &str, limit: usize)
        -> Result<Vec<ChannelMessageItem>, CommError>;
    async fn list_channel_members(&self, channel_id: &str)
        -> Result<Vec<ChannelMemberItem>, CommError>;

    // Token rotation
    async fn take_rotated_token(&self) -> Option<String>;

    // Agent space lookups
    async fn agent_slug_for_conv(&self, conv_id: &str) -> Option<String>;
    async fn agent_space_loop_id(&self, conv_id: &str) -> Option<String>;
    async fn agent_space_conv_for_slug(&self, slug: &str) -> Option<String>;

    // Disconnect notification
    async fn wait_disconnect(&self);
}
```

**Key design notes:**

- `set_message_handler` is deliberately synchronous (`fn`, not `async fn`) because
  it must work with `std::sync::RwLock` in the NeboAI plugin — the handler is set
  from the PluginManager which may already hold a tokio lock.
- Default implementations return `Err(CommError::Other("not supported"))` for
  optional query methods, allowing plugins to selectively implement capabilities.
- `wait_disconnect()` defaults to `std::future::pending()` (never returns) for
  plugins that do not support disconnect notification.

---

## 4. ChannelProvider Trait

A secondary trait for routing agent responses back through communication channels.
Currently the NeboAI plugin implements this via `CommPlugin::send()`. Future
channel plugins (Slack, Discord) would implement this independently.

```rust
#[async_trait::async_trait]
pub trait ChannelProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn send_response(&self, msg: CommMessage) -> Result<(), CommError>;
}
```

The server stores channel providers in `AppState::channel_providers`:
```rust
pub channel_providers: Arc<tokio::sync::RwLock<HashMap<String, Arc<dyn ChannelProvider>>>>,
```

When `chat_dispatch` needs to send a reply via comm, it checks
`channel_providers` first, then falls back to `comm_manager.send()`.

---

## 5. Plugin Manager

**File:** `crates/comm/src/manager.rs`

The `PluginManager` is the single entry point for all comm operations. It maintains
a registry of plugins and delegates operations to the currently active one.

```rust
pub struct PluginManager {
    inner: RwLock<Inner>,
}

struct Inner {
    plugins: HashMap<String, Arc<dyn CommPlugin>>,
    active: Option<Arc<dyn CommPlugin>>,
    handler: Option<MessageHandler>,
    topics: Vec<String>,
}
```

### Operations

| Method                 | Description                                                  |
|------------------------|--------------------------------------------------------------|
| `register(plugin)`    | Add plugin to registry (does not activate)                   |
| `unregister(name)`    | Remove plugin, disconnect if active                          |
| `set_active(name)`    | Activate a specific plugin, disconnect previous if different |
| `connect_active(cfg)` | Connect the active plugin with config map                    |
| `send(msg)`           | Send through active plugin (checks `is_connected()` first)  |
| `subscribe(topic)`    | Subscribe on active plugin, track in topics list             |
| `unsubscribe(topic)`  | Unsubscribe on active plugin, remove from topics list        |
| `set_message_handler` | Set callback for incoming messages, wire to active plugin    |
| `shutdown()`          | Disconnect all plugins, clear active and topics              |
| `wait_disconnect()`   | Await unexpected disconnect from active plugin               |
| `take_rotated_token()` | Consume rotated JWT from active plugin                      |
| `agent_slug_for_conv()`| Delegate agent space lookup to active plugin                |

### Snapshot-then-Release Pattern

For `connect_active()`, the manager clones the `Arc<dyn CommPlugin>`, releases
the `RwLock`, then does network I/O. This prevents holding the lock during
potentially long connection attempts:

```rust
pub async fn connect_active(&self, config: HashMap<String, String>) -> Result<(), CommError> {
    let plugin = {
        let inner = self.inner.read().await;
        inner.active.clone().ok_or(CommError::NoActivePlugin)?
    };
    // Lock released -- safe to do network I/O
    plugin.connect(config).await
}
```

---

## 6. Wire Protocol

**File:** `crates/comm/src/wire.rs`

JSON payload types for the NeboAI comms binary protocol. Both the gateway and
Rust SDK use these as the single source of truth. All payloads use `camelCase`
serialization.

### Frame Payload Types

| Frame Type         | Direction        | Payload Struct        | Key Fields                                    |
|--------------------|------------------|-----------------------|-----------------------------------------------|
| CONNECT (1)        | client -> server | `ConnectPayload`      | `botId`, `token`                              |
| AUTH_OK (2)        | server -> client | `AuthResultPayload`   | `ok`, `botId`, `plan`, `token` (rotated)      |
| AUTH_FAIL (3)      | server -> client | `AuthResultPayload`   | `ok=false`, `reason`                          |
| JOIN_CONVERSATION (4) | client -> server | `JoinPayload`      | `conversationId` OR `botId+stream` OR `channelId`, `lastAckedSeq` |
| LEAVE_CONVERSATION (5) | client -> server | `LeavePayload`   | `conversationId`                              |
| SEND_MESSAGE (6)   | client -> server | `SendPayload`         | `conversationId`, `stream`, `content` (JSON)  |
| MESSAGE_DELIVERY (7) | server -> client | `DeliveryPayload`  | `senderId`, `stream`, `content`, `agentId`, `agentSlug`, `sourceChannelId` |
| ACK (8)            | client -> server | `AckPayload`          | `conversationId`, `ackedSeq`                  |
| REPLAY (12)        | server -> client | `ReplayPayload`       | `conversationId`, `fromSeq`, `toSeq`, `messageCount` |

Other frame types (PRESENCE=9, TYPING=10, SLOW_DOWN=11, CLOSE=13) exist in the
protocol but do not carry structured payloads in the current implementation.

### Join Resolution

The `JoinPayload` supports three modes:
1. **By conversation ID** — resume an existing conversation
2. **By bot_id + stream** — join a named bot stream (dm, chat, installs, account, voice)
3. **By channel_id** — join a loop channel

The `JoinResultPayload` includes all resolution data: `conversationId`, `channelId`,
`channelName`, `loopId`, `peerId`, `peerType`, `agentId`, `agentSlug`.

---

## 7. Frame Codec

**File:** `crates/comm/src/frame.rs`

Every message over the WebSocket is a binary frame consisting of a 47-byte
fixed header followed by a variable-length JSON payload.

### Header Layout (47 bytes, big-endian)

```
Offset  Size  Field            Type    Notes
------  ----  ---------------  ------  -----------------------------------------
[0]     1     proto_version    u8      Always 1
[1]     1     frame_type       u8      1-13 (see constants below)
[2]     1     flags            u8      bit0=compressed, bit1=encrypted, bit2=ephemeral
[3-6]   4     payload_len      u32     Max 32,768 bytes (32 KB)
[7-22]  16    msg_id           bytes   ULID (monotonic, time-ordered)
[23-38] 16    conversation_id  bytes   UUID (16-byte binary)
[39-46] 8     seq              u64     Sequence number for ordering/replay
```

### Frame Type Constants

```rust
pub const TYPE_CONNECT: u8              = 1;
pub const TYPE_AUTH_OK: u8              = 2;
pub const TYPE_AUTH_FAIL: u8            = 3;
pub const TYPE_JOIN_CONVERSATION: u8    = 4;
pub const TYPE_LEAVE_CONVERSATION: u8   = 5;
pub const TYPE_SEND_MESSAGE: u8         = 6;
pub const TYPE_MESSAGE_DELIVERY: u8     = 7;
pub const TYPE_ACK: u8                  = 8;
pub const TYPE_PRESENCE: u8             = 9;
pub const TYPE_TYPING: u8              = 10;
pub const TYPE_SLOW_DOWN: u8           = 11;
pub const TYPE_REPLAY: u8             = 12;
pub const TYPE_CLOSE: u8              = 13;
```

### Flag Bits

```rust
pub const FLAG_COMPRESSED: u8  = 1 << 0;  // Payload is zstd-compressed
pub const FLAG_ENCRYPTED: u8   = 1 << 1;  // Payload is encrypted (reserved)
pub const FLAG_EPHEMERAL: u8   = 1 << 2;  // Not persisted by gateway
```

### Encode / Decode

```rust
pub fn encode(mut header: Header, payload: &[u8]) -> Result<Vec<u8>, FrameError>;
pub fn decode(data: &[u8]) -> Result<(Header, &[u8]), FrameError>;
pub async fn read_frame<R: AsyncReadExt + Unpin>(r: &mut R) -> Result<(Header, Vec<u8>), FrameError>;
```

The `encode` function sets `proto_version = 1` and `payload_len` automatically.
The `decode` function validates version, payload size, and minimum buffer length.
The `read_frame` async function reads exactly one frame from a `tokio::io::AsyncReadExt`.

### Error Types

```rust
pub enum FrameError {
    BadVersion { got: u8 },
    PayloadTooLarge { len: u32 },
    ShortRead { need: usize, got: usize },
    Io(std::io::Error),
}
```

---

## 8. Compression

**File:** `crates/comm/src/compress.rs`

Uses Zstd (level 1) compression with a 1 KB threshold. Payloads smaller than
1024 bytes are sent uncompressed to avoid overhead on small messages.

```rust
const COMPRESSION_THRESHOLD: usize = 1024;

pub fn compress(payload: &[u8]) -> (Vec<u8>, bool);    // Returns (data, was_compressed)
pub fn decompress(data: &[u8]) -> Result<Vec<u8>, io::Error>;
```

Compression is applied at the frame level. When `was_compressed` is true, the
frame header's `FLAG_COMPRESSED` bit is set. The read loop checks
`header.is_compressed()` and decompresses before JSON parsing.

---

## 9. ULID Generation

**File:** `crates/comm/src/ulid.rs`

Monotonic ULID generator following the Crockford ULID spec. Used for outgoing
message IDs (`msg_id` in the frame header).

### Layout (16 bytes)

```
[0-5]   48-bit Unix millisecond timestamp (big-endian)
[6-15]  80-bit random, monotonically incrementing within same millisecond
```

Within the same millisecond, the random component is incremented (not re-randomized).
This guarantees strict ordering even under high throughput.

```rust
pub struct UlidGen {
    inner: Mutex<[u8; 16]>,   // Thread-safe via std::sync::Mutex
}

impl UlidGen {
    pub fn new() -> Self;
    pub fn next(&self) -> [u8; 16];    // Generate next monotonic ULID
}

pub fn timestamp_ms(id: &[u8; 16]) -> u64;    // Extract millisecond timestamp
pub fn to_u64(id: &[u8; 16]) -> u64;          // First 8 bytes for comparison/sorting
```

---

## 10. Message Deduplication

**File:** `crates/comm/src/dedup.rs`

Per-connection sliding window deduplicator. Tracks up to 1000 message IDs or
5 minutes, whichever is reached first. Created fresh on each `connect()`.

```rust
const WINDOW_SIZE: usize = 1000;
const WINDOW_TTL: Duration = Duration::from_secs(5 * 60);

pub struct DedupWindow {
    inner: Mutex<Vec<Entry>>,  // Each entry: [u8; 16] msg_id + Instant seen
}

impl DedupWindow {
    pub fn is_duplicate(&self, msg_id: [u8; 16]) -> bool;  // true = skip this message
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
}
```

The dedup window is checked in the read loop for every `TYPE_MESSAGE_DELIVERY`
frame. This prevents processing the same message twice during network hiccups,
reconnects with replay, or gateway retransmissions.

**Eviction strategy:**
1. Remove all entries older than `WINDOW_TTL` (5 minutes)
2. If still at capacity (1000), remove the oldest entry
3. Insert the new message ID

---

## 11. NeboAI Plugin

**File:** `crates/comm/src/neboai.rs`

The production `CommPlugin` implementation. Connects to the NeboAI comms gateway
via `tokio-tungstenite`, authenticates with binary framing, and dispatches typed
messages.

### Internal State

```rust
pub struct NeboAIPlugin {
    inner: RwLock<Inner>,                          // send_tx, cancel token, API client
    handler: std::sync::RwLock<Option<MessageHandler>>,  // Sync lock (trait is sync)
    bot_id: RwLock<String>,
    rotated_token: RwLock<Option<String>>,          // Rotated JWT from AUTH_OK
    conv_maps: Arc<RwLock<ConvMaps>>,              // Conversation routing tables
    ulid_gen: UlidGen,                             // Monotonic message IDs
    devlog: RwLock<Option<DevLog>>,                // Traffic log
    connected: Arc<AtomicBool>,                    // Lock-free connected flag
    disconnect_notify: Arc<Notify>,                // Signal for unexpected disconnects
}

struct Inner {
    send_tx: Option<mpsc::Sender<Vec<u8>>>,        // Write loop channel
    cancel: Option<CancellationToken>,             // Graceful shutdown
    api: Option<Arc<NeboAIApi>>,                 // REST API client
}
```

### Background Tasks (3 spawned tasks per connection)

```
NeboAIPlugin::connect()
   |
   +-- tokio::spawn(read_loop)        -- reads WS frames, decodes, dispatches
   |
   +-- tokio::spawn(write_loop)       -- drains send channel, writes to WS + pings
   |
   +-- tokio::spawn(join_processor)   -- processes JoinUpdate from read_loop,
                                         updates ConvMaps
```

---

## 12. Connection Lifecycle

### Connect Sequence

```
Client                              Gateway
  |                                    |
  |--- WS CONNECT (wss://...) ------->|
  |<-- WS UPGRADE 101 ----------------|
  |                                    |
  |--- CONNECT frame {botId, token} ->|
  |                                    |  (JWT verification)
  |<-- AUTH_OK {ok, botId, plan,       |
  |        token (rotated)} ----------|
  |                                    |
  |--- JOIN {botId, stream="dm"} ----->|
  |<-- JOIN_RESULT {conversationId} ---|
  |                                    |
  |--- JOIN {botId, stream="installs"}->|
  |<-- JOIN_RESULT -------------------|
  |                                    |
  |--- JOIN {botId, stream="chat"} --->|
  |<-- JOIN_RESULT -------------------|
  |                                    |
  |--- JOIN {botId, stream="account"}->|
  |<-- JOIN_RESULT -------------------|
  |                                    |
  |--- JOIN {botId, stream="voice"} -->|
  |<-- JOIN_RESULT -------------------|
  |                                    |
  | (ready — read/write loops active)  |
```

### Default Bot Streams

On connect, the plugin automatically joins these 5 streams:
- `dm` — direct messages from other bots or human users
- `installs` — install/uninstall events from NeboAI marketplace
- `chat` — owner chat messages (web UI or mobile app)
- `account` — plan changes, token refreshes, account events
- `voice` — voice pipeline events (reserved)

### Disconnect Handling

1. `disconnect()` sets `connected = false`, cancels the `CancellationToken`,
   and drops `send_tx`. All three background tasks exit.
2. On unexpected disconnect (read loop exits without cancel), the read loop
   sets `connected = false` and signals `disconnect_notify`.
3. The server's reconnect loop awaits `comm_manager.wait_disconnect()` and
   triggers `activate_neboai()` after a backoff delay.

### Ghost Connection Prevention

Before connecting, the plugin tears down any existing connection:
```rust
if self.is_connected() {
    self.disconnect().await.ok();
} else {
    // Clean up stale state even if not connected
    let mut inner = self.inner.write().await;
    if let Some(cancel) = inner.cancel.take() {
        cancel.cancel();
    }
    inner.send_tx = None;
}
```

---

## 13. Conversation Management

The `ConvMaps` struct tracks all active conversations, updated by the join
processor task as JOIN_RESULT frames arrive.

```rust
struct ConvMaps {
    conv_by_key: HashMap<String, String>,            // "botId:stream" -> conv_id
    pending_joins: Vec<String>,                      // FIFO queue for unresolved joins
    channel_convs: HashMap<String, String>,          // channel_id -> conv_id
    channel_by_conv: HashMap<String, String>,        // conv_id -> channel_id
    channel_meta: HashMap<String, ChannelMeta>,      // channel_id -> metadata
    dm_convs: HashMap<String, DmPeer>,               // conv_id -> DmPeer
    dm_by_peer: HashMap<String, String>,             // peer_id -> conv_id
    agent_space_convs: HashMap<String, AgentSpaceMeta>,  // conv_id -> meta
    agent_space_by_slug: HashMap<String, String>,    // slug -> conv_id
    agent_space_by_id: HashMap<String, String>,      // agent_id -> conv_id
}
```

### Join Update Processing

The read loop sends `JoinUpdate` variants through an `mpsc` channel to the
join processor task:

```rust
enum JoinUpdate {
    BotStream { key: String, conversation_id: String },
    Channel(ChannelMeta, String),       // meta, conversation_id
    Dm(DmPeer, String),                 // peer, conversation_id
    AgentSpace(AgentSpaceMeta, String), // meta, conversation_id
}
```

The join processor applies updates to `ConvMaps` in a single task, avoiding
lock contention between the read loop and public query methods.

### Resolution Logic

When `send()` is called with a `CommMessage`, the plugin resolves the target
conversation:

1. If `msg.conversation_id` is non-empty, use it directly
2. If `msg.to` is non-empty, check `agent_space_by_slug` first
3. Fall back to `dm_by_peer` lookup
4. Error if no conversation found

The `stream` field defaults to `"agent_space"` if resolved via agent slug,
otherwise `"dm"`.

---

## 14. Read Loop

The read loop is the core inbound message handler. It runs as a spawned tokio
task and processes WebSocket messages until cancellation or error.

```
read_loop(read, handler, join_tx, dedup, cancel, devlog, connected, disconnect_notify)
```

### Processing Pipeline

```
WebSocket Frame
  |
  +-- Binary? --> decode frame header + payload
  |                |
  |                +-- Compressed? --> zstd decompress
  |                |
  |                +-- Match frame_type:
  |                     |
  |                     +-- TYPE_MESSAGE_DELIVERY
  |                     |     +-- dedup check (skip if duplicate)
  |                     |     +-- parse DeliveryPayload
  |                     |     +-- build CommMessage
  |                     |     +-- call handler(msg)
  |                     |
  |                     +-- TYPE_JOIN_CONVERSATION (JOIN_RESULT)
  |                     |     +-- parse JoinResultPayload
  |                     |     +-- send JoinUpdate to join_tx
  |                     |
  |                     +-- TYPE_SLOW_DOWN
  |                     |     +-- log warning (rate limiting)
  |                     |
  |                     +-- TYPE_REPLAY
  |                     |     +-- log info (message replay)
  |                     |
  |                     +-- TYPE_CLOSE
  |                          +-- break (server closing)
  |
  +-- Ping? --> log, continue
  +-- Pong? --> log, continue
  +-- Close? --> log, break
```

### Timeout Detection

The read loop wraps `read.next()` in a 60-second timeout. If no data is received
(4x the 15s ping interval), the connection is treated as dead — this handles
system sleep/wake scenarios where the TCP connection is silently broken.

### CommMessage Construction from Delivery

```rust
CommMessage {
    id: ulid_string(header.msg_id),
    from: delivery.sender_id,
    to: bot_id,
    topic: delivery.stream,
    conversation_id: uuid_from_bytes(header.conversation_id),
    msg_type: determine_type(delivery.stream),  // "dm" -> Message, "installs" -> Info, etc.
    content: delivery.content.to_string(),
    metadata: { agent_id, agent_slug, source_channel_id },
    timestamp: ulid::timestamp_ms(header.msg_id),
    human_injected: false,
    human_id: None,
    // A2A fields: None
}
```

### Stream-to-MessageType Mapping

| Stream          | CommMessageType   |
|-----------------|-------------------|
| `dm`            | `Message`         |
| `chat`          | `Message`         |
| `channel`       | `LoopChannel`     |
| `installs`      | `Info`            |
| `account`       | `Info`            |
| `agent_space`   | `Mention`         |
| (other)         | `Message`         |

---

## 15. Write Loop

The write loop drains the `mpsc` send channel and writes binary frames to the
WebSocket. It also sends periodic pings.

```
write_loop(write, send_rx, cancel, devlog)
```

### Ping Interval

The write loop sends WebSocket ping frames every 15 seconds to keep the
connection alive and detect dead connections. The gateway expects regular
pings and may disconnect idle clients.

### Graceful Shutdown

On cancellation, the write loop sends a `TYPE_CLOSE` frame before exiting,
allowing the gateway to clean up the connection state.

---

## 16. Message Routing — Inbound

Inbound messages flow from the NeboAI gateway through the comm framework
into the Nebo agent system.

```
NeboAI Gateway
  |
  | TYPE_MESSAGE_DELIVERY frame
  v
NeboAIPlugin read_loop
  |
  | CommMessage (via MessageHandler callback)
  v
PluginManager message handler
  |
  | (set by server during startup)
  v
handle_comm_message() [server/src/lib.rs]
  |
  +-- topic == "account"  --> route to account event handler
  |
  +-- topic == "installs" --> route to install event handler
  |
  +-- topic == "agent_space" --> route to agent runner via chat_dispatch
  |     (agent_slug resolved from conv_maps)
  |
  +-- topic == "dm" --> route to agent runner via chat_dispatch
  |
  +-- topic == "chat" --> route to agent runner via chat_dispatch
  |
  +-- (other) --> route based on session key lookup
```

### Agent Space Routing

When a message arrives on an `agent_space` stream, `handle_comm_message()`:

1. Extracts `agent_slug` from metadata (set by the read loop from `DeliveryPayload`)
2. Looks up the local agent by slug in the `agent_registry`
3. Resolves entity config (permissions, model override) for the agent
4. Builds a `ChatConfig` with `comm_reply` set to route responses back via NeboAI
5. Dispatches through `chat_dispatch::run_chat()`

### Session Key Construction

For non-agent-space messages, a session key is built:
```
neboai:{topic}:{conversation_id}
```

This maps to a persistent session in the agent runner, allowing context
continuity across reconnects.

---

## 17. Message Routing — Outbound

Outbound messages flow from the agent runner back through the comm framework
to the NeboAI gateway.

```
Agent Runner (chat_dispatch)
  |
  | streaming response chunks
  v
chat_dispatch stream handler
  |
  | CommMessage (coalesced, ~500ms batches)
  v
send_comm_response() [server/src/chat_dispatch.rs]
  |
  +-- Check channel_providers (e.g., "neboai")
  |     |
  |     +-- Found? --> provider.send_response(msg)
  |     |
  |     +-- Not found? --> comm_manager.send(msg)
  |
  v
NeboAIPlugin::send()
  |
  +-- Resolve conversation_id from conv_maps
  +-- Build SendPayload { conversation_id, stream, content }
  +-- Encode frame (TYPE_SEND_MESSAGE)
  +-- Queue via send_tx (mpsc channel)
  |
  v
write_loop
  |
  | Binary WebSocket frame
  v
NeboAI Gateway
```

### CommReplyConfig

When chat_dispatch processes a comm-originated message, it receives a
`CommReplyConfig` (`chat_dispatch.rs:121`):

```rust
pub struct CommReplyConfig {
    pub provider: String,        // "neboai"
    pub topic: String,           // stream name (e.g., "agent_space", "dm", "channel")
    pub conversation_id: String, // target conversation
}
```

This config tells chat_dispatch where to send the response after the agent
runner completes.

`comm_reply` is set for **all** inbound comm paths — the four `CommReplyConfig`
sites in `server/src/lib.rs` (~2678, 2835, 2927, 3401):

- **DM** (`topic: "agent_space"` for a DM rerouted to an agent space, else
  `msg.topic`)
- **agent_space** (`topic: msg.topic`)
- **channel** (`topic: "channel"`)

Because every path carries a `comm_reply`, tool-activity mirroring and clean
streaming (below) work identically in DMs, agent spaces, and channels.

### Stream Coalescing

Streaming LLM responses are coalesced before sending via comm to reduce
gateway traffic. A ~500ms coalesce window batches token chunks into larger
messages, with `metadata.stream = "true"` to indicate partial responses.

### Wire Type Tagging

`NeboAIPlugin::send()` (`neboai.rs:640`) tags `content["type"]` on the wire with
the serde-serialized `CommMessageType` for **every** message type **except**
`Message`. `Message` stays typeless on purpose: the frontend uses the absent
type as the signal to *finalize* the streamed bubble in place (replace the
accumulated streaming fragments with the full text) rather than appending a new
bubble.

The `FLAG_EPHEMERAL` flag is set for `Stream | ToolActivity`
(`neboai.rs:631-634`) — both are transient signals the frontend assembles live
and the gateway fans out without persisting, so a history replay shows one clean
final `Message` instead of intermediate fragments and tool chatter.

### Tool-Activity Mirroring

To make a loop reply render exactly like the local app — a collapsed
"Used N tools" timeline instead of raw tool dumps or endless "working" messages —
chat_dispatch mirrors tool events to the reply channel as a new comm message
type, `CommMessageType::ToolActivity` (serde `tool_activity`, defined in
`comm/src/types.rs:39`).

`send_comm_tool_activity()` (`chat_dispatch.rs:1368`) emits two frames per tool,
both carrying a `stream_id` (`metadata.stream_id`) that ties the tool frames to
the reply bubble they belong to:

- On `StreamEventType::ToolCall` (`chat_dispatch.rs:434`) — a `start` frame
  carrying `phase`, `tool`, `tool_id`, the human label (e.g. "reading a file"),
  and the tool input as `request` metadata (capped to 2000 chars).
- On `StreamEventType::ToolResult` (`chat_dispatch.rs:501`) — a `result` frame
  carrying the tool output text (trimmed and capped to 4000 chars, well under
  the 32 KB frame limit) plus an `is_error` flag.

`tool_activity_label()` (`chat_dispatch.rs:38`) maps a tool name to a human label
("running a command", "searching files", "reading a file", "searching the web",
"reading a page", …); the default is "working". The same label also drives a
live `send_typing()` activity signal alongside the mirrored frame.

### Heartbeat Suppression

Orchestrator progress heartbeats — the transient `_Working on: ..._` /
`_Working..._` status the sub-agent runner emits every 30s — are **not** streamed
to the loop. `is_progress_heartbeat()` (`chat_dispatch.rs:64`) detects them
(trimmed text starting with `_Working` and ending with `_`); the stream path
skips them and the loop instead gets a live `send_typing()` signal
(`chat_dispatch.rs:391`). `strip_progress_heartbeats()` (`chat_dispatch.rs:73`)
removes any heartbeat lines from the finalized response so the noise never lands
in the message that replaces the streamed bubble.

### Reply Attribution & Persona

`SendPayload` has no agent id/slug field, so the only identity hint a reply
carries is `senderName` inside `content.metadata` (set from
`agent_display_name`, `chat_dispatch.rs:754`). The responding agent's display
name resolves via the fallback chain `registry.get(agent_id).name →
store.get_agent(agent_id).name → "Nebo"` — the `store.get_agent` step is needed
because an agent that is *exposed* to the loop but not *enabled* locally is not
in the in-memory registry (channel path: `lib.rs:3275-3280`; agent_space path:
`lib.rs:2615-2624`).

Persona is resolved with `entity_config::resolve_for_chat("agent", agent_id)`
for any resolved agent, in **both** the DM/agent_space path (`lib.rs:2658`,
`2815`) and the channel path (`lib.rs:3351`). This makes a secondary agent answer
with **its** persona rather than the primary "Nebo" persona.

### Frontend Rendering (NeboLoop, brief)

On the NeboLoop side, the loop frontend turns the `tool_activity` start/result
frames into a "Used N tools" details element for both channels and DMs
(`attachToolActivity` in `app/src/lib/stores/chat.ts`, consumed for `type ===
"tool_activity"` on both the channel and agent-space paths), and renders
`#channel` references as clickable chips
(`app/src/lib/components/chat/MessageArea.svelte`). (Repo:
`neboloop`, separate from this desktop repo.)

---

## 18. Agent Spaces

Agent spaces are a NeboAI feature that gives each registered agent its own
conversation endpoint within a loop. Users can @mention agents or interact
with them directly.

### Registration Flow

1. Server calls `NeboAIApi::register_agent(loop_id, name, slug, description)`
2. Gateway auto-creates an agent space conversation
3. Gateway sends JOIN_RESULT with `agentId` and `agentSlug` fields
4. Join processor stores in `agent_space_convs`, `agent_space_by_slug`, `agent_space_by_id`

### Lookup Methods

```rust
// conv_id -> agent_slug
async fn agent_slug_for_conv(&self, conv_id: &str) -> Option<String>;

// conv_id -> loop_id
async fn agent_space_loop_id(&self, conv_id: &str) -> Option<String>;

// slug -> conv_id
async fn agent_space_conv_for_slug(&self, slug: &str) -> Option<String>;
```

These are used by:
- `handle_comm_message()` to determine which local agent should process an inbound message
- `send()` to resolve the target conversation when sending to an agent by slug

### Source Channel Handling

When a message is @mentioned from a loop channel (rather than sent directly to
the agent space), the `DeliveryPayload` includes `source_channel_id`. This is
preserved in the `CommMessage.metadata` so the agent can reference the originating
channel context.

---

## 19. NeboAI REST API Client

**File:** `crates/comm/src/api.rs`

`NeboAIApi` is a comprehensive REST client for the NeboAI platform. It uses
the owner's OAuth JWT for authentication and covers all 5 hierarchy layers plus
loops, billing, and content protection.

### Construction

```rust
pub struct NeboAIApi {
    api_server: String,
    bot_id: String,
    token: RwLock<String>,   // std::sync::RwLock — token can be refreshed
    client: Client,          // reqwest with 5s connect / 15s total timeout
}
```

### API Coverage

| Category           | Endpoints                                                    |
|--------------------|--------------------------------------------------------------|
| **Products**       | `list_products`, `install_product`, `uninstall_product`      |
| **Apps**           | `list_apps`, `get_app`, `get_app_reviews`, `install_app`, `uninstall_app` |
| **Skills**         | `list_skills`, `get_skill`, `list_top_skills`, reviews, media, feedback |
| **Workflows**      | `list_workflows`, `install_workflow`, `uninstall_workflow`   |
| **Agents**         | `get_agent`, `install_agent`, `uninstall_agent`              |
| **Publishing**     | `publish_skill`, `publish_agent`, `submit_for_review`        |
| **Loops**          | `join_loop`, `list_bot_loops`, `get_loop`, `list_loop_members` |
| **Loop Agents**    | `register_agent`, `list_agents`, `deregister_agent`          |
| **Channels**       | `list_bot_channels`, `list_channel_members`, `list_channel_messages` |
| **Code Redemption**| `redeem_code` (universal), `install_skill`, `install_workflow` |
| **Plugins**        | `get_plugin`, `download_plugin_binary`                       |
| **Billing**        | prices, subscription, checkout, portal, setup intent, cancel, invoices, payment methods |
| **Bot Identity**   | `update_bot_identity`, `register_bot`                        |
| **Content Protection** | `fetch_license_keys`                                     |
| **Referral**       | `referral_code`                                              |
| **Downloads**      | `download_napp`, `fetch_raw`                                 |

### Standalone Function

```rust
pub async fn redeem_code(api_server, code, name, purpose, bot_id) -> Result<RedeemCodeResponse, CommError>;
```

This is unauthenticated — used during initial onboarding before the bot has
credentials.

---

## 20. API Type System

**File:** `crates/comm/src/api_types.rs`

Comprehensive DTOs for all NeboAI REST API responses. All types use
`#[serde(rename_all = "camelCase")]` for JSON compatibility.

### Key Type Categories

- **App types:** `AppItem`, `AppDetail`, `AppsResponse`, `ChangelogEntry`
- **Skill types:** `SkillItem`, `SkillDetail`, `SkillsResponse`
- **Workflow types:** `WorkflowItem`, `WorkflowDetail`, `WorkflowsResponse`
- **Agent types:** `AgentItem`, `AgentDetail`, `AgentInfo`, `AgentDetailResponse`
- **Loop types:** `Loop`, `LoopDetail`, `LoopMember`, `LoopMembership`
- **Channel types:** `LoopChannel`, `ChannelMember`, `ChannelMessageRaw`, `NormalizedChannelMessage`
- **Install types:** `InstallResponse`, `CodeRedeemResponse`, `CodeRedeemArtifact`
- **Connection types:** `RedeemCodeRequest`, `RedeemCodeResponse`
- **Event types:** `InstallEvent`, `TaskSubmission`, `TaskResult`, `DirectMessage`, `ChannelMessage`
- **Content protection:** `LicenseKeyEntry`, `LicenseKeysResponse`

### Channel Message Normalization

Raw channel messages from the API have a nested JSON payload format:
```json
{ "content": { "text": "..." }, "metadata": { "role": "..." } }
```

`ChannelMessagesResponse::normalize()` parses this into flat `NormalizedChannelMessage`
structs for easier consumption by the tool layer.

### Platform-Specific Downloads

`InstallResponse::download_url()` resolves the correct binary URL for the
current platform (e.g., `darwin-arm64`, `linux-x86_64`).

---

## 21. Loopback Plugin

**File:** `crates/comm/src/loopback.rs`

In-memory `CommPlugin` for testing. Messages sent via `send()` are logged.
Messages can be injected via `inject_message()` which calls the registered
handler if subscribed to the message's topic.

```rust
pub struct LoopbackPlugin {
    inner: RwLock<Inner>,  // handler, connected, topics, agent_id
}

impl LoopbackPlugin {
    pub fn new() -> Self;
    pub fn inject_message(&self, msg: CommMessage);  // Simulate inbound message
}
```

Uses `std::sync::RwLock` (not tokio) since all operations are synchronous.
Registered alongside NeboAIPlugin during server startup but never activated
in production.

---

## 22. Server Integration

**File:** `crates/server/src/lib.rs`, `crates/server/src/state.rs`

### Startup Sequence

1. Create `PluginManager::new()`
2. Register `NeboAIPlugin` and `LoopbackPlugin`
3. Set initial message handler (broadcasts to WebSocket hub)
4. Store in `AppState::comm_manager`
5. After full state initialization, replace message handler with full version
   that routes chat/DM to agent runner
6. If NeboAI is enabled, call `activate_neboai()` for auto-connect
7. Spawn reconnect loop that monitors `wait_disconnect()`

### AppState Fields

```rust
pub struct AppState {
    pub comm_manager: Arc<PluginManager>,
    pub channel_providers: Arc<tokio::sync::RwLock<HashMap<String, Arc<dyn ChannelProvider>>>>,
    pub personal_loop_id: Arc<tokio::sync::RwLock<Option<String>>>,
    // ... other fields
}
```

### Message Handler Phases

**Phase 1 (early startup):** Simple broadcast to WebSocket hub:
```rust
Arc::new(move |msg: comm::CommMessage| {
    comm_hub.broadcast("comm_message", serde_json::to_value(&msg).unwrap());
})
```

**Phase 2 (after full init):** Full routing with agent runner:
```rust
Arc::new(move |msg: comm::CommMessage| {
    let st = handler_state.clone();
    tokio::spawn(async move {
        handle_comm_message(st, msg).await;
    });
})
```

### Shutdown Sequence

1. Stop app sidecars
2. Call `comm_manager.shutdown()` to disconnect all plugins
3. This cancels all background tasks (read loop, write loop, join processor)

---

## 23. Chat Dispatch Integration

**File:** `crates/server/src/chat_dispatch.rs`

Chat dispatch is the bridge between comm messages and the agent runner.

### ChatConfig for Comm Messages

When a comm message triggers a chat run, a `ChatConfig` is built with:

```rust
ChatConfig {
    channel: "neboai".to_string(),
    lane: lanes::COMM,
    comm_reply: Some(CommReplyConfig {
        provider: "neboai".to_string(),
        topic: msg.topic,
        conversation_id: msg.conversation_id,
    }),
    source: format!("neboai.{}", msg.topic),
    origin: "neboai".to_string(),
    // ... other fields
}
```

### Response Streaming via Comm

During `run_chat()`, streaming response chunks are coalesced and sent back
through the comm channel:

1. Chunks accumulate in `comm_buffer`
2. Every ~500ms (`COMM_COALESCE_MS`), the buffer is flushed as a `CommMessage`
   with `metadata.stream = "true"`
3. After completion, the full response is sent as a final `CommMessage` with
   `metadata.stream = "false"` (or absent)

### Provider Resolution

`send_comm_response()` checks `channel_providers` first, then falls back to
`comm_manager.send()`:

```rust
async fn send_comm_response(provider, comm_manager, channel_providers, msg) {
    if let Some(providers) = channel_providers {
        if let Some(p) = providers.get(provider) {
            return p.send_response(msg).await;
        }
    }
    if let Some(cm) = comm_manager {
        return cm.send(msg).await;
    }
}
```

---

## 24. Tool Integration — Loop Tool

**File:** `crates/tools/src/loop_tool.rs`

The `loop` STRAP domain tool exposes NeboAI communication capabilities to the
agent. It takes an `Arc<dyn CommPlugin>` and provides sub-commands for:

| Category  | Actions                                              |
|-----------|------------------------------------------------------|
| `dm`      | `send` — send a direct message to a bot/person       |
| `channel` | `send`, `messages`, `members` — channel operations   |
| `group`   | `list` — list loop channels                          |
| `loop`    | `list`, `info`, `members` — loop management          |
| `topic`   | `subscribe`, `unsubscribe` — topic management        |
| `status`  | (default) — connection and plugin status              |

The tool guards against disconnected state:
```rust
if !self.comm.is_connected() {
    return "Not connected to NeboAI. The comm plugin is not active.";
}
```

---

## 25. Reconnection & Resilience

### Automatic Reconnect Loop

The server spawns a persistent reconnect task after initial NeboAI activation:

```rust
loop {
    tokio::select! {
        _ = reconnect_state.comm_manager.wait_disconnect() => {
            info!("neboai: disconnect notification received, will reconnect");
        }
        // Also handles system sleep detection
    }
    // Backoff, then:
    match codes::activate_neboai(&reconnect_state).await {
        Ok(()) => { /* persist rotated token */ }
        Err(e) => { /* log, retry on next iteration */ }
    }
}
```

### System Sleep Detection

The reconnect loop also detects system sleep/wake cycles. When the OS suspends
and resumes, TCP connections may be silently broken. The read loop's 60-second
timeout detects this, but the reconnect loop also has independent sleep detection
that forces a reconnect.

### Stale Token Recovery

If `connect()` fails with "stale token" or "auth failed":
1. Attempt OAuth token refresh via `refresh_neboai_token()`
2. If successful, retry connect with the fresh token
3. Persist the new token to the auth profile

### Token Cache File

The rotated token from AUTH_OK is persisted to a cache file
(`~/.nebo/data/neboai_token.cache`) immediately. This survives hot-reload
or process crashes where the DB persist may not have completed:

```rust
if let Ok(dir) = config::data_dir() {
    let cache_path = dir.join("neboai_token.cache");
    std::fs::write(&cache_path, &auth_result.token);
}
```

On startup, the cache file is checked before the DB token, using whichever
is newer.

---

## 26. Token Rotation & Security

### JWT-Based Authentication

NeboAI uses bot JWTs for authentication. Each bot has a unique `bot_id` and
receives a JWT during onboarding (code redemption).

### Token Rotation

The gateway may issue a rotated JWT in the AUTH_OK response. When present:

1. The NeboAIPlugin stores it in `rotated_token` (in-memory)
2. Writes it to `neboai_token.cache` (disk, immediate)
3. The server's reconnect loop persists it to the auth profile DB

The next connect attempt uses the rotated token.

### Credential Flow

```
User enters connection code
  |
  v
redeem_code() [unauthenticated]
  |
  +-- POST /api/v1/bots/connect/redeem
  |    Returns: { connection_token, owner_email, ... }
  v
Store as auth_profile (provider="neboai")
  |
  v
activate_neboai()
  |
  +-- Build config: { gateway, api_server, bot_id, token }
  +-- comm_manager.set_active("neboai")
  +-- comm_manager.connect_active(config)
  |     |
  |     +-- NeboAIPlugin::connect()
  |     |     +-- WebSocket dial
  |     |     +-- CONNECT frame { bot_id, token }
  |     |     +-- Wait AUTH_OK
  |     |     +-- Store rotated token
  |     |     +-- Spawn read/write/join loops
  |     |     +-- Auto-join default streams
  |     v
  +-- Take rotated token, persist to DB
```

### OAuth Integration

For web-based login (NeboAI OAuth), the server implements PKCE flow:
- `oauth_start()` generates a code verifier, challenge, and state
- Redirects to NeboAI authorization endpoint
- `oauth_callback()` exchanges the authorization code for tokens
- Stores tokens as auth_profile with `refresh_token` in metadata

---

## 27. DevLog — Traffic Inspection

**File:** `crates/comm/src/devlog.rs`

Human-readable traffic log written to `~/.nebo/data/logs/neboai.log`.
Designed for `tail -f` during development.

### Log Format

```
HH:MM:SS.mmm ── EVENT_TYPE details
HH:MM:SS.mmm → OUT stream=agent_space conv=abc12345
                        "Hello, I can help with..."
HH:MM:SS.mmm ← IN  stream=dm from=bot-456 conv=def67890
                        "Can you summarize this document?"
HH:MM:SS.mmm !! ERROR read error: connection reset
```

### Event Types

| Prefix | Meaning                          |
|--------|----------------------------------|
| `──`   | Lifecycle event (CONNECT, AUTH_OK, DISCONNECT) |
| `→`    | Outbound (JOIN request, SEND)    |
| `←`    | Inbound (JOIN result, delivery)  |
| `!!`   | Error                            |

Content is truncated to 200 characters for readability. UUIDs are shortened
to 8 characters.

---

## 28. Error Handling

### CommError Variants

```rust
pub enum CommError {
    NotConnected,                 // Plugin not connected
    NoActivePlugin,               // No plugin set as active
    PluginNotFound(String),       // Named plugin not in registry
    Other(String),                // Catch-all with message
}
```

### FrameError Variants

```rust
pub enum FrameError {
    BadVersion { got: u8 },       // Protocol version mismatch
    PayloadTooLarge { len: u32 }, // Exceeds 32 KB limit
    ShortRead { need, got },      // Incomplete frame data
    Io(std::io::Error),           // Underlying I/O error
}
```

### Error Propagation

- Network errors in `NeboAIPlugin::connect()` bubble up as `CommError::Other`
- Frame decode errors in the read loop are logged and the frame is skipped (not fatal)
- Send channel errors indicate the write loop has exited (connection dead)
- REST API errors include HTTP status code and response body in the error message

---

## 29. Configuration

### NeboAI Config (from `etc/nebo.yaml`)

```yaml
neboai:
  api_url: "https://api.neboai.com"
  comms_url: "wss://comms.neboai.com/ws"
  janus_url: "https://janus.neboai.com"
```

### Environment Variable Overrides

| Variable              | Overrides                |
|-----------------------|--------------------------|
| `NEBOAI_API_URL`    | `neboai.api_url`       |
| `NEBOAI_COMMS_URL`  | `neboai.comms_url`     |
| `NEBOAI_JANUS_URL`  | `neboai.janus_url`     |

### Plugin Connect Config Map

The `connect()` method receives a `HashMap<String, String>`:

| Key           | Description                              | Required |
|---------------|------------------------------------------|----------|
| `gateway`     | WebSocket URL (wss://comms.neboai.com) | Yes      |
| `bot_id`      | Bot identifier                           | Yes      |
| `token`       | JWT authentication token                 | Yes      |
| `api_server`  | REST API base URL                        | No (derived from gateway) |
| `data_dir`    | Data directory for devlog + token cache  | No       |

### API Client Configuration

The REST client uses:
- 5-second connect timeout
- 15-second total request timeout
- Bearer token authentication
- Automatic `Content-Type: application/json`

---

## 30. File Manifest

| File                       | Lines | Purpose                                          |
|----------------------------|-------|--------------------------------------------------|
| `crates/comm/src/lib.rs`   | ~118  | Module decl, CommPlugin + ChannelProvider traits  |
| `crates/comm/src/types.rs` | ~190  | CommMessage, CommError, AgentCard, TaskStatus     |
| `crates/comm/src/manager.rs` | ~261 | PluginManager: registry, routing, shutdown       |
| `crates/comm/src/neboai.rs` | ~700+ | NeboAIPlugin: WS connection, loops, framing  |
| `crates/comm/src/loopback.rs` | ~196 | LoopbackPlugin: in-memory test plugin           |
| `crates/comm/src/frame.rs` | ~289  | 47-byte binary header codec + tests              |
| `crates/comm/src/wire.rs`  | ~224  | JSON payload types for binary protocol           |
| `crates/comm/src/compress.rs` | ~53 | Zstd compression (threshold: 1 KB)              |
| `crates/comm/src/ulid.rs`  | ~113  | Monotonic ULID generator                         |
| `crates/comm/src/dedup.rs` | ~108  | Sliding-window deduplication (1000/5min)         |
| `crates/comm/src/devlog.rs`| ~128  | Traffic log for `tail -f`                        |
| `crates/comm/src/api.rs`   | ~1049 | NeboAI REST API client                         |
| `crates/comm/src/api_types.rs` | ~846 | DTOs for REST API responses                   |

### Cross-Crate Integration Points

| File                                     | Usage                                   |
|------------------------------------------|-----------------------------------------|
| `crates/server/src/state.rs`             | `comm_manager`, `channel_providers`     |
| `crates/server/src/lib.rs`               | Plugin registration, handler wiring, reconnect loop, `handle_comm_message()` |
| `crates/server/src/chat_dispatch.rs`     | `CommReplyConfig`, `send_comm_response()` |
| `crates/server/src/codes.rs`             | `activate_neboai()`, `redeem_nebo_code()` |
| `crates/server/src/handlers/neboai.rs` | OAuth flow, profile storage             |
| `crates/server/src/handlers/agents.rs`   | Agent registration in loops             |
| `crates/tools/src/loop_tool.rs`          | Agent-facing NeboAI tool              |

---

## Appendix: CommMessage Struct

```rust
pub struct CommMessage {
    pub id: String,                              // Message ID (ULID string)
    pub from: String,                            // Sender ID
    pub to: String,                              // Recipient ID
    pub topic: String,                           // Stream name (dm, chat, agent_space, etc.)
    pub conversation_id: String,                 // NeboAI conversation UUID
    pub msg_type: CommMessageType,               // Message, Stream, Mention, ToolActivity, etc.
    pub content: String,                         // JSON or plain text content
    pub metadata: HashMap<String, String>,       // agent_id, agent_slug, senderName, etc.
    pub timestamp: i64,                          // ULID-derived millisecond timestamp
    pub human_injected: bool,                    // True if injected by human (not bot)
    pub human_id: Option<String>,                // Human user ID if applicable

    // A2A task lifecycle fields
    pub task_id: Option<String>,
    pub correlation_id: Option<String>,
    pub task_status: Option<TaskStatus>,          // Submitted/Working/Completed/Failed/Canceled/InputRequired
    pub artifacts: Vec<TaskArtifact>,
    pub error: Option<String>,
}
```

## Appendix: AgentCard Struct (A2A Discovery)

```rust
pub struct AgentCard {
    pub name: String,
    pub description: Option<String>,
    pub url: Option<String>,
    pub preferred_transport: Option<String>,
    pub protocol_version: Option<String>,
    pub default_input_modes: Vec<String>,
    pub default_output_modes: Vec<String>,
    pub capabilities: HashMap<String, serde_json::Value>,
    pub skills: Vec<AgentCardSkill>,
    pub provider: Option<AgentCardProvider>,
}
```

## Appendix: TaskStatus Lifecycle (A2A Spec)

```
Submitted --> Working --> Completed
                |              |
                +--> Failed    +--> (done)
                |
                +--> Canceled
                |
                +--> InputRequired --> (re-submit)
```

## Appendix: Channel-Plugin Bridges (Slack/Discord-style outbound)

This SME covers the NeboLoop comm framework. The *other* outbound comm path —
channel plugins like Slack — is a separate mechanism, documented for plugin
authors in `docs/publishers-guide/channel-plugins.md`. Runtime facts (verified
against source, 2026-06-11):

- A plugin declares a `channel` block in plugin.json (`command: "bridge --listen"`,
  `shared`, `restartDelaySecs`). Nebo runs one resident bridge process per
  enabled (agent, plugin) pair, registered globally as `{agent_id}:{plugin_slug}`
  (`crates/tools/src/channel_bridge.rs`).
- Outbound ops are intercepted by the `plugin` tool: when a command's first
  verb is `reply`, `post`, `upload`, or `dm`, it is routed to the agent's bridge
  over stdin NDJSON instead of spawning the CLI
  (`crates/tools/src/plugin_tool.rs` — verb match + `route_through_bridge`).
- **No CLI fallback**: if no bridge is registered for the calling agent, the op
  fails with a structured error pointing at channel settings.
- **Agent context required**: the caller's agent id is parsed from the session
  key (`agent:<id>:…`); runs without agent context (system tasks, cron without
  a bound agent) cannot perform channel ops.
- Inbound messages stream from the bridge's stdout into the channel loop;
  replies route back through the same bridge (`reply` op). The loop-side
  CommReplyConfig machinery in this document is not involved.
