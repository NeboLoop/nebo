# Channel Plugins

A **channel plugin** is a plugin that connects a Nebo agent to a messaging surface — Slack, Discord, Microsoft Teams, IRC, etc. The plugin owns the persistent connection to the platform (Socket Mode WebSocket, webhook receiver, etc.) and exchanges messages with Nebo through a single canonical pathway: NDJSON ops on the bridge process's stdin/stdout.

This guide describes the conventions a channel plugin MUST follow. The convention exists because **every channel plugin in Nebo MUST have exactly one canonical pathway for every messaging operation**. Two invocation models for the same operation (e.g. a CLI subcommand and a bridge handler) race for the upstream socket and create silent failures — we've shipped fewer pathways, not more.

This shape mirrors the trait-based adapters used by [openclaw](https://github.com/openclaw/openclaw)'s `ChannelPlugin<TAccount, TProbe>` and the `BasePlatformAdapter` ABC in hermes-agent — expressed through Nebo's sidecar process model.

---

## Required Subcommands

A channel plugin MUST expose these subcommands. Names are NOT optional — Nebo's runtime, docs, and agent system prompts assume them.

| Subcommand | Purpose | Lifecycle | Why CLI vs bridge |
|---|---|---|---|
| `bridge --listen` | Long-running bidirectional channel bridge. Owns the live platform connection. Reads outbound ops as NDJSON from stdin (reply/post/upload/dm); prints inbound platform events as NDJSON on stdout. | Long-running | Owns the persistent connection — all real-time ops route through it. |
| `auth login` / `auth status` / `auth logout` | Authentication management. | One-shot | Stateless; doesn't need the live connection. |
| `init` | Initialize local state (DB, default workspace) | One-shot | Stateless; runs once at install. |
| `doctor` | Diagnostics (connectivity, scopes, archive integrity) | One-shot | Reports state; doesn't need to be inside the bridge. |

Optional but recommended for archiver-style plugins:

| Subcommand | Purpose |
|---|---|
| `sync` | Download workspace history into a local archive |
| `search`, `users`, `channels`, `messages` | Read-only queries against the local archive |

**Real-time messaging operations (reply, post, upload, dm) have NO CLI subcommand.** They route through the bridge's stdin. See [Bridge Stdin Protocol](#bridge-stdin-protocol).

---

## The Bridge Process

The `bridge --listen` process is how messages flow in real time. Nebo spawns one bridge per `(agent_id, plugin_slug)` pair — each agent that has the plugin enabled gets its own bridge process with its own credentials. Nebo supervises the process for its lifetime; the bridge exits when the channel is toggled off, the parent Nebo exits, or `cancel` is signalled.

### Process lifecycle — exit on stdin EOF (REQUIRED)

Any long-running process a plugin spawns — the bridge itself, and any watcher/poller/subscriber it forks that holds an upstream connection — MUST terminate itself when its stdin reaches EOF.

**Why:** Nebo spawns the bridge with a piped stdin and holds the write end open for the bridge's entire life. When Nebo goes away by *any* means — graceful shutdown, panic, crash, or `kill -9` (SIGKILL, which no signal handler can intercept) — the operating system closes that pipe, and the bridge's `read()` on stdin returns EOF. **This is the only parent-death signal that survives SIGKILL.** A bridge that ignores EOF and keeps its Socket Mode WebSocket (or Discord gateway, or IMAP IDLE) open becomes an orphan: it holds the upstream connection, may keep posting, and silently competes with the new bridge Nebo spawns on its next launch.

**The contract:** the bridge's stdin reader loop MUST, when it observes EOF (a read returning zero bytes / `Ok(None)` / a closed-stream error), log one line to stderr and exit the *whole process* — not merely end the reader task.

```rust
// Slack reference: slack-cli/src/commands/bridge.rs::read_stdin
while let Ok(Some(line)) = lines.next_line().await {
    // …dispatch ops…
}
// stdin closed → Nebo is gone. Exit so we don't orphan the upstream connection.
eprintln!("bridge: stdin closed (Nebo exited), shutting down");
std::process::exit(0);
```

If a long-running mode does **not** otherwise read stdin (e.g. a one-way event watcher that only writes to stdout), spawn a dedicated parent-death watchdog task whose sole job is to drain stdin to EOF and then `exit(0)`:

```rust
tokio::spawn(async {
    use tokio::io::AsyncReadExt;
    let mut stdin = tokio::io::stdin();
    let mut buf = [0u8; 256];
    loop {
        match stdin.read(&mut buf).await {
            Ok(0) | Err(_) => break,   // EOF or error → parent gone
            Ok(_) => continue,          // bytes ignored; stdin is used only as a death pipe
        }
    }
    eprintln!("<plugin>: stdin closed (Nebo exited), shutting down");
    std::process::exit(0);
});
```

This complements — it does not replace — Nebo's `kill_on_drop` + signal-handler cleanup, which cover the *graceful* exit path. EOF self-exit is what closes the SIGKILL window those can't. Short-lived one-shot subcommands (`auth`, `init`, `doctor`) need none of this — they exit on their own and cannot orphan.

### stdout — inbound (plugin → Nebo)

NDJSON, one event per line, flushed after each write. Inbound events represent something that happened on the upstream platform (a user sent a message, a file was shared, etc.).

```json
{
  "text": "Hi, can you generate the Q4 report?",
  "channel": "C1234567890",
  "user": "U0987654321",
  "thread_ts": "1700000000.123456",
  "ts": "1700000060.234567",
  "placeholder_ts": "1700000061.345678"
}
```

Required fields: `text`, `channel`. Recommended: `user`, `thread_ts`, `placeholder_ts` — the timestamp of a "_Thinking..._" placeholder the bridge posts immediately on receiving an inbound message, so Nebo can later update it with the final agent reply.

### stdin — outbound (Nebo → plugin)

NDJSON, one op per line. Each line MUST include an `op` field that selects the handler. There is no fallback for missing-`op` lines — they are logged and dropped.

```json
{ "op": "reply",  "req_id": "…", "channel": "C1", "thread_ts": "1.2", "user_ts": "1.3", "text": "...", "files": [{"path": "/abs/x.png", "title": "..."}], "username": "Nebo" }
{ "op": "post",   "req_id": "…", "channel": "C1", "thread_ts": null, "text": "...", "files": [], "username": "Nebo" }
{ "op": "upload", "req_id": "…", "channel": "C1", "thread_ts": null, "path": "/abs/file", "caption": "Q4 report" }
{ "op": "dm",     "req_id": "…", "user": "U1",   "text": "...", "files": [], "username": "Nebo" }
```

When Nebo sends a `req_id`, the bridge MUST echo it back in an `op_result`
event on stdout once the handler finishes (success or failure). Without
this correlation, the agent-side tool result would acknowledge "sent"
based purely on stdin acceptance — and the agent would tell the user
"uploaded" even when the bridge's handler later failed. See
`crates/tools/src/plugin_tool.rs::route_through_bridge` for the awaiter.

Paths like `~/Desktop/foo.pdf` are expanded by the bridge before reading —
plugin handlers should treat the incoming `path` as a real local path and
not require the agent to canonicalize it.

### stdout `op_result` event — outcome correlation

For every stdin op carrying a `req_id`, the bridge writes exactly one
NDJSON line back with this shape, after the handler returns:

```json
{ "event": "op_result", "req_id": "…", "op": "upload", "ok": true,  "error": null }
{ "event": "op_result", "req_id": "…", "op": "upload", "ok": false, "error": "files.completeUploadExternal failed: missing_scope" }
```

Required: `event="op_result"`, `req_id`, `ok`. Recommended: `op`, `error`
when `ok=false`. Nebo's `channel_loop` looks up the `req_id` in its
pending-ops map and forwards the outcome to the awaiting agent-side
caller. If the bridge crashes or restarts before responding, the awaiter
sees a "bridge closed before reporting" error after a 30s timeout —
either way the agent never silently believes a failed op succeeded.

### stdout `keepalive` event — bridge liveness contract

Every channel plugin's `bridge --listen` process MUST emit a keepalive
NDJSON line on stdout **at least every 10 seconds** for as long as the
process is running:

```json
{ "event": "keepalive", "status": "connected" }
{ "event": "keepalive", "status": "disconnected" }
```

Required: `event="keepalive"`. Recommended: `status` ∈
`{"connected", "disconnected"}` reflecting the upstream platform
connection — `connected` while the WebSocket / poller / webhook receiver
is healthy, `disconnected` while the bridge is in its internal
reconnect-backoff. Nebo logs the status but does not act on it; the
event being received at all is what matters.

**Why this matters:** the bridge process can stay alive while its
upstream connection silently dies (NAT timeout, post-sleep half-open
socket, wifi blip without TCP `RST`). Without an explicit liveness
signal, Nebo would never notice — the bridge sits parked on `read()`,
no events flow, and from Nebo's side the channel looks idle rather than
broken. The keepalive event is the cross-plugin signal Nebo's watchdog
uses to detect this.

**Nebo's watchdog**: `channel_loop` tracks the last keepalive timestamp
per bridge. If no keepalive arrives within **30 seconds**, Nebo kills
the child process; the outer loop respawns with backoff. This handles
"process alive but channel dead" generically — independent of which
platform the plugin talks to. The 30-second threshold tolerates two
missed keepalives at the 10s cadence — tight enough that a hung bridge
recovers before the user notices.

**Naming note:** we deliberately do NOT call this a "heartbeat."
`heartbeat` is reserved in Nebo for the workflow trigger type
(`type: heartbeat`, `interval: 30m`) that fires agent workflows on a
schedule. The bridge liveness signal is `keepalive` to avoid that
collision.

**Protocol-level keepalive is each plugin's responsibility.** This
stdout event is for Nebo↔bridge liveness only. The upstream connection's
own keepalive idiom is unrelated and lives inside the plugin:

| Platform | Plugin-internal keepalive |
|---|---|
| Slack Socket Mode | Outbound WebSocket `Ping` every 30s + 60s idle-read timeout (`slack-core/socket_mode.rs`). Slack also pings us ~every 15s; either resets the idle clock. |
| Discord Gateway | Application-level heartbeat (op code 1 / ACK op code 11) on the server-dictated interval from the `Hello` payload. WebSocket `Ping` frames are NOT used. |
| Microsoft Teams (Bot Framework) | Inbound webhooks — no persistent connection, no keepalive needed. |
| IMAP IDLE | `NOOP` every ~29 minutes (before the server's 30-minute IDLE timeout). |
| Telegram long-poll | Server-held request, client just dials again after each timeout. |

A plugin that has only a stdout keepalive but no protocol-level keepalive
will appear healthy to Nebo while silently failing to receive events
from its upstream. Implement both.

---

## Bridge Stdin Protocol

| op | Required fields | Optional fields | When to expect it |
|---|---|---|---|
| `reply` | `channel`, `text` | `thread_ts`, `user_ts`, `files`, `username` | After an inbound message: the agent has produced a response. Bridge clears any working-indicator reaction it placed on the user's message (using `user_ts`), then posts a **fresh** message in the thread. Posting fresh (instead of editing a placeholder) preserves notifications — see Working Indicator Pattern below. |
| `post` | `channel`, `text` | `thread_ts`, `files`, `username` | Agent posts unsolicited (e.g. a scheduled briefing fires, a workflow output is fanned out). Bridge always posts a new message. |
| `upload` | `channel`, `path` | `thread_ts`, `caption` | Agent attaches a file without a text body. Bridge uploads via the platform's file API. |
| `dm` | `user`, `text` | `files`, `username` | Agent direct-messages a specific user. Bridge resolves the DM channel (e.g. Slack's `conversations.open`) before posting. |

### Working Indicator Pattern

Channel plugins SHOULD give the user immediate feedback that the agent has received their message — but **never** by posting a placeholder message that the agent will later edit.

**Why edits are wrong:** Slack (and most messaging platforms) do not send mobile/desktop notifications for edited messages. A `_Thinking..._` placeholder that becomes the agent's response via `chat.update` arrives silently — users have to be actively watching the channel to know the agent responded. We shipped this pattern once and it cost us notifications on every reply. Don't.

**Why typing indicators don't help:** Slack does not expose a typing API for bots. The Web API has no `setTyping` endpoint; Socket Mode only RECEIVES `user_typing` events; the legacy RTM API that had one is deprecated. This is a platform limitation across most messaging APIs — Discord and Teams have the same constraint for bots.

**The pattern:** add a 👀 (or platform-equivalent) reaction to the user's message on inbound, then remove the reaction and post the response as a **fresh** message on reply. The reaction provides immediate feedback; the fresh post preserves notification delivery.

Slack reference:
- Inbound handler: `client.reactions_add(channel, ts, "eyes")` — eyes is Slack convention for "I saw this and I'm working on it" (Slackbot itself uses it on auto-acked messages).
- Stdout event includes `ts` (the user message's timestamp).
- Nebo echoes `ts` back as `user_ts` on the `reply` op.
- Reply handler: `client.reactions_remove(channel, user_ts, "eyes")` (best-effort) then `chat.postMessage` — a brand-new message in the same thread.
- Required scope: `reactions:write`. If missing, the bridge logs a warning and skips the reaction — responses still post, just without the immediate indicator.

Plugins targeting other platforms should pick the equivalent. Discord: react with `:eyes:` via `PUT /channels/{channel.id}/messages/{message.id}/reactions/{emoji}/@me`. Teams: there's no clean equivalent — accept that the working indicator is best-effort across platforms and that the canonical UX is the fresh-post notification, not the indicator.

The `files` array carries file refs by absolute path on the host running Nebo:

```json
"files": [
  { "path": "/Users/me/Desktop/report.pdf", "name": "Q4-Report.pdf", "title": "Quarterly report" }
]
```

The bridge reads file bytes itself — Nebo does not stream bytes through stdin. The bridge plugin already has the credentials and API client for its upstream platform; reading a local file is one more `std::fs::read` away.

---

## Where ops come from

| op source | Triggered by | Channel / target chosen by |
|---|---|---|
| `reply` | Agent finishes responding to an inbound message picked up on the bridge's stdout | Echoed back from inbound payload (channel, thread_ts, placeholder_ts) |
| `post` | Agent calls `plugin(resource: "<slug>", command: "post --channel <id> --text <body>")` directly, OR a cron job with a captured `ChannelContext` fires and the scheduler routes its response back here | Agent supplies `--channel` / `--thread_ts`, or scheduler restores them from the saved cron job context |
| `upload` | Agent calls `plugin(resource: "<slug>", command: "upload --channel <id> --path <abs>")` | Agent's args |
| `dm` | Agent calls `plugin(resource: "<slug>", command: "dm --user <id> --text <body>")` | Agent's args |

Nebo's plugin tool routes these calls through the running bridge's stdin — see `crates/tools/src/plugin_tool.rs::handle_exec` — by looking up the bridge in `AppState.channel_bridges` keyed by `{agent_id}:{plugin_slug}`. If no bridge is registered for the current agent, plugin_tool returns a structured error pointing the user at Settings → Channels. **There is no fallback to a one-shot CLI invocation.** That fallback IS the competing pathway we eliminated.

---

## How AppState routes ops

The handle that connects the agent side to the bridge stdin lives in `AppState.channel_bridges`:

- **Type**: `Arc<RwLock<HashMap<String, ChannelBridgeHandle>>>` (re-exported as `tools::ChannelBridgeRegistry`)
- **Key format**: `{agent_id}:{plugin_slug}` — see `tools::channel_bridge_key`
- **Producer**: `agent_worker::channel_loop` registers a handle when it spawns the bridge sidecar, removes it on child exit / cancel
- **Consumer**: `plugin_tool::handle_exec` looks up the handle and writes the op as one NDJSON line on the bridge's stdin
- **Secondary producer**: the scheduler (`crates/server/src/scheduler.rs`) writes `op: "post"` directly when a cron job that captured its originating `ChannelContext` fires — bypassing the plugin tool because the agent's response is already in hand

This is the *only* place real-time channel ops live. Plugin authors don't interact with the registry directly — they just keep the bridge process alive and dispatch on `op`.

---

## Multi-Agent Support

Channel plugins support two models, selected by the `channel.shared` flag in `plugin.json`. The default is **one bridge per agent**; setting `shared: true` switches to **one shared bridge for all agents**.

### Default — one bridge per agent (`shared: false`)

Each agent that enables your plugin gets its own bridge process with its own env (its own `SLACK_BOT_TOKEN`, etc.). Nebo's `channel_loop` spawns one bridge per `(agent_id, plugin_slug)` and registers it in `AppState.channel_bridges` under the matching key. Two agents using the same Slack workspace get two distinct bridges — that's by design, since they may have different bot identities, different scopes, or different rate-limit budgets.

There's no plugin-side multiplexing required. Your bridge reads `SLACK_BOT_TOKEN` from env on startup; if Nebo spawns multiple bridges for multiple agents, each instance gets its own token.

### Shared — one bridge, route by name (`shared: true`)

When the manifest declares `channel.shared: true`, Nebo spawns a **single** bridge process shared across every agent that has the plugin enabled, rather than one per agent. This fits platforms where a single connection/identity serves all agents (a single bot token, a shared workspace). Nebo's `shared_channel_loop` manages this bridge through a `SharedBridgeRegistry` (`crates/agent/src/agent_worker.rs`).

In this model:

- Inbound platform events carry the target agent's **name**, and Nebo routes each message to the matching agent rather than to a per-agent bridge.
- Each agent's replies are posted with its own display identity, even though they flow through the one shared bridge process.
- Your bridge still uses the same NDJSON stdin/stdout protocol — the difference is on Nebo's side (one process, name-based routing) rather than in the op shapes.

Use the default per-agent model unless your platform genuinely has one connection for all agents; set `shared: true` only then.

---

## Anti-Patterns

Don't do these:

| Anti-pattern | Why it's wrong |
|---|---|
| Adding a CLI subcommand like `slack post` / `slack upload` that talks to the platform | Competes with the bridge. Two processes hitting the same Slack workspace race each other — we've watched four orphan slack bridges all post "_Thinking..._" for one inbound message. CLI is for stateless local ops only. |
| Spawning a gRPC server inside the bridge | Over-engineered. NDJSON on stdin/stdout is the canonical Nebo plugin protocol; adding gRPC creates a second pathway and breaks Nebo's process supervision. |
| Reading file bytes from Nebo and shipping them over stdin | Use file paths. The bridge already has API access; opening a local file is cheaper and more reliable than streaming bytes through pipes. |
| Treating `op` as optional / falling back to "reply" | Nebo 0.10+ always sends `op`. Lines without it are logged and skipped — silent dispatch was the failure mode we eliminated. |
| Hardcoding channel IDs in your bridge instead of taking them from each op | Breaks Scenario 3 (agent posts to an arbitrary channel). The bridge is a pipe, not a policy. |

---

## See Also

- [Plugins](plugins.md) — general plugin authoring guide
- [Packaging](packaging.md) — how to publish a plugin
- `crates/tools/src/channel_bridge.rs` — registry types
- `crates/agent/src/agent_worker.rs::channel_loop` — bridge spawn + stdin forwarder
- `crates/tools/src/plugin_tool.rs::route_through_bridge` — op routing
- `crates/server/src/scheduler.rs::execute_agent_channel_bound` — scheduler-side `op: "post"` write
