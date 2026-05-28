# NeboAI Connection & Hub Chat — SME Reference

This is a research/knowledge task, not an implementation plan. The output is a comprehensive SME reference for how the NeboAI connection works and how chat flows through the hub.

---

## 1. Architecture Overview

The local Nebo desktop app connects to the NeboAI cloud gateway via a **binary-framed WebSocket** protocol. The connection is managed by a plugin system (`CommPlugin` trait) with the `NeboAIPlugin` as the production implementation. All inbound messages flow through a **single unified chat pipeline** (`chat_dispatch::run_chat()`), the same one used for local WebSocket chat.

**Key crates:**
- `crates/comm/` — Plugin trait, NeboAI WebSocket client, REST API, wire protocol
- `crates/server/src/chat_dispatch.rs` — Unified chat dispatch pipeline
- `crates/server/src/lib.rs` — Message handler registration, routing logic
- `crates/server/src/codes.rs` — Connection activation, agent registration, reconciliation

---

## 2. Binary Wire Protocol

### 47-byte Header (`crates/comm/src/frame.rs`)

```
[0]     proto_version   u8          (always 1)
[1]     frame_type      u8          (1-13)
[2]     flags           u8          (bit0=compressed, bit1=encrypted, bit2=ephemeral)
[3-6]   payload_len     u32         (big-endian, max 32KB)
[7-22]  msg_id          16 bytes    (ULID, monotonic)
[23-38] conversation_id 16 bytes    (UUID)
[39-46] seq             u64         (sequence number)
```

### Frame Types

| Type | ID | Direction | Purpose |
|------|-----|-----------|---------|
| CONNECT | 1 | C→S | Bot auth with `{bot_id, token}` |
| AUTH_OK | 2 | S→C | Accepted + rotated JWT |
| AUTH_FAIL | 3 | S→C | Rejected with reason |
| JOIN_CONVERSATION | 4 | Both | Join stream/channel/agent_space (C→S), Join result (S→C) |
| LEAVE_CONVERSATION | 5 | C→S | Leave a conversation |
| SEND_MESSAGE | 6 | C→S | Send message on conversation |
| MESSAGE_DELIVERY | 7 | S→C | Incoming message delivery |
| ACK | 8 | C→S | Acknowledge up to seq |
| PRESENCE | 9 | S→C | Online/offline |
| TYPING | 10 | Both | Typing indicator |
| SLOW_DOWN | 11 | S→C | Backpressure/rate limit |
| REPLAY | 12 | S→C | Historical message replay |
| CLOSE | 13 | C→S | Graceful close |

### JSON Payloads (`crates/comm/src/wire.rs`, all camelCase)

- **ConnectPayload**: `{botId, token}` — sent with CONNECT frame
- **AuthResultPayload**: `{ok, reason, botId, plan, token}` — rotated JWT in `token` field
- **SendPayload**: `{conversationId, stream, content}` — wraps content as JSON value
- **DeliveryPayload**: `{senderId, stream, content, agentId, agentSlug, sourceChannelId}` — agent fields for @mentions/agent spaces
- **JoinPayload**: `{conversationId | botId+stream | channelId, lastAckedSeq}` — one of three join modes
- **JoinResultPayload**: `{conversationId, botId, stream, channelId, channelName, loopId, peerId, peerType, agentId, agentSlug}` — type detected by which fields are set
- **AckPayload**: `{conversationId, ackedSeq}`
- **ReplayPayload**: `{conversationId, fromSeq, toSeq, messageCount}`
- **Attachment**: `{fileId, filename, mimeType, size, url, thumbnailUrl?, width?, height?, duration?}` — file/image/video metadata embedded in `content.attachments[]`

---

## 3. Connection Lifecycle

### Initial Connection (`neboai.rs:288-500`)

```
0. Tear down existing connection (cancel old tasks, clear stale state)
   - If is_connected → disconnect()
   - Else → cancel any stale token, clear send_tx
1. WebSocket dial → gateway URL (e.g. wss://comms.neboai.com/ws)
2. Send CONNECT frame {bot_id, token}
3. Wait for AUTH_OK/AUTH_FAIL (10s timeout)
4. Extract rotated JWT from AUTH_OK → store in memory + persist to <data_dir>/neboai_token.cache
5. Reset ConvMaps (fresh for new connection)
6. Spawn 3 background tasks:
   - Read loop: decode frames → dedup → dispatch to handler (120s read timeout)
   - Write loop: drain send queue + 30s keepalive pings
   - Join processor: update ConvMaps from join results
7. Auto-join 5 bot streams: dm, installs, chat, account, voice
```

