# NeboLoop Connection & Hub Chat — SME Reference

This is a research/knowledge task, not an implementation plan. The output is a comprehensive SME reference for how the NeboLoop connection works and how chat flows through the hub.

---

## 1. Architecture Overview

The local Nebo desktop app connects to the NeboLoop cloud gateway via a **binary-framed WebSocket** protocol. The connection is managed by a plugin system (`CommPlugin` trait) with the `NeboLoopPlugin` as the production implementation. All inbound messages flow through a **single unified chat pipeline** (`chat_dispatch::run_chat()`), the same one used for local WebSocket chat.

**Key crates:**
- `crates/comm/` — Plugin trait, NeboLoop WebSocket client, REST API, wire protocol
- `crates/server/src/chat_dispatch.rs` — Unified chat dispatch pipeline
- `crates/server/src/lib.rs` — Message handler registration, routing logic
- `crates/server/src/codes.rs` — Connection activation, agent registration

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

---

## 3. Connection Lifecycle

### Initial Connection (`neboloop.rs:257-421`)

```
1. WebSocket dial → gateway URL (e.g. wss://comms.neboloop.com/ws)
2. Send CONNECT frame {bot_id, token}
3. Wait for AUTH_OK/AUTH_FAIL (10s timeout)
4. Extract rotated JWT from AUTH_OK → store for next reconnect
5. Reset ConvMaps (fresh for new connection)
6. Spawn 3 background tasks:
   - Read loop: decode frames → dedup → dispatch to handler
   - Write loop: drain send queue + 15s keepalive pings
   - Join processor: update ConvMaps from join results
7. Auto-join 5 bot streams: dm, installs, chat, account, voice
```

### Activation (`codes.rs:549-584`)

`activate_neboloop()` is the canonical connection function:
1. Read `bot_id` from config file
2. Read NeboLoop auth profile from DB (provider="neboloop")
3. Build config map: `{gateway, api_server, bot_id, token}`
4. Set "neboloop" as active plugin → `comm_manager.set_active("neboloop")`
5. Connect → `comm_manager.connect_active(config)`
6. Broadcast `settings_updated` to frontend

### Auto-connect & Reconnect (`lib.rs:848-882`)

- **Startup**: If NeboLoop enabled + credentials exist → `activate_neboloop()` in background task
- **Reconnect watcher**: Polls every `backoff_secs` (starts 30s, initial 60s delay)
  - If connected → reset backoff to 30s
  - If disconnected → attempt reconnect, on failure double backoff (max 600s)

### Token Rotation

- Gateway rotates JWT on every successful AUTH_OK
- Client retrieves via `plugin.take_rotated_token()` and persists to auth_profiles DB
- Prevents token expiry across reconnects

---

## 4. Conversation Maps (`neboloop.rs:647-704`)

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

**Join Result Classification** (read loop, `neboloop.rs:804-856`):
- If `agent_id` set → `JoinUpdate::AgentSpace`
- Else if `peer_id` set → `JoinUpdate::Dm` (legacy)
- Else if `channel_id` set → `JoinUpdate::Channel`
- Else → `JoinUpdate::BotStream` (pops from `pending_joins` FIFO)

---

## 5. Inbound Message Flow

### Read Loop (`neboloop.rs:793+`)

1. Receive WS binary message
2. Decode 47-byte header + payload
3. Decompress if compressed flag set (zstd)
4. Dedup check (sliding window: 1000 entries, 5min TTL)
5. For `TYPE_MESSAGE_DELIVERY`:
   - Parse `DeliveryPayload`
   - Build `CommMessage` with metadata (agent_id, agent_slug, source_channel_id)
   - Set `topic` = `delivery.stream` (e.g., "chat", "dm", "agent_space", "installs")
   - Call registered message handler callback (warns + logs if handler is None)
6. For `TYPE_JOIN_CONVERSATION`: route to join processor

### Message Handler (`lib.rs:835-846, 1034-1223`)

Two-phase handler registration:
- **Phase 1** (before AppState ready): installs → napp_registry, everything else → broadcast to frontend
- **Phase 2** (full handler): `handle_comm_message()` with complete routing

`handle_comm_message()` routes by `msg.topic`:

