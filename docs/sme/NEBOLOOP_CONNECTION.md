# NeboLoop Connection & Hub Chat — SME Reference

This is a research/knowledge task, not an implementation plan. The output is a comprehensive SME reference for how the NeboLoop connection works and how chat flows through the hub.

---

## 1. Architecture Overview

The local Nebo desktop app connects to the NeboLoop cloud gateway via a **binary-framed WebSocket** protocol. The connection is managed by a plugin system (`CommPlugin` trait) with the `NeboLoopPlugin` as the production implementation. All inbound messages flow through a **single unified chat pipeline** (`chat_dispatch::run_chat()`), the same one used for local WebSocket chat.

**Key crates:**
- `crates/comm/` — Plugin trait, NeboLoop WebSocket client, REST API, wire protocol
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

---

## 3. Connection Lifecycle

### Initial Connection (`neboloop.rs:268-497`)

```
0. Tear down existing connection (cancel old tasks, clear stale state)
   - If is_connected → disconnect()
   - Else → cancel any stale token, clear send_tx
1. WebSocket dial → gateway URL (e.g. wss://comms.neboloop.com/ws)
2. Send CONNECT frame {bot_id, token}
3. Wait for AUTH_OK/AUTH_FAIL (10s timeout)
4. Extract rotated JWT from AUTH_OK → store in memory + persist to <data_dir>/neboloop_token.cache
5. Reset ConvMaps (fresh for new connection)
6. Spawn 3 background tasks:
   - Read loop: decode frames → dedup → dispatch to handler
   - Write loop: drain send queue + 15s keepalive pings
   - Join processor: update ConvMaps from join results
7. Auto-join 5 bot streams: dm, installs, chat, account, voice
```

The token cache file at `<data_dir>/neboloop_token.cache` is persisted immediately on AUTH_OK to survive hot-reload kills (SIGKILL) that prevent the caller from writing to DB.

### Activation (`codes.rs:794-904`)

`activate_neboloop()` is the canonical connection function:
1. Guard: if already connected, return `Ok(())`
2. Read `bot_id` from config
3. Read NeboLoop auth profile from DB (provider="neboloop")
4. Get JWT token: check cached file first (`<data_dir>/neboloop_token.cache`), fall back to DB token
5. Build config map: `{gateway, api_server, bot_id, token, data_dir}`
6. Set "neboloop" as active plugin → `comm_manager.set_active("neboloop")`
7. Connect → `comm_manager.connect_active(config)`
8. On stale token error → OAuth refresh (`refresh_neboloop_token`) → retry connect
9. Persist rotated JWT from AUTH_OK to DB auth_profiles
10. Broadcast `settings_updated` to frontend
11. Spawn background reconciliation: `reconcile_agents()` + `sync_bot_identity()`

### Auto-connect & Reconnect (`lib.rs:988-1027`)

- **Startup**: If NeboLoop enabled + credentials exist → `activate_neboloop()` in background task
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

## 4. Conversation Maps (`neboloop.rs:746-758`)

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

**Join Result Classification** (read loop, `neboloop.rs:1000-1046`):
- If `agent_id` set → `JoinUpdate::AgentSpace`
- Else if `peer_id` set → `JoinUpdate::Dm` (legacy)
- Else if `channel_id` set → `JoinUpdate::Channel`
- Else → `JoinUpdate::BotStream` (pops from `pending_joins` FIFO)

---

## 5. Inbound Message Flow

### Read Loop (`neboloop.rs:805+`)

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

### Message Handler (`lib.rs:974-985, 1212-1555`)

Single handler registration at startup — message handler is set once during `start_server()`, wrapping each incoming message in a spawned async task that calls `handle_comm_message()`.

`handle_comm_message()` routes by `msg.topic`:

| Topic | Route | Details |
|-------|-------|---------|
| `"account"` | Token/plan update | Parse tokenRefresh → persist JWT to DB, update plan_tier, broadcast `plan_changed` |
| `"installs"` | `napp_registry.handle_install_event()` | Parse as InstallEvent, broadcast tool_event/tool_error, return immediately |
| `"agent_space"` | Unified chat pipeline | Resolve agent from agent_slug, session key = `neboloop:agent_space:{slug}:{conv_id}` |
| `"chat"` or `"dm"` | Unified chat pipeline | Agent-space reroute check → optional @mention routing, session key = `neboloop:{topic}:{conv_id}` |
| Other | Event bus + broadcast | Emit as `neboloop.{topic}`, broadcast raw to frontend |