The token cache file at `<data_dir>/neboai_token.cache` is persisted immediately on AUTH_OK to survive hot-reload kills (SIGKILL) that prevent the caller from writing to DB.

### Activation (`codes.rs:1241+`)

`activate_neboai()` is the canonical connection function:
1. Guard: if already connected, return `Ok(())`
2. Read `bot_id` from config
3. Read NeboAI auth profile from DB (provider="neboai")
4. Get JWT token: check cached file first (`<data_dir>/neboai_token.cache`), fall back to DB token
5. Build config map: `{gateway, api_server, bot_id, token, data_dir}`
6. Set "neboai" as active plugin → `comm_manager.set_active("neboai")`
7. Connect → `comm_manager.connect_active(config)`
8. On stale token error → OAuth refresh (`refresh_neboai_token`) → retry connect
9. Persist rotated JWT from AUTH_OK to DB auth_profiles
10. Broadcast `settings_updated` to frontend
11. Spawn background reconciliation: `reconcile_agents()` + `sync_bot_identity()`

### Keepalive & Timeouts

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| Ping interval | 30s | Standard WebSocket keepalive; balances bandwidth vs liveness detection |
| Read timeout | 120s | 4× ping interval; allows 4 missed pings before treating as dead |
| Sleep drift threshold | 60s | 2× ping interval; detects system sleep/wake and forces reconnect |

Ping/pong frames are logged to devlog (dev builds only) for connection health visibility.

### Auto-connect & Reconnect (`lib.rs:1626-1700`)

- **Startup**: If NeboAI enabled + credentials exist → `activate_neboai()` in background task
- **Reconnect watcher**: Polls every `backoff_secs` (starts 30s, initial 60s delay)
  - If connected → reset backoff to 30s
  - If disconnected → attempt reconnect, on failure double backoff (max 600s)
  - On successful reconnect → persist rotated token to DB

### Token Rotation

- Gateway rotates JWT on every successful AUTH_OK
- Token stored in memory (`rotated_token` RwLock) AND persisted to cache file immediately
- Caller retrieves via `plugin.take_rotated_token()` and persists to auth_profiles DB
- Prevents token expiry across reconnects and hot-reload restarts

---

## 4. Conversation Maps (`neboai.rs:815-826`)

In-memory maps rebuilt on each connection. Managed by the join processor task.

```rust
ConvMaps {
    conv_by_key:         "botId:stream"   → conversation_id    // 5 bot streams
    pending_joins:       Vec<String>                            // FIFO for bot stream join matching
    channel_convs:       channel_id        → conversation_id
    channel_by_conv:     conversation_id   → channel_id
    channel_meta:        channel_id        → ChannelMeta{id, name, loop_id}
    dm_convs:            conversation_id   → DmPeer{peer_id, type, loop_id}
    dm_by_peer:          peer_id           → conversation_id
    agent_space_convs:   conversation_id   → AgentSpaceMeta{agent_id, slug, loop_id}
    agent_space_by_slug: agent_slug        → conversation_id
    agent_space_by_id:   agent_id          → conversation_id
}
```

**Join Result Classification** (read loop, `neboai.rs:1085-1131`):
- If `agent_id` set → `JoinUpdate::AgentSpace`
- Else if `peer_id` set → `JoinUpdate::Dm` (legacy)
- Else if `channel_id` set → `JoinUpdate::Channel`
- Else → `JoinUpdate::BotStream` (pops from `pending_joins` FIFO)

---

## 5. Inbound Message Flow

### Read Loop (`neboai.rs:874+`)

1. Receive WS binary message
2. Decode 47-byte header + payload
3. Decompress if compressed flag set (zstd)
4. Dedup check (sliding window: 1000 entries, 5min TTL)
5. For `TYPE_MESSAGE_DELIVERY`:
   - Parse `DeliveryPayload`
   - Build `CommMessage` with metadata (agent_id, agent_slug, source_channel_id)
   - Extract `attachments[]` from `delivery.content` JSON → `Vec<wire::Attachment>`
   - Set `topic` = `delivery.stream` (e.g., "chat", "dm", "agent_space", "installs")
   - Call registered message handler callback (warns + logs if handler is None)
6. For `TYPE_JOIN_CONVERSATION`: route to join processor

### Message Handler (`lib.rs:2272+`)

Single handler registration at startup — message handler is set once during `start_server()`, wrapping each incoming message in a spawned async task that calls `handle_comm_message()`.