| Topic | Route | Details |
|-------|-------|---------|
| `"installs"` | `napp_registry.handle_install_event()` | Parse as InstallEvent, return immediately |
| `"agent_space"` | Unified chat pipeline | Resolve agent from agent_slug, session key = `neboloop:agent_space:{slug}:{conv_id}` |
| `"chat"` or `"dm"` | Unified chat pipeline | Optional @mention routing via agent_slug, session key = `neboloop:{topic}:{conv_id}` |
| Other | Event bus + broadcast | Emit as `neboloop.{topic}`, broadcast raw to frontend |

### Agent Space Messages (`lib.rs:1053-1122`)

```
1. Extract text from JSON content (msg.content.text or msg.content.content or raw)
2. Get agent_slug from msg.metadata["agent_slug"]
3. Resolve agent_id from slug (scan agent_registry for matching name)
4. Build session_key: "neboloop:agent_space:{slug}:{conversation_id}"
5. Pre-create chat record with "Agent: {agent_name}" title
6. Send desktop notification
7. Resolve entity config for ("channel", "agent_space")
8. Build ChatConfig {origin: Comm, lane: COMM, agent_id, comm_reply: {topic, conv_id}}
9. Dispatch to run_chat()
10. Emit event: "neboloop.agent_space.{slug}" → triggers agent event subscriptions
```

### Chat/DM Messages (`lib.rs:1125-1195`)

```
1. Extract text
2. Send desktop notification
3. Build session_key: "neboloop:{topic}:{conversation_id}"
4. Resolve entity config for ("channel", topic)
5. Check @mention: if agent_slug in metadata → resolve agent_id
6. If @mentioned: pre-create chat with "@{slug} (channel)" title
7. Build ChatConfig {origin: Comm, lane: COMM, agent_id (empty if no @mention), comm_reply}
8. Dispatch to run_chat()
9. Emit event: "neboloop.{topic}" → triggers agent event subscriptions
```

---

## 6. Unified Chat Pipeline (`chat_dispatch.rs:78-372`)

**ONE function for all chat**: `run_chat(state, config, active_runs)`

### ChatConfig Decorators

```rust
ChatConfig {
    session_key:   String,           // Unique conversation ID
    prompt:        String,           // User message text
    system:        String,           // System prompt override
    user_id:       String,           // User identity
    channel:       String,           // "neboloop" for comm messages
    origin:        Origin::Comm,     // Identifies source (Comm vs Ws vs Heartbeat)
    agent_id:      String,           // Route to specific agent (empty = main agent)
    cancel_token:  CancellationToken,
    lane:          String,           // "comm" for NeboLoop (not "main")
    comm_reply:    Option<CommReplyConfig>, // Where to send response back
    entity_config: Option<ResolvedEntityConfig>, // Permissions, model, personality
    images:        Vec<ImageContent>,
}
```

### Execution Flow

```
1. Register in active_runs (if provided) for external cancellation
2. Broadcast "chat_created" to frontend
3. Enqueue lane task on specified lane (e.g., COMM)
4. Lane task spawns agent runner.run(RunRequest)
5. Stream events from runner:
   - Text → coalesce 75ms, broadcast "chat_stream" to frontend
   - Thinking → broadcast "thinking"
   - ToolCall → broadcast "tool_start"
   - ToolResult → broadcast "tool_result"
   - Error → broadcast "chat_error"
   - Usage → broadcast "usage"
   - ApprovalRequest → broadcast "approval_request"
   - Done → exit loop
6. Flush remaining text buffer
7. If comm_reply configured → send accumulated response back via comm_manager.send()
8. Broadcast "chat_complete"
```

### CommReplyConfig (`chat_dispatch.rs:67-72`)

```rust
CommReplyConfig {
    topic: String,          // "agent_space", "chat", or "dm"
    conversation_id: String, // NeboLoop conversation ID
}
```

After the agent completes, the full accumulated text response is packaged into a `CommMessage` and sent back through `comm_manager.send()` → WebSocket → NeboLoop gateway → recipient. **Response is NOT streamed — it's sent as one complete message after agent finishes.**

---

## 7. Outbound Message Flow

### From Chat Reply (`chat_dispatch.rs:316-343`)

```
full_response (accumulated text) → CommMessage {
    topic: reply_config.topic,
    conversation_id: reply_config.conversation_id,
    msg_type: Message,
    content: full_response,
} → comm_manager.send() → NeboLoopPlugin.send()
```

### NeboLoopPlugin.send() (`neboloop.rs:442-496`)