### Account Messages (`lib.rs:1222-1254`)

```
1. Parse JSON content for type="tokenRefresh"
2. Extract new JWT token and plan tier
3. Persist JWT to auth_profiles DB table
4. Update in-memory plan_tier atomic
5. Broadcast "plan_changed" event to frontend
6. Return (no chat dispatch)
```

### Agent Space Messages (`lib.rs:1272-1349`)

```
1. Extract text from JSON content (msg.content.text or msg.content.content or raw)
2. Get agent_slug from msg.metadata["agent_slug"]
3. Resolve agent_id from slug (scan agent_registry for matching name)
4. Build session_key: "neboloop:agent_space:{slug}:{conversation_id}"
5. Pre-create chat record with "Agent: {agent_name}" title
6. Send desktop notification
7. Resolve entity config for ("channel", "agent_space")
8. Build ChatConfig {origin: Comm, lane: COMM, agent_id, comm_reply: {topic: "agent_space", conv_id}}
9. Dispatch to run_chat()
10. Emit event: "neboloop.agent_space.{slug}" → triggers agent event subscriptions
```

### Chat/DM Messages (`lib.rs:1352-1500`)

```
1. Agent-space reroute check: call comm_manager.agent_slug_for_conv(conversation_id)
   - If conversation belongs to an agent space (gateway sends stream=dm for these):
     Reroute to agent space path with comm_reply topic=msg.topic (preserves "dm")
     Same flow as agent_space above, but reply goes back on original topic
2. Extract text
3. Send desktop notification
4. Build session_key: "neboloop:{topic}:{conversation_id}"
5. Resolve entity config for ("channel", topic)
6. Check @mention: if agent_slug in metadata → resolve agent_id
7. If @mentioned: pre-create chat with "@{slug} (channel)" title
8. Build ChatConfig {origin: Comm, lane: COMM, agent_id (empty if no @mention), comm_reply}
9. Dispatch to run_chat()
10. Emit event: "neboloop.{topic}" → triggers agent event subscriptions
```

---

## 6. Unified Chat Pipeline (`chat_dispatch.rs:56-537`)

**ONE function for all chat**: `run_chat(state, config)`

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
   - Text → coalesce 75ms, broadcast "chat_stream" to frontend
          → if comm_reply set: coalesce 500ms, send Stream chunks to NeboLoop
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
7. Flush remaining comm_buffer to NeboLoop (Stream type)
8. If no stream chunks were sent (short response) → send full_response as single Message
9. Broadcast "chat_complete"
```

### Comm Response Streaming

Responses to NeboLoop are **streamed in real-time** (not batched). Two coalescing windows operate in parallel:
- **Frontend**: 75ms coalesce → `chat_stream` WebSocket event
- **NeboLoop comm**: 500ms coalesce → `CommMessageType::Stream` chunks via `comm_manager.send()`

After the agent completes:
- Remaining `comm_buffer` is flushed as a final Stream chunk
- If no chunks were streamed at all (response completed before first 500ms flush), the full response is sent as a single `CommMessageType::Message`
- This prevents duplicate messages: once streaming has started, no final "complete" message is sent

### CommReplyConfig (`chat_dispatch.rs:45-49`)

```rust
CommReplyConfig {
    topic: String,          // "agent_space", "chat", or "dm"
    conversation_id: String, // NeboLoop conversation ID
}
```

---

## 7. Outbound Message Flow

### From Chat Reply (`chat_dispatch.rs:431-507`)

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

### NeboLoopPlugin.send() (`neboloop.rs:517-581`)

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
5. Build SendPayload → encode frame → queue to write loop
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
| **`comm`** | **Inbound NeboLoop messages** | **Unlimited** |
| `events` | Event-triggered workflows | Unlimited |
| `heartbeat` | Agent proactive ticks | Unlimited |
| `subagent` | Subagent execution | Unlimited |
| `nested` | Nested agent calls | Unlimited |
| `dev` | Developer/debug lane | Unlimited |
| `desktop` | Screen/mouse automation | **1 (serialized)** |

COMM lane prevents NeboLoop message flood from blocking local user chat on MAIN lane. DESKTOP is the only serialized lane — all others have unlimited concurrency.

---

## 9. Agent Registration

When an agent is installed/activated, the bot registers an agent in the owner's personal loop (`codes.rs:1115-1133`):

```
1. Get API client from stored credentials
2. List bot's loops → pick first (personal loop)
3. POST /api/v1/loops/{loopId}/agents {agent_name, agent_slug, description}
4. Gateway auto-creates agent_space conversation + subscribes bot to it
```

Deregistration on agent removal: `DELETE /api/v1/loops/{loopId}/agents/{agentSlug}`

### Reconciliation (`codes.rs:930-996`)

On every successful connection, `reconcile_agents()` syncs local ↔ remote:
- Fetches remote agents from NeboLoop
- Deregisters agents that are truly **deleted** locally (not just disabled)
- Registers local agents missing from remote
- **Disabled** agents are synced with status `"paused"` — they are NOT deregistered

---

## 10. REST API Client (`crates/comm/src/api.rs`)

`NeboLoopApi` provides authenticated REST endpoints alongside the WebSocket connection:

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
| **Referral** | `get_referral_code` |
| **Unauthenticated** | `redeem_connect_code(code)` — for initial bot activation |

API server URL comes from config (`state.config.neboloop.api_url`), defaulting to `https://api.neboloop.com`.