`handle_comm_message()` routes by `msg.topic`:

| Topic | Route | Details |
|-------|-------|---------|
| `"account"` | Token/plan update | Parse tokenRefresh → persist JWT to DB, update plan_tier, broadcast `plan_changed` |
| `"installs"` | `napp_registry.handle_install_event()` | Parse as InstallEvent, broadcast tool_event/tool_error, return immediately |
| `"agent_space"` | Unified chat pipeline | Resolve agent from agent_slug, session key = `neboai:agent_space:{slug}:{conv_id}` |
| `"chat"` or `"dm"` | Unified chat pipeline | Agent-space reroute check → optional @mention routing, session key = `neboai:{topic}:{conv_id}` |
| Other | Event bus + broadcast | Emit as `neboai.{topic}`, broadcast raw to frontend |

### Account Messages (`lib.rs:2282-2318`)

```
1. Parse JSON content for type="tokenRefresh"
2. Extract new JWT token and plan tier
3. Persist JWT to auth_profiles DB table
4. Update in-memory plan_tier atomic
5. Broadcast "plan_changed" event to frontend
6. Return (no chat dispatch)
```

### Agent Space Messages (`lib.rs:2366-2500`)

```
1. Extract text from JSON content (msg.content.text or msg.content.content or raw)
2. Get agent_slug from msg.metadata["agent_slug"]
3. Resolve agent_id from slug (scan agent_registry for matching name)
4. Build session_key: "neboai:agent_space:{slug}:{conversation_id}"
5. Pre-create chat record with "Agent: {agent_name}" title
6. Send desktop notification
7. Resolve entity config for ("channel", "agent_space")
8. Process attachments via process_comm_attachments():
   - Image attachments → download via API, base64-encode → Vec<ImageContent> for vision
   - Non-image attachments → append "[Attached: filename (size)]" to prompt text
9. Build ChatConfig {origin: Comm, lane: COMM, agent_id, images, comm_reply: {topic: "agent_space", conv_id}}
10. Dispatch to run_chat()
11. Emit event: "neboai.agent_space.{slug}" → triggers agent event subscriptions
```

### Chat/DM Messages (`lib.rs:2523+`)

```
1. Agent-space reroute check: call comm_manager.agent_slug_for_conv(conversation_id)
   - If conversation belongs to an agent space (gateway sends stream=dm for these):
     Reroute to agent space path with comm_reply topic=msg.topic (preserves "dm")
     Same flow as agent_space above, but reply goes back on original topic
2. Extract text
3. Send desktop notification
4. Build session_key: "neboai:{topic}:{conversation_id}"
5. Resolve entity config for ("channel", topic)
6. Check @mention: if agent_slug in metadata → resolve agent_id
7. If @mentioned: pre-create chat with "@{slug} (channel)" title
8. Process attachments via process_comm_attachments() (same as agent_space path)
9. Build ChatConfig {origin: Comm, lane: COMM, agent_id (empty if no @mention), images, comm_reply}
10. Dispatch to run_chat()
11. Emit event: "neboai.{topic}" → triggers agent event subscriptions
```

---

## 6. Unified Chat Pipeline (`chat_dispatch.rs:56-537`)

**ONE function for all chat**: `run_chat(state, config)`

### ChatConfig Decorators

```rust
ChatConfig {
    session_key:   String,           // Unique conversation ID
    prompt:        String,           // User message text (may include "[Attached: ...]" for non-image files)
    system:        String,           // System prompt override
    user_id:       String,           // User identity
    channel:       String,           // "neboai" for comm messages
    origin:        Origin::Comm,     // Identifies source (Comm vs Ws vs Heartbeat)
    agent_id:      String,           // Route to specific agent (empty = main agent)
    cancel_token:  CancellationToken,
    lane:          String,           // "comm" for NeboAI (not "main")
    comm_reply:    Option<CommReplyConfig>, // Where to send response back
    entity_config: Option<ResolvedEntityConfig>, // Permissions, model, personality
    images:        Vec<ImageContent>, // Populated from comm attachment downloads (image/* types)
    entity_name:   String,           // Display name (agent name or "Nebo"), used in RunRegistry
}
```

### Execution Flow

