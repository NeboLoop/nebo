# MCP Integration Guide

Nebo is both an MCP client (connects to external MCP servers) and an MCP server (exposes its tools to external clients like Claude Desktop, Cursor, and Claude Code).

---

## Using Nebo as an MCP Server

### Quick Start

1. Start Nebo:
   ```bash
   nebo serve
   ```

2. Generate config for your client:
   ```bash
   # For Claude Desktop
   nebo mcp config --target claude-desktop

   # For Cursor
   nebo mcp config --target cursor
   ```

3. Copy the output into your client's config file:
   - **Claude Desktop:** `~/Library/Application Support/Claude/claude_desktop_config.json`
   - **Cursor:** `.cursor/mcp.json`

4. Restart your client. Nebo's tools will appear automatically.

### What Gets Exposed

When an MCP client connects, it sees:

**All Nebo domain tools** (STRAP pattern):
- `system` — file operations, shell commands, platform info
- `web` — HTTP requests, web search, browser control
- `bot` — task management, memory, sessions, context
- `loop` — DMs, channels, groups, topics
- `message` — owner messaging, SMS, notifications
- `event` — scheduling
- `app` — lifecycle, marketplace
- `desktop` — input, UI, windows, menus (macOS/Windows)
- `organizer` — mail, contacts, calendar, reminders (macOS/Windows)
- `skill` — dynamic per-skill tools

**The `nebo` service tool** — chat with Nebo's agent, manage sessions, emit events:
```
nebo(resource: "chat", action: "send", message: "What's on my calendar today?")
nebo(resource: "chat", action: "send", message: "Draft an email to John", session_id: "work")
nebo(resource: "sessions", action: "list")
nebo(resource: "work", action: "history")
nebo(resource: "work", action: "reset")
nebo(action: "emit", source: "my.custom.event", payload: {"key": "value"})
```

### Tool Filtering

Restrict which tools are exposed:

```bash
# Only expose system, web, and the nebo service tool
nebo mcp serve --tools system,web,nebo

# Expose everything except desktop and organizer
nebo mcp serve --exclude-tools desktop,organizer
```

### Sessions

MCP chat sessions are persistent. Each `nebo(resource: "chat", action: "send")` call can specify a `session_id` for conversation continuity:

- Default session: `mcp-default`
- Custom session: any string (prefixed with `mcp-` internally)
- Sessions survive server restarts (stored in SQLite)
- Reset a session: `nebo(resource: "<id>", action: "reset")`

### Timeouts

Chat calls have a default timeout of 300 seconds (5 minutes), configurable up to 600 seconds:

```
nebo(resource: "chat", action: "send", message: "...", timeout_secs: 600)
```

---

## Securing the MCP Endpoint

### Default: Open (Localhost Only)

By default, Nebo binds to `127.0.0.1:27895` and the MCP endpoint requires no authentication. This is safe for single-user desktop use.

### API Key Authentication

Set the `NEBO_MCP_API_KEY` environment variable to require authentication:

```bash
NEBO_MCP_API_KEY=my-secret-key nebo serve
```

MCP clients must then include the key in requests:
```
Authorization: Bearer my-secret-key
```

For the stdio bridge, the key is handled internally (bridge connects to localhost).

### Remote Access

Nebo refuses to bind to non-localhost addresses unless explicitly opted in:

```bash
# This will fail:
nebo serve --host 0.0.0.0

# This works (but warns if no API key set):
NEBO_ALLOW_REMOTE=true nebo serve --host 0.0.0.0

# Recommended for remote access:
NEBO_ALLOW_REMOTE=true NEBO_MCP_API_KEY=my-secret-key nebo serve --host 0.0.0.0
```

### Security Model

| Scenario | Auth Required | Notes |
|----------|---------------|-------|
| Localhost, no API key | No | Default, safe for desktop |
| Localhost, API key set | Yes | Extra protection |
| Remote, no API key | Blocked | Server refuses to start |
| Remote, API key set | Yes | Full protection |

MCP tool calls are auto-approved (no interactive prompts). The MCP context uses `Origin::Mcp` for audit trail, with the same tool access as a direct user.

---

## Connecting External MCP Servers to Nebo

Nebo can also connect TO external MCP servers and make their tools available to the agent.

### Via the Web UI

1. Open Nebo's web UI (`http://localhost:27895`)
2. Navigate to **Integrations**
3. Click **Add Server**
4. Enter the server URL and choose an auth method (None, API Key, or OAuth)
5. Click **Connect**

### Via the REST API

```bash
# Create an integration
curl -X POST http://localhost:27895/api/v1/integrations \
  -H "Content-Type: application/json" \
  -d '{"name": "My Server", "serverUrl": "https://my-mcp-server.com/mcp"}'

# Connect to it
curl -X POST http://localhost:27895/api/v1/integrations/{id}/connect
```