---

## 11. Event Bus Integration

Every incoming NeboLoop message emits an event for agent triggers:

| Message Topic | Event Source | Payload |
|---------------|-------------|---------|
| agent_space | `neboloop.agent_space.{slug}` | from, content, conversation_id, agent_slug |
| chat | `neboloop.chat` | from, content, conversation_id |
| dm | `neboloop.dm` | from, content, conversation_id |
| other | `neboloop.{topic}` | from, content, topic |

All events include `origin: "neboloop"` and a UNIX timestamp.

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
}
```

The `Stream` message type is used for real-time response streaming (500ms coalesced chunks). `Task`/`TaskResult`/`TaskStatus` types support the A2A protocol for inter-agent task delegation.

---

## 13. Key Implementation Details

1. **Token rotation**: Every AUTH_OK includes a new JWT. Persisted immediately to cache file + later to DB via `take_rotated_token()`.
2. **Pending joins FIFO**: Bot stream JoinResult doesn't include stream name. Read loop sends empty key; join processor pops from `pending_joins` queue (FIFO order matches join order).
3. **Dedup per connection**: Fresh `DedupWindow` (1000 entries, 5min TTL) created each connect.
4. **Compression**: Zstd for payloads > 1KB, flagged in header byte.
5. **Session isolation**: Each NeboLoop conversation gets unique `session_key` → separate chat history.
6. **Response streaming**: Responses are streamed in real-time at 500ms intervals via `CommMessageType::Stream`. Short responses (< 500ms total) are sent as a single `CommMessageType::Message`. No duplicate final message when streaming has started.
7. **ConvMaps thread safety**: `Arc<RwLock<ConvMaps>>` shared across read loop, join processor, and public query methods.
8. **Graceful shutdown**: Single `CancellationToken` coordinates all 3 background tasks (read, write, join processor). Note: process kill (SIGKILL / cargo watch) bypasses this — spawned tasks are dropped without running cleanup.
9. **Entity config**: Per-topic permissions resolved via `entity_config::resolve_for_chat("channel", topic)`.
10. **@mention routing**: `agent_slug` in DeliveryPayload metadata → resolve to local agent_id → route to that agent's persona.
11. **Handler field**: Message handler is stored in a separate `std::sync::RwLock` (not inside the async `Inner` RwLock) so `set_message_handler()` always succeeds synchronously. The handler is cloned at connect time and passed to the read loop as a local variable.
12. **DM→agent_space reroute**: Gateway sometimes sends `stream=dm` for agent space conversations. `handle_comm_message()` checks `agent_slug_for_conv()` and reroutes to the agent space path if the conversation belongs to an agent.
13. **Token cache file**: `<data_dir>/neboloop_token.cache` survives hot-reload kills. `activate_neboloop()` reads this first, falling back to DB token.
14. **RunRegistry**: All chat runs auto-register in the global RunRegistry for visibility, progress tracking, and external cancellation.

---

## 13b. Known Issues & Fixes Applied

### Fixed: `try_write()` race in `set_message_handler` (neboloop.rs)
**Was:** Handler stored inside `Inner` (tokio async RwLock). `set_message_handler()` used `try_write()` which silently failed if the lock was contended, causing the handler to remain `None`. All delivered messages would then be silently dropped at `read_loop` line ~925.
**Fix:** Moved handler to its own `std::sync::RwLock<Option<MessageHandler>>` field on `NeboLoopPlugin`. `set_message_handler()` now uses `.write().unwrap()` which always succeeds.

### Fixed: Silent message drop when handler is None
**Was:** When `handler` was `None` in the read loop, messages were silently swallowed with zero logging.
**Fix:** Added `warn!()` and devlog error entry when a message is dropped due to missing handler.

### Fixed: Token loss on hot-reload (partially)
**Was:** Process kill (cargo watch) prevented persisting the rotated JWT to DB. Next startup used the old DB token, which was already revoked by the gateway rotation → "stale token" connection failure.
**Fix:** `connect()` now writes the rotated token to `<data_dir>/neboloop_token.cache` immediately on AUTH_OK. `activate_neboloop()` reads this file before falling back to DB. Additionally, OAuth refresh is attempted on stale token errors.

### Open: Ghost connections from hot-reload (dev mode)
`cargo watch` sends SIGTERM/SIGKILL to the process. Spawned tokio tasks (read/write loops) are dropped immediately without running cleanup, so no WebSocket close frame is sent. The gateway keeps the old connection alive until its own keepalive timeout. A new process connects, creating a second connection for the same bot_id. The gateway may route inbound messages to the old (dead) connection, causing message loss. **Evidence:** devlog shows 46 connections with zero disconnect/error/cancel entries; only 1 inbound message received across all connections. **Mitigation:** Gateway should replace old connection when same bot_id reconnects. Client should implement SIGTERM handler for graceful disconnect.

### Open: Reconnect watcher uses polling instead of `wait_disconnect()`
The reconnect watcher in `lib.rs` polls `is_connected()` every 30s. The `wait_disconnect()` method exists on the plugin trait and fires immediately when the read loop exits unexpectedly, but it's not used. Using it would reduce reconnect latency from up to 30s to near-instant.

### Open: No guard against concurrent `activate_neboloop()` calls
`activate_neboloop()` has a guard at the top (`if already connected, return Ok()`) but no mutex. If the function takes longer than the reconnect poll interval (30s), a second call can overlap. Both pass the guard (both see `false`), and both call `connect()`. The second `connect()` tears down the first at its top, but this creates wasted connections and confusing devlog entries.

---

## 14. Complete End-to-End Sequence

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
Read loop (neboloop.rs:805+)
  ├─ Decode 47-byte header + JSON payload
  ├─ Decompress if zstd flag set
  ├─ Dedup check (skip if duplicate msg_id)
  ├─ Parse DeliveryPayload → build CommMessage
  └─ Call registered handler callback
  │
  ▼
handle_comm_message (lib.rs:1212)
  ├─ topic="account" → persist token/plan update (no chat)
  ├─ topic="installs" → napp_registry
  ├─ topic="agent_space" → resolve agent from slug, build ChatConfig
  ├─ topic="chat"/"dm" → agent_space reroute check → optional @mention resolve, build ChatConfig
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
  ├─ runner.run(RunRequest{origin: Comm, agent_id, ...})
  ├─ Agent processes (tools, thinking, text generation)
  ├─ Stream events → broadcast to frontend (chat_stream 75ms, tool_start, etc.)
  ├─ Stream text → NeboLoop comm channel (Stream chunks, 500ms coalesce)
  │
  ▼
Agent completes
  ├─ Flush remaining comm_buffer as final Stream chunk
  ├─ If no streaming happened → send full_response as single Message
  │
  ▼
NeboLoopPlugin.send (neboloop.rs:517)
  ├─ Resolve conversation_id (from CommReplyConfig)
  ├─ Build SendPayload{conversation_id, topic, content: {"text": response}}
  ├─ Encode frame (TYPE_SEND_MESSAGE)
  └─ Queue to write loop
  │
  ▼
Write loop sends frame → WebSocket → NeboLoop Gateway → recipient sees response
```