```
1. Register in RunRegistry for visibility and external cancellation
2. Broadcast "chat_created" to frontend
3. Enqueue lane task on specified lane (e.g., COMM)
4. Lane task spawns agent runner.run(RunRequest)
5. Stream events from runner:
   - Text → coalesce 50ms, broadcast "chat_stream" to frontend
          → if comm_reply set: coalesce 500ms, send Stream chunks to NeboAI
   - Thinking → broadcast "thinking"
   - ToolCall → broadcast "tool_start"
   - ToolResult → broadcast "tool_result"
   - Error → broadcast "chat_error"
   - Usage → broadcast "usage"
   - ApprovalRequest → broadcast "approval_request"
   - SubagentProgress → broadcast "subagent_progress"
   - SubagentComplete → broadcast "subagent_complete"
   - Done → exit loop
6. Flush remaining text buffer to frontend
7. Flush remaining comm_buffer to NeboAI (Stream type)
8. If no stream chunks were sent (short response) → send full_response as single Message
9. Broadcast "chat_complete"
```

### Comm Response Streaming

Responses to NeboAI are **streamed in real-time** (not batched). Two coalescing windows operate in parallel:
- **Frontend**: 50ms coalesce → `chat_stream` WebSocket event
- **NeboAI comm**: 500ms coalesce → `CommMessageType::Stream` chunks via `comm_manager.send()`

After the agent completes:
- Remaining `comm_buffer` is flushed as a final Stream chunk
- If no chunks were streamed at all (response completed before first 500ms flush), the full response is sent as a single `CommMessageType::Message`
- This prevents duplicate messages: once streaming has started, no final "complete" message is sent

### CommReplyConfig (`chat_dispatch.rs:45-49`)

```rust
CommReplyConfig {
    provider: String,        // "neboai", or future: "slack", "discord"
    topic: String,           // "agent_space", "chat", or "dm"
    conversation_id: String, // NeboAI conversation ID
}
```

---

## 7. Outbound Message Flow

### From Chat Reply (`chat_dispatch.rs:260-620`)

```
During execution (500ms coalesce):
  text chunk → CommMessage {
    topic: reply_config.topic,
    conversation_id: reply_config.conversation_id,
    msg_type: Stream,
    content: chunk_text,
    metadata: {senderName: agent_display_name},
  } → comm_manager.send()

After completion (only if no chunks were streamed):
  full_response → CommMessage {
    topic: reply_config.topic,
    conversation_id: reply_config.conversation_id,
    msg_type: Message,
    content: full_response,
    metadata: {senderName: agent_display_name},
  } → comm_manager.send()
```

### NeboAIPlugin.send() (`neboai.rs:553-625`)

```
1. Check connected flag → return NotConnected if false
2. Resolve conversation_id:
   - If msg.conversation_id set → use it
   - Else if msg.to set → lookup agent_space_by_slug, then dm_by_peer
   - Else → error
3. Determine stream:
   - If msg.topic set → use it
   - Else if resolved via agent_space → "agent_space"
   - Else → "dm"
4. Build content: {"text": msg.content} merged with metadata fields
5. If msg.attachments is non-empty → add content["attachments"] with attachment metadata array
6. Build SendPayload → encode frame → queue to write loop
```

### From Agent Tools (loop tool)

Agents can also send messages proactively using the "loop" tool:
- `dm.send(to, text)` — DM another agent
- `channel.send(channel_id, text)` — Post to channel
- Requires "loop" permission enabled

---

## 8. Lane System

| Lane | Purpose | Concurrency |
|------|---------|-------------|
| `main` | User chat from local WebSocket | Unlimited |
| **`comm`** | **Inbound NeboAI messages** | **Unlimited** |
| `events` | Event-triggered workflows | Unlimited |
| `heartbeat` | Agent proactive ticks | Unlimited |
| `subagent` | Subagent execution | Unlimited |
| `nested` | Nested agent calls | Unlimited |
| `dev` | Developer/debug lane | Unlimited |
| `desktop` | Screen/mouse automation | **1 (serialized)** |

COMM lane prevents NeboAI message flood from blocking local user chat on MAIN lane. DESKTOP is the only serialized lane — all others have unlimited concurrency.

---

## 9. Agent Registration

When an agent is installed/activated, the bot registers an agent in the owner's personal loop (`codes.rs:1576-1601`):

```
1. Get API client from stored credentials
2. List bot's loops → pick first (personal loop)
3. POST /api/v1/loops/{loopId}/agents {agent_name, agent_slug, description}
4. Gateway auto-creates agent_space conversation + subscribes bot to it
```

Deregistration on agent removal: `DELETE /api/v1/loops/{loopId}/agents/{agentSlug}`

### Reconciliation (`codes.rs:1384-1457`)

