# Channel Plugins

A **channel plugin** is a plugin that connects a Nebo agent to a messaging surface — Slack, Discord, Microsoft Teams, IRC, etc. Agents can listen for inbound messages, reply with text, and **upload files** to the channel.

This guide describes the conventions a channel plugin MUST follow so that Nebo can route messages, attach files, and thread replies consistently across platforms. The convention exists because the agent's mental model is **"use the channel plugin for channel things"** — and Nebo enforces that by making sure all channel operations live in the plugin's own CLI rather than in some out-of-band Nebo tool.

---

## Required Subcommands

A channel plugin MUST expose these subcommands. Names and flags are NOT optional — Nebo's runtime, docs, and agent system prompts assume them.

| Subcommand | Purpose | Lifecycle |
|---|---|---|
| `bridge --listen` | Long-running bidirectional channel bridge. Reads inbound messages from the platform and prints them as NDJSON on stdout; reads outbound reply NDJSON from stdin and posts them. | Long-running |
| `upload --path <file> [--channel <id>] [--thread_ts <id>] [--caption <text>]` | Upload a local file to a channel. Uses the plugin's own API client + auth token. | One-shot |
| `auth login` / `auth status` / `auth logout` | Authentication management. | One-shot |

Optional but recommended:

| Subcommand | Purpose |
|---|---|
| `init` | Initialize local state (DB, default workspace) |
| `doctor` | Diagnostics |
| `sync` | Archive workspace messages |
| `search`, `users`, `channels` | Read-only queries |

---

## The Bridge Process

The `bridge --listen` process is how messages flow in real time. Nebo spawns one bridge per agent that has the plugin enabled, and supervises the process for its lifetime.

### stdout — inbound (plugin → Nebo)

NDJSON, one event per line, flushed after each write.

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

Required fields: `text`, `channel`. Recommended: `user`, `thread_ts`, `placeholder_ts` (the timestamp of a "_Thinking..._" placeholder the bridge posted, so Nebo can update it with the final reply).

### stdin — outbound (Nebo → plugin)

NDJSON, one reply per line. The bridge reads these and posts them to the channel — typically by updating the `placeholder_ts` message with the agent's response.

```json
{
  "channel": "C1234567890",
  "thread_ts": "1700000000.123456",
  "placeholder_ts": "1700000061.345678",
  "user": "U0987654321",
  "text": "Here's the Q4 summary: ...",
  "username": "Nebo Assistant"
}
```

**Replies carry text only.** Files are NOT included in the reply payload — see [File Uploads](#file-uploads) below.

---

## File Uploads

This is the part publishers most often get wrong, so read carefully.

**Rule:** file uploads are handled by your plugin's `upload` CLI subcommand. They are NOT routed through the bridge stdin reply.

### Why this convention

The agent that wants to upload a file calls `plugin(resource: "yourplugin", action: "exec", command: "upload --path /abs/file.png")`. That:

1. Keeps the agent's mental model clean — "use the channel's plugin for channel things"
2. Lets your plugin use its own existing API client + auth token (no IPC needed)
3. Avoids a separate "send-this-file" Nebo tool that would compete with your plugin
4. Works identically across channel plugins (Slack, Discord, etc.) once each publishes an `upload` subcommand

### Environment variables Nebo injects

When a tool call is triggered by an inbound channel message, Nebo injects these env vars into every plugin invocation:

| Variable | Source | Example |
|---|---|---|
| `NEBO_CHANNEL_KIND` | Plugin slug | `slack` |
| `NEBO_CHANNEL_ID` | Inbound message `channel` field | `C1234567890` |
| `NEBO_THREAD_TS` | Inbound message `thread_ts` (optional) | `1700000000.123456` |

Your `upload` subcommand SHOULD default `--channel` to `$NEBO_CHANNEL_ID` and `--thread_ts` to `$NEBO_THREAD_TS` when those flags aren't passed explicitly. That way the agent only needs to provide `--path`:

```
slack upload --path /abs/file.png
```

…and the file lands in the right channel + thread automatically.

### Required behavior

The `upload` subcommand MUST:

- Accept `--path <abs-path>` (required)
- Accept `--channel <id>` (optional; default to `$NEBO_CHANNEL_ID`)
- Accept `--thread_ts <id>` (optional; default to `$NEBO_THREAD_TS`)
- Accept `--caption <text>` (optional)
- Reject relative paths or missing files with a clear error
- Use the plugin's own authenticated API client (no calling back into Nebo)
- Print JSON to stdout on success:
  ```json
  {"ok": true, "channel": "C123", "thread_ts": "...", "filename": "report.pdf", "size": 12345, "caption": "Q4 report"}
  ```
- Print a structured JSON error to stderr on failure (Nebo's plugin tool surfaces this to the agent)

### Reference: Slack plugin

See `crates/slack-cli/src/commands/upload.rs` in the Slack plugin repo for a reference implementation. The full call from the agent looks like:

```
plugin(
  resource: "slack",
  action: "exec",
  command: "upload --path /Users/me/Desktop/report.pdf --caption \"Q4 report\""
)
```

Nebo injects `NEBO_CHANNEL_ID` + `NEBO_THREAD_TS`, the plugin uses its `SLACK_BOT_TOKEN` to call `files.uploadV2`, and the file appears in the right Slack thread.

---

## Multi-Bot Support

Nebo supports multiple bots per channel platform two ways:

1. **Per-agent credentials** — each agent that enables your plugin gets its own bridge process with its own env (its own `SLACK_BOT_TOKEN`, for example). This is the default when an agent supplies plugin-specific config.
2. **Shared bridge** — one bridge per plugin instance, with Nebo routing inbound messages to whichever agent's name matches. Use this when one workspace serves several agents.

Your plugin does NOT need to multiplex bots internally — Nebo handles that by spawning multiple plugin processes with different env. The `upload` subcommand reads its credentials from env on each invocation, so per-bot uploads just work.

---

## Anti-Patterns

Don't do these:

| Anti-pattern | Why it's wrong |
|---|---|
| Routing file uploads through bridge stdin (passing `files: [...]` in the reply) | Creates a competing pathway with `upload`. The agent then has two ways to attach files, and picks the wrong one. |
| Reading file bytes from Nebo and posting via a Nebo-side tool | Forces every channel plugin's upload to flow through one Nebo tool. Doesn't scale across plugins. |
| Adding a new top-level Nebo tool (e.g. `message channel attach`) for uploads | Same problem — competes with the per-plugin `upload`. Don't. |
| Spawning a gRPC server in the bridge to accept upload requests | Over-engineered. The plugin already has API client + auth in one binary. CLI subcommand is the right granularity. |
| Hardcoding `--channel` instead of reading `NEBO_CHANNEL_ID` | Breaks the convention; the agent must then look up channel IDs, which it doesn't have natively. |

---

## See Also

- [Plugins](plugins.md) — general plugin authoring guide
- [Packaging](packaging.md) — how to publish a plugin