### Authentication Methods

**None** — No auth. For public or localhost MCP servers.

**API Key** — Stored encrypted. Sent as Bearer token on tool calls.

**OAuth 2.0** — Full flow with:
- RFC 8414 well-known discovery
- RFC 7591 Dynamic Client Registration (DCR)
- RFC 7636 PKCE (S256 challenge method)
- Automatic token refresh (60-second expiry buffer)
- Proactive refresh before tool calls + retry on 401

### How Connected Tools Appear

External MCP tools are namespaced and available via the `mcp()` STRAP tool:

```
mcp(server: "monument.sh", resource: "project", action: "list")
mcp(server: "monument.sh", resource: "todo", action: "create", title: "Ship v2")
```

Tool naming convention: `mcp__{server_slug}__{tool_name}`
Example: `mcp__monument_sh__project`, `mcp__brave_search__web_search`

### OAuth Flow

For OAuth-enabled MCP servers:

1. Call `GET /api/v1/integrations/{id}/oauth-url`
2. Open the returned `authUrl` in a browser
3. Authorize Nebo on the external server
4. The browser redirects to Nebo's callback (`/api/v1/integrations/oauth/callback`)
5. Nebo exchanges the code for tokens, encrypts and stores them
6. Connection is established automatically

Tokens are encrypted at rest with AES-256-GCM. Refresh happens automatically.

---

## Protocol Details

### JSON-RPC 2.0

Nebo's MCP server speaks JSON-RPC 2.0 over HTTP:

**Endpoint:** `POST /agent/mcp`

**Supported methods:**

| Method | Purpose |
|--------|---------|
| `initialize` | Handshake, returns protocol version `2025-03-26` and capabilities |
| `notifications/initialized` | Client acknowledgment |
| `tools/list` | List all available tools with schemas |
| `tools/call` | Execute a tool by name with arguments |

**Example: List tools**
```json
// Request
{"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}

// Response
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "tools": [
      {
        "name": "system",
        "description": "File operations, shell commands, platform info",
        "inputSchema": { "type": "object", "properties": { ... } }
      },
      { "name": "nebo", "description": "Chat with nebo's agent...", ... }
    ]
  }
}
```

**Example: Call a tool**
```json
// Request
{
  "jsonrpc": "2.0", "id": 2,
  "method": "tools/call",
  "params": {
    "name": "nebo",
    "arguments": {
      "resource": "chat",
      "action": "send",
      "message": "What time is it?"
    }
  }
}

// Response
{
  "jsonrpc": "2.0", "id": 2,
  "result": {
    "content": [{"type": "text", "text": "It's 2:30 PM PST.\n\n[Tools used: system]"}],
    "isError": false
  }
}
```

**Error codes:**

| Code | Meaning |
|------|---------|
| -32700 | Parse error (malformed JSON) |
| -32601 | Method not found |
| -32602 | Invalid params (missing tool name, etc.) |
| -32603 | Internal error (tool execution failure) |
| -32000 | Server error (connection, auth) |

### Transport

**Stdio (for Claude Desktop / Cursor):**
The `nebo mcp serve` command runs a stdio bridge that reads JSON-RPC from stdin and forwards to the HTTP endpoint. Tool filtering (`--tools`, `--exclude-tools`) is applied at the bridge level.

**HTTP (direct):**
Any HTTP client can POST to `/agent/mcp`. Set `NEBO_MCP_API_KEY` for authentication in production.

### Capabilities

Currently implemented:
- `tools` — list and call

Not yet implemented:
- `resources` — browse Nebo's files, settings, memory
- `prompts` — dynamic prompt templates
- `logging` — remote log level control
- `sampling` — delegate decisions to client

---

## Workflow Availability

MCP tools from external servers are available in conversational context but **not** in workflow activities. Workflow activities reference skills by qualified name (`@org/skills/name`) or interface binding. MCP tools use the `mcp()` STRAP tool pattern and are excluded from the workflow skill-filtering system.

---

## Troubleshooting

**"Cannot connect to Nebo"** — Make sure the Nebo server is running (`nebo serve` or `make dev`).

**401 errors on startup** — OAuth token may have expired while Nebo was stopped. The startup flow attempts refresh automatically; if refresh also fails (revoked token, server down), reconnect manually via the web UI.

**"No MCP servers connected"** — Brief transient during reconnection. The bridge uses `try_lock()` for non-blocking reads; during heavy reconnection the tool list may appear empty for a moment.

**Tools not showing up in Claude Desktop** — Verify config with `nebo mcp config --target claude-desktop`. Restart Claude Desktop after changing config. Check that `nebo mcp serve` can reach the server (health check must pass).

**OAuth callback not working** — The callback URL must be `http://localhost:{port}/api/v1/integrations/oauth/callback`. Ensure Nebo is running on the expected port (default 27895).