On every successful connection, `reconcile_agents()` syncs local ↔ remote:
- Fetches remote agents from NeboAI
- Deregisters agents that are truly **deleted** locally (not just disabled)
- Registers local agents missing from remote
- **Disabled** agents are synced with status `"paused"` — they are NOT deregistered

---

## 10. REST API Client (`crates/comm/src/api.rs`)

`NeboAIApi` provides authenticated REST endpoints alongside the WebSocket connection:

| Category | Key Endpoints |
|----------|--------------|
| **Products** | `list_products(type, query, category, page)` — unified product listing |
| **Apps** | `list_apps`, `get_app`, `app_reviews`, `install_app`, `uninstall_app`, `similar_apps`, `featured_apps` |
| **Skills** | `list_skills`, `get_skill`, `top_skills`, `skill_reviews`, `submit_review`, `skill_media`, `skill_feedback` |
| **Marketplace** | `list_categories`, `get_screenshots` |
| **Code Redemption** | `redeem_code(code, botIds, platform)` — universal (SKIL/WORK/AGNT/PLUG codes) |
| **Downloads** | `download_napp(url)` — sealed .napp archives |
| **Loops** | `join_loop(code)`, `list_bot_loops`, `get_loop`, `list_loop_members` |
| **Channels** | `list_bot_channels`, `list_channel_messages`, `list_channel_members` |
| **Agents** | `register_agent`, `list_agents`, `deregister_agent` |
| **Billing** | `billing_prices`, `billing_subscription`, `billing_checkout`, `billing_subscribe`, `billing_portal`, `billing_setup_intent`, `billing_cancel`, `invoices`, `payment_methods` |
| **Bot Identity** | `update_bot_identity(name, role)` |
| **Publishing** | `publish_skill`, `publish_agent`, `submit_for_review` |
| **Plugins** | `get_plugin(slug, platform)`, `download_plugin_binary(url)` |
| **Workflows** | `list_workflows`, `uninstall_workflow` |
| **Files** | `upload_file(filename, mime_type, data)` → `Attachment`, `download_file(file_id)` → raw bytes |
| **Referral** | `get_referral_code` |
| **Unauthenticated** | `redeem_connect_code(code)` — for initial bot activation |

API server URL comes from config (`state.config.neboai.api_url`), defaulting to `https://api.neboai.com`.

---

## 11. Event Bus Integration

Every incoming NeboAI message emits an event for agent triggers:

| Message Topic | Event Source | Payload |
|---------------|-------------|---------|
| agent_space | `neboai.agent_space.{slug}` | from, content, conversation_id, agent_slug |
| chat | `neboai.chat` | from, content, conversation_id |
| dm | `neboai.dm` | from, content, conversation_id |
| other | `neboai.{topic}` | from, content, topic |

All events include `origin: "neboai"` and a UNIX timestamp.

Agents subscribe to these event sources in their configuration, triggering automated responses.

---

## 12. CommMessage Type (`crates/comm/src/types.rs`)

```rust
CommMessage {
    id: String,
    from: String,
    to: String,
    topic: String,
    conversation_id: String,
    msg_type: CommMessageType,   // Message, Stream, Mention, Proposal, Command, Info, Task, TaskResult, TaskStatus, LoopChannel
    content: String,
    metadata: HashMap<String, String>,
    timestamp: i64,
    human_injected: bool,
    human_id: Option<String>,

    // A2A task lifecycle fields
    task_id: Option<String>,
    correlation_id: Option<String>,
    task_status: Option<TaskStatus>,   // Submitted, Working, Completed, Failed, Canceled, InputRequired
    artifacts: Vec<TaskArtifact>,
    error: Option<String>,

    // File/image/video attachments
    attachments: Vec<wire::Attachment>,  // #[serde(default, skip_serializing_if = "Vec::is_empty")]
}
```

The `Stream` message type is used for real-time response streaming (500ms coalesced chunks). `Task`/`TaskResult`/`TaskStatus` types support the A2A protocol for inter-agent task delegation. `attachments` carries file/image/video metadata — actual file bytes are uploaded/downloaded via the REST API, only metadata (~200 bytes each) travels over the WebSocket.

---

## 12b. File/Image/Video Attachments

### Upload-Then-Reference Pattern

Attachments use an **upload-then-reference** pattern. File bytes are uploaded via REST, returning an `Attachment` metadata struct. Only the metadata is embedded in the WebSocket `content.attachments[]` JSON — file bytes never travel over the 32KB wire protocol.

### Attachment Struct (`crates/comm/src/wire.rs`)