```
1. Resolve conversation_id:
   - If msg.conversation_id set → use it
   - Else if msg.to set → lookup agent_space_by_slug, then dm_by_peer
   - Else → error
2. Determine stream:
   - If msg.topic set → use it
   - Else if resolved via agent_space → "agent_space"
   - Else → "dm"
3. Wrap content as {"text": msg.content}
4. Build SendPayload → encode frame → queue to write loop
```

### From Agent Tools (message_tool.rs / loop tool)

Agents can also send messages proactively using the "loop" tool:
- `dm.send(to, text)` — DM another agent
- `channel.send(channel_id, text)` — Post to channel
- Requires "loop" permission enabled

---

## 8. Lane System

| Lane | Purpose | Concurrency |
|------|---------|-------------|
| `main` | User chat from local WebSocket | Unlimited |
| **`comm`** | **Inbound NeboLoop messages** | **Unlimited** |
| `events` | Event-triggered workflows | Unlimited |
| `heartbeat` | Agent proactive ticks | Unlimited |
| `desktop` | Screen/mouse automation | 1 (serialized) |

COMM lane prevents NeboLoop message flood from blocking local user chat on MAIN lane.

---

## 9. Agent Registration

When an agent is installed/activated, the bot registers an agent in the owner's personal loop (`codes.rs:622-644`):

```
1. Get API client from stored credentials
2. List bot's loops → pick first (personal loop)
3. POST /api/v1/loops/{loopId}/agents {agent_name, agent_slug, description}
4. Gateway auto-creates agent_space conversation + subscribes bot to it
```

Deregistration on agent removal: `DELETE /api/v1/loops/{loopId}/agents/{agentSlug}`

---

## 10. REST API Client (`crates/comm/src/api.rs`)

`NeboLoopApi` provides authenticated REST endpoints alongside the WebSocket connection:

| Category | Key Endpoints |
|----------|--------------|
| **Marketplace** | `list_products`, `list_skills`, `get_skill`, `list_categories`, `get_featured` |
| **Code Redemption** | `redeem_code(code)` — universal (SKIL/WORK/AGNT/NEBO/LOOP codes) |
| **Downloads** | `download_napp(url)` — sealed .napp archives |
| **Loops** | `list_bot_loops`, `get_loop`, `join_loop(code)`, `list_loop_members` |
| **Channels** | `list_bot_channels`, `list_channel_messages`, `list_channel_members` |
| **Agents** | `register_agent`, `deregister_agent` |
| **Billing** | `billing_prices`, `billing_subscription`, `billing_checkout`, `billing_cancel` |
| **Bot Identity** | `update_bot_identity(name, role)` |

API URL derived from gateway: `wss://comms.neboloop.com/ws` → `https://api.neboloop.com`

---

## 11. Event Bus Integration

Every incoming NeboLoop message emits an event for agent triggers:

| Message Topic | Event Source | Payload |
|---------------|-------------|---------|
| agent_space | `neboloop.agent_space.{slug}` | from, content, conversation_id, agent_slug |
| chat | `neboloop.chat` | from, content, conversation_id |
| dm | `neboloop.dm` | from, content, conversation_id |
| other | `neboloop.{topic}` | from, content, topic |

Agents subscribe to these event sources in their configuration, triggering automated responses.

---

## 12. Key Implementation Details

1. **Token rotation**: Every AUTH_OK includes a new JWT. Must persist via `take_rotated_token()` for next connect.
2. **Pending joins FIFO**: Bot stream JoinResult doesn't include stream name. Read loop sends empty key; join processor pops from `pending_joins` queue (FIFO order matches join order).
3. **Dedup per connection**: Fresh `DedupWindow` (1000 entries, 5min TTL) created each connect.
4. **Compression**: Zstd for payloads > 1KB, flagged in header byte.
5. **Session isolation**: Each NeboLoop conversation gets unique `session_key` → separate chat history.
6. **Response is sync, not streamed**: Agent response accumulated during execution, sent as single message after completion.
7. **ConvMaps thread safety**: `Arc<RwLock<ConvMaps>>` shared across read loop, join processor, and public query methods.
8. **Graceful shutdown**: Single `CancellationToken` coordinates all 3 background tasks (read, write, join processor). Note: process kill (SIGKILL / cargo watch) bypasses this — spawned tasks are dropped without running cleanup.
9. **Entity config**: Per-topic permissions resolved via `entity_config::resolve_for_chat("channel", topic)`.
10. **@mention routing**: `agent_slug` in DeliveryPayload metadata → resolve to local agent_id → route to that agent's persona.
11. **Handler field**: Message handler is stored in a separate `std::sync::RwLock` (not inside the async `Inner` RwLock) so `set_message_handler()` always succeeds synchronously. The handler is cloned at connect time and passed to the read loop as a local variable.