```rust
Attachment {
    file_id: String,         // Server-assigned file ID
    filename: String,        // Original filename
    mime_type: String,       // MIME type (e.g., "image/png", "video/mp4", "application/pdf")
    size: u64,               // File size in bytes
    url: String,             // Download URL (may be pre-signed)
    thumbnail_url: Option,   // Resized image / video first frame
    width: Option<u32>,      // Image/video dimensions
    height: Option<u32>,
    duration: Option<f64>,   // Video duration in seconds
}
```

### REST Endpoints (`crates/comm/src/api.rs`)

- `upload_file(filename, mime_type, data)` → `POST /api/v1/files/upload` (multipart/form-data) → returns `Attachment`
- `download_file(file_id)` → `GET /api/v1/files/{file_id}` → raw bytes

Upload uses a separate reqwest client with 120s timeout (vs 15s default) for large files.

### Wire Protocol Integration

Attachments piggyback on the existing freeform `content: Value` field in `SendPayload` and `DeliveryPayload`:

- **Outbound** (`neboai.rs:send()`): If `msg.attachments` is non-empty, `content["attachments"]` is set to the serialized attachment metadata array alongside `content["text"]`.
- **Inbound** (`neboai.rs:read_loop`): `delivery.content.get("attachments")` is parsed into `Vec<wire::Attachment>` and stored on `CommMessage.attachments`.

No wire protocol header, frame type, or version changes — fully backward compatible.

### Vision Integration (`lib.rs:process_comm_attachments()`)

When a comm message with attachments reaches `handle_comm_message()`:

1. **Image attachments** (`mime_type.starts_with("image/")`) are downloaded via `NeboAIApi.download_file()`, base64-encoded, and added to `ChatConfig.images` as `ai::ImageContent`. The AI provider receives these as vision input.
2. **Non-image attachments** (PDF, video, etc.) are appended to the prompt as text descriptions: `[Attached: report.pdf (245 KB)]`.
3. Errors (download failure, missing API client) are logged and skipped — the message is still processed with whatever attachments succeeded.

### Local Server Upload Proxy (`crates/server/src/handlers/files.rs`)

The frontend uploads through the local server (`POST /api/v1/files/upload`) which proxies to NeboAI. This avoids CORS issues and keeps the NeboAI token off the browser.

### Frontend Flow

1. User attaches files in the chat composer
2. Files are uploaded via `app/src/lib/api/upload.ts` (XHR for progress tracking) → returns `UploadedAttachment[]`
3. `controller.send(text, { attachments })` includes attachment metadata in the WebSocket payload
4. `ChatPane.svelte` renders attachments inline: images as thumbnails, videos with controls, files as download pills

---

## 13. Key Implementation Details