---

## 12b. Known Issues & Fixes Applied

### Fixed: `try_write()` race in `set_message_handler` (neboloop.rs)
**Was:** Handler stored inside `Inner` (tokio async RwLock). `set_message_handler()` used `try_write()` which silently failed if the lock was contended, causing the handler to remain `None`. All delivered messages would then be silently dropped at `read_loop` line ~925.
**Fix:** Moved handler to its own `std::sync::RwLock<Option<MessageHandler>>` field on `NeboLoopPlugin`. `set_message_handler()` now uses `.write().unwrap()` which always succeeds.

### Fixed: Silent message drop when handler is None
**Was:** When `handler` was `None` in the read loop, messages were silently swallowed with zero logging.
**Fix:** Added `warn!()` and devlog error entry when a message is dropped due to missing handler.

### Open: Ghost connections from hot-reload (dev mode)
`cargo watch` sends SIGTERM/SIGKILL to the process. Spawned tokio tasks (read/write loops) are dropped immediately without running cleanup, so no WebSocket close frame is sent. The gateway keeps the old connection alive until its own keepalive timeout. A new process connects, creating a second connection for the same bot_id. The gateway may route inbound messages to the old (dead) connection, causing message loss. **Evidence:** devlog shows 46 connections with zero disconnect/error/cancel entries; only 1 inbound message received across all connections. **Mitigation:** Gateway should replace old connection when same bot_id reconnects. Client should implement SIGTERM handler for graceful disconnect.

### Open: Reconnect watcher uses polling instead of `wait_disconnect()`
The reconnect watcher in `lib.rs` polls `is_connected()` every 30s. The `wait_disconnect()` method exists on the plugin trait and fires immediately when the read loop exits unexpectedly, but it's not used. Using it would reduce reconnect latency from up to 30s to near-instant.

### Open: No guard against concurrent `activate_neboloop()` calls
If `activate_neboloop()` takes longer than the reconnect poll interval (30s), a second call can overlap. Both pass the `is_connected()` guard (both see `false`), and both call `connect()`. The second `connect()` will disconnect the first via `self.disconnect()` at the top of `connect()`, but this creates wasted connections and confusing devlog entries.

---

## 13. Complete End-to-End Sequence

```
Human types in NeboLoop app (web/mobile)
  │
  ▼
NeboLoop Gateway receives message
  │ Resolves @mention → wraps with agentId/agentSlug/sourceChannelId
  │
  ▼
TYPE_MESSAGE_DELIVERY frame → WebSocket → Nebo desktop app
  │
  ▼
Read loop (neboloop.rs:707)
  ├─ Decode 47-byte header + JSON payload
  ├─ Decompress if zstd flag set
  ├─ Dedup check (skip if duplicate msg_id)
  ├─ Parse DeliveryPayload → build CommMessage
  └─ Call registered handler callback
  │
  ▼
handle_comm_message (lib.rs:1035)
  ├─ topic="installs" → napp_registry
  ├─ topic="agent_space" → resolve agent from slug, build ChatConfig
  ├─ topic="chat"/"dm" → optional @mention resolve, build ChatConfig
  └─ other → event_bus + broadcast to frontend
  │
  ▼
chat_dispatch::run_chat (chat_dispatch.rs:78)
  ├─ Broadcast "chat_created" to frontend
  ├─ Enqueue on COMM lane
  │
  ▼
Lane task executes
  ├─ runner.run(RunRequest{origin: Comm, agent_id, ...})
  ├─ Agent processes (tools, thinking, text generation)
  ├─ Stream events → broadcast to frontend (chat_stream, tool_start, etc.)
  │
  ▼
Agent completes
  ├─ Accumulated full_response text
  ├─ CommReplyConfig present → comm_manager.send(reply)
  │
  ▼
NeboLoopPlugin.send (neboloop.rs:442)
  ├─ Resolve conversation_id (from CommReplyConfig)
  ├─ Build SendPayload{conversation_id, topic, content: {"text": response}}
  ├─ Encode frame (TYPE_SEND_MESSAGE)
  └─ Queue to write loop
  │
  ▼
Write loop sends frame → WebSocket → NeboLoop Gateway → recipient sees response
```