1. **Token rotation**: Every AUTH_OK includes a new JWT. Persisted immediately to cache file + later to DB via `take_rotated_token()`.
2. **Pending joins FIFO**: Bot stream JoinResult doesn't include stream name. Read loop sends empty key; join processor pops from `pending_joins` queue (FIFO order matches join order).
3. **Dedup per connection**: Fresh `DedupWindow` (1000 entries, 5min TTL) created each connect.
4. **Compression**: Zstd for payloads > 1KB, flagged in header byte.
5. **Session isolation**: Each NeboAI conversation gets unique `session_key` → separate chat history.
6. **Response streaming**: Responses are streamed in real-time at 500ms intervals via `CommMessageType::Stream`. Short responses (< 500ms total) are sent as a single `CommMessageType::Message`. No duplicate final message when streaming has started.
7. **ConvMaps thread safety**: `Arc<RwLock<ConvMaps>>` shared across read loop, join processor, and public query methods.
8. **Graceful shutdown**: Single `CancellationToken` coordinates all 3 background tasks (read, write, join processor). Note: process kill (SIGKILL / cargo watch) bypasses this — spawned tasks are dropped without running cleanup.
9. **Entity config**: Per-topic permissions resolved via `entity_config::resolve_for_chat("channel", topic)`.
10. **@mention routing**: `agent_slug` in DeliveryPayload metadata → resolve to local agent_id → route to that agent's persona.
11. **Handler field**: Message handler is stored in a separate `std::sync::RwLock` (not inside the async `Inner` RwLock) so `set_message_handler()` always succeeds synchronously. The handler is cloned at connect time and passed to the read loop as a local variable.
12. **DM→agent_space reroute**: Gateway sometimes sends `stream=dm` for agent space conversations. `handle_comm_message()` checks `agent_slug_for_conv()` and reroutes to the agent space path if the conversation belongs to an agent.
13. **Token cache file**: `<data_dir>/neboai_token.cache` survives hot-reload kills. `activate_neboai()` reads this first, falling back to DB token.
14. **RunRegistry**: All chat runs auto-register in the global RunRegistry for visibility, progress tracking, and external cancellation.
15. **Dev-only devlog**: DevLog initialization is gated behind `#[cfg(debug_assertions)]` — no traffic logging in release builds. Writes to `<data_dir>/logs/neboai.log`. Includes all frame I/O, ping/pong, JOINs, errors, and session separators.
16. **Sleep drift detection**: After each ping, elapsed time since last ping is checked. If > 60s (2× the 30s interval), the connection is assumed stale (e.g., laptop lid close/open) and force-reconnected.
17. **Attachment upload-then-reference**: Files are uploaded via REST (`POST /api/v1/files/upload` multipart), returning an `Attachment` metadata struct. Only the metadata (~200 bytes/attachment) is embedded in `content.attachments[]` inside the WebSocket payload — file bytes never travel over the 32KB-limited wire protocol. The local server proxies uploads through `POST /api/v1/files/upload` (handlers/files.rs) to the NeboAI API.
18. **Image vision from attachments**: `process_comm_attachments()` in `lib.rs` downloads image attachments (`mime_type.starts_with("image/")`) via `NeboAIApi.download_file()`, base64-encodes them, and populates `ChatConfig.images` for the AI provider's vision pipeline. Non-image attachments (PDF, video, etc.) are appended as text descriptions to the prompt.
19. **Attachment backward compatibility**: `CommMessage.attachments` uses `#[serde(default, skip_serializing_if = "Vec::is_empty")]` — old clients that don't understand attachments still see the `text` field, and old messages without attachments deserialize with an empty vec.

---

## 13b. Known Issues & Fixes Applied

### Fixed: `try_write()` race in `set_message_handler` (neboai.rs)
**Was:** Handler stored inside `Inner` (tokio async RwLock). `set_message_handler()` used `try_write()` which silently failed if the lock was contended, causing the handler to remain `None`. All delivered messages would then be silently dropped at `read_loop` line ~925.
**Fix:** Moved handler to its own `std::sync::RwLock<Option<MessageHandler>>` field on `NeboAIPlugin`. `set_message_handler()` now uses `.write().unwrap()` which always succeeds.

### Fixed: Silent message drop when handler is None
**Was:** When `handler` was `None` in the read loop, messages were silently swallowed with zero logging.
**Fix:** Added `warn!()` and devlog error entry when a message is dropped due to missing handler.

### Fixed: Token loss on hot-reload (partially)
**Was:** Process kill (cargo watch) prevented persisting the rotated JWT to DB. Next startup used the old DB token, which was already revoked by the gateway rotation → "stale token" connection failure.
**Fix:** `connect()` now writes the rotated token to `<data_dir>/neboai_token.cache` immediately on AUTH_OK. `activate_neboai()` reads this file before falling back to DB. Additionally, OAuth refresh is attempted on stale token errors.

### Open: Ghost connections from hot-reload (dev mode)
`cargo watch` sends SIGTERM/SIGKILL to the process. Spawned tokio tasks (read/write loops) are dropped immediately without running cleanup, so no WebSocket close frame is sent. The gateway keeps the old connection alive until its own keepalive timeout. A new process connects, creating a second connection for the same bot_id. The gateway may route inbound messages to the old (dead) connection, causing message loss. **Evidence:** devlog shows 46 connections with zero disconnect/error/cancel entries; only 1 inbound message received across all connections. **Mitigation:** Gateway should replace old connection when same bot_id reconnects. Client should implement SIGTERM handler for graceful disconnect.

### Fixed: Reconnect watcher now uses `wait_disconnect()`
The reconnect watcher in `lib.rs:1642` uses `tokio::select!` with both `wait_disconnect()` (instant notification when read loop exits) and a backoff sleep (periodic fallback). Whichever fires first triggers reconnect. This replaced the earlier polling-only approach.

### Open: No guard against concurrent `activate_neboai()` calls
`activate_neboai()` has a guard at the top (`if already connected, return Ok()`) but no mutex. If the function takes longer than the reconnect poll interval (30s), a second call can overlap. Both pass the guard (both see `false`), and both call `connect()`. The second `connect()` tears down the first at its top, but this creates wasted connections and confusing devlog entries.

### Open: Missing agent_spaces — `AddMember` early-return skips `EnsureAgentForBot`
**Root cause:** In the NeboAI server, `membership.go:AddMember()` returns early (line 32-38) if the bot is already a member of the loop, skipping the call to `EnsureAgentForBot()`. If membership was created before the agent system existed, or if `EnsureAgentForBot` failed on the original call, the bot has a valid loop membership but no agent, no agent_space conversation, and no default channels.

**Impact:** The gateway's `subscribeToAgentSpaces()` queries `ListAgentSpacesByBot` which returns empty. The bot connects successfully (AUTH_OK), auto-joins 5 bot_streams, but receives zero agent_space JOINs and zero channel JOINs. All owner↔agent conversations via the mobile/web app are undeliverable.

**Evidence:** Diagnostic analysis of `~/.nebo/logs/neboai.log` across 684 connections showed:
- 684 AUTH_OK (all healthy)
- 5 bot_stream JOINs per connect (dm, installs, chat, account, voice) ✓
- 0 agent_space JOINs across all 684 connections ✗
- 0 channel JOINs across all 684 connections ✗
- 0 chat/dm/agent_space messages ever received ✗

**Proposed fix:** Self-healing in gateway's `subscribeToAgentSpaces()` — if `ListAgentSpacesByBot` returns empty, check loop memberships and call `EnsureAgentForBot` for each loop, then re-query. Fix proposed to NeboAI team via Discuss (discussion `4e9cdce6`).

### Open: `reconcile_agents` does not cover the default bot agent
`reconcile_agents()` (codes.rs:1384-1457) only syncs custom agents from `list_agents()`. The default bot agent (slug `bot_*`) is created by `EnsureAgentForBot` on the NeboAI side during `AddMember`. If this default agent is missing (see above), `reconcile_agents` will not detect or repair it because it only iterates locally-installed agents, not the implicit default.

---

## 14. Complete End-to-End Sequence

```
Human types in NeboAI app (web/mobile), optionally attaches files/images/video
  │
  ▼
NeboAI Gateway receives message
  │ Resolves @mention → wraps with agentId/agentSlug/sourceChannelId
  │ Attachments metadata embedded in content.attachments[] (file bytes stored in object storage)
  │
  ▼
TYPE_MESSAGE_DELIVERY frame → WebSocket → Nebo desktop app
  │
  ▼
Read loop (neboai.rs:874+)
  ├─ Decode 47-byte header + JSON payload
  ├─ Decompress if zstd flag set
  ├─ Dedup check (skip if duplicate msg_id)
  ├─ Parse DeliveryPayload → build CommMessage (including attachments from content JSON)
  └─ Call registered handler callback
  │
  ▼
handle_comm_message (lib.rs:2272)
  ├─ topic="account" → persist token/plan update (no chat)
  ├─ topic="installs" → napp_registry
  ├─ topic="agent_space" / "chat" / "dm":
  │    ├─ Resolve agent, build session_key
  │    ├─ process_comm_attachments():
  │    │    ├─ image/* attachments → download via REST API → base64 → Vec<ImageContent>
  │    │    └─ non-image attachments → append "[Attached: filename (size)]" to prompt
  │    └─ Build ChatConfig {images, prompt (with attachment text), ...}
  └─ other → event_bus + broadcast to frontend
  │
  ▼
chat_dispatch::run_chat (chat_dispatch.rs:56)
  ├─ Register in RunRegistry
  ├─ Broadcast "chat_created" to frontend
  ├─ Enqueue on COMM lane
  │
  ▼
Lane task executes
  ├─ runner.run(RunRequest{origin: Comm, agent_id, images: [base64 vision data], ...})
  ├─ Agent processes (tools, thinking, text generation — can "see" attached images via vision)
  ├─ Stream events → broadcast to frontend (chat_stream 50ms, tool_start, etc.)
  ├─ Stream text → NeboAI comm channel (Stream chunks, 500ms coalesce)
  │
  ▼
Agent completes
  ├─ Flush remaining comm_buffer as final Stream chunk
  ├─ If no streaming happened → send full_response as single Message
  │
  ▼
NeboAIPlugin.send (neboai.rs:553)
  ├─ Resolve conversation_id (from CommReplyConfig)
  ├─ Build SendPayload{conversation_id, topic, content: {"text": response, "attachments": [...]}}
  ├─ Encode frame (TYPE_SEND_MESSAGE)
  └─ Queue to write loop
  │
  ▼
Write loop sends frame → WebSocket → NeboAI Gateway → recipient sees response
```
