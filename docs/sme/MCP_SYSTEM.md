# MCP System — Deep Dive Reference

> Last verified: 2026-03-31 against Rust codebase
> Audience: Claude Code SME context

## What Is It

Nebo's MCP (Model Context Protocol) system connects to external MCP servers over HTTP, lists their tools, and makes them available to the agent via a unified `mcp()` STRAP domain tool. It also exposes Nebo itself as an MCP server for integration with Claude Desktop, Cursor, and other MCP clients.

Two distinct subsystems:
1. **MCP Client** — Nebo connects TO external MCP servers (monument.sh, neboloop, etc.)
2. **MCP Server** — External tools connect TO Nebo via JSON-RPC 2.0

## Architecture Overview

```
                    ┌──────────────────────────────────────────┐
                    │              External MCP Servers         │
                    │  (monument.sh, neboloop.com/mcp, etc.)   │
                    └──────────────┬───────────────────────────┘
                                   │ JSON-RPC 2.0 over HTTP
                    ┌──────────────▼───────────────────────────┐
                    │          McpClient (client.rs)            │
                    │  • list_tools() — tools/list              │
                    │  • call_tool()  — tools/call              │
                    │  • OAuth discovery, token refresh          │
                    │  • Token encrypt/decrypt                   │
                    │  • SSE + raw JSON response parsing        │
                    └──────────────┬───────────────────────────┘
                                   │
                    ┌──────────────▼───────────────────────────┐
                    │           Bridge (bridge.rs)              │
                    │  • Connection tracking per integration    │
                    │  • sync_all() / connect() / disconnect()  │
                    │  • Tool naming: mcp__{slug}__{tool}       │
                    │  • find_integration_for_tool()            │
                    └──────────────┬───────────────────────────┘
                                   │
         ┌─────────────────────────┼─────────────────────────┐
         │                         │                         │
┌────────▼─────────┐  ┌───────────▼──────────┐  ┌──────────▼──────────┐
│  McpTool (STRAP) │  │  integrations.rs     │  │  Startup sync       │
│  mcp(server,     │  │  REST API handlers   │  │  (lib.rs:581-628)   │
│    resource,     │  │  CRUD + OAuth flow   │  │  reconnect with     │
│    action, ...)  │  │  + connect/test      │  │  stored tokens      │
└──────────────────┘  └──────────────────────┘  └─────────────────────┘

         ┌──────────────────────────────────────────────────┐
         │            MCP SERVER (nebo as server)           │
         │  POST /agent/mcp — JSON-RPC 2.0 handler         │
         │  • tools/list — all Nebo tools + "nebo" service  │
         │  • tools/call — execute any tool                 │
         │  • "nebo" service: chat, sessions, events        │
         └──────────────────────────┬───────────────────────┘
                                    │
         ┌──────────────────────────▼───────────────────────┐
         │         McpStdioBridge (mcp_serve.rs)            │
         │  stdin ↔ HTTP bridge for Claude Desktop/Cursor   │
         │  `nebo mcp serve`                                │
         └──────────────────────────────────────────────────┘
```

## Crate Structure

**`crates/mcp/`** — 4 source files, pure library (no server deps):

| File | Lines | Purpose |
|------|-------|---------|
| `lib.rs` | 8 | Re-exports: `McpClient`, `Bridge`, types, crypto |
| `types.rs` | 79 | `McpError`, `McpToolDef`, `McpToolResult`, `OAuthTokens`, `OAuthMetadata`, `RefreshResult`, `ConnectionStatus` |
| `client.rs` | 385 | HTTP client: JSON-RPC, OAuth discovery, token refresh, SSE parsing |
| `bridge.rs` | 256 | Connection management, tool naming, `ProxyToolRegistry` trait |
| `crypto.rs` | 156 | AES-256-GCM encryption for credentials, key resolution |

**Dependencies:** `reqwest`, `tokio`, `serde`/`serde_json`, `aes-gcm`, `sha2`, `rand`, `base64`, `hex`, `tracing`, `thiserror`

## MCP Client (client.rs)

### McpClient Struct

```rust
pub struct McpClient {
    http: reqwest::Client,          // 30s timeout, "nebo-mcp/1.0" UA
    encryptor: Arc<Encryptor>,      // AES-256-GCM for token storage
    sessions: RwLock<HashMap<String, Session>>,  // integration_id → Session
}

struct Session {
    server_url: String,
    tokens: Option<OAuthTokens>,
    _tools: Vec<McpToolDef>,        // cached tool list
}
```

### Key Methods

**`discover_oauth(server_url)`** — RFC 8414 well-known discovery
- Extracts origin from URL: `https://monument.sh/mcp` → `https://monument.sh`
- GETs `{origin}/.well-known/oauth-authorization-server`
- Returns `OAuthMetadata` (auth/token/registration/revocation endpoints)

**`list_tools(integration_id, server_url, access_token)`** — JSON-RPC `tools/list`
- POSTs to server URL directly (Streamable HTTP, not sub-paths)
- Headers: `Content-Type: application/json`, `Accept: application/json, text/event-stream`
- Bearer auth if token provided
- Parses response as raw JSON or SSE (extracts last `data:` line)
- Extracts tools from `body.result.tools` or `body.tools`
- Caches tools in session
- **This is where the 401 errors originate** — line 105-111

**`call_tool(integration_id, tool_name, input)`** — JSON-RPC `tools/call`
- Reads session from cache (fails if no session → "not found")
- Sends to cached server_url with cached bearer token
- Parses JSON-RPC response content blocks (type "text" only)
- Returns `McpToolResult { content, is_error }`

**`refresh_token(endpoint, client_id, secret, refresh_token)`** — RFC 6749 token refresh
- Form-encoded POST with `grant_type=refresh_token`
- 15s timeout
- Returns `RefreshResult { access_token, refresh_token, expires_in, scope }`

**`update_session_token(integration_id, tokens)`** — Updates in-memory session after refresh

### SSE Parsing

```rust
fn parse_sse_json(text: &str) -> Result<serde_json::Value, McpError>
```
- Scans all lines for `event:` and `data:` prefixes
- Returns JSON from the **last** `data:` line
- Falls back to error if no data lines found

## Bridge (bridge.rs)

### Connection Tracking

```rust
struct Connection {
    integration_id: String,
    server_slug: String,
    tool_names: Vec<String>,      // mcp__monument_sh__project
    original_names: Vec<String>,  // project, todo, comment
}

pub struct Bridge {
    connections: Mutex<HashMap<String, Connection>>,
    client: Arc<McpClient>,
    registry: Arc<dyn ProxyToolRegistry>,  // NOTE: not actually used for registration
}
```

### Tool Naming

```rust
fn make_tool_name(server_type: &str, original: &str) -> String {
    format!("mcp__{}__{}",
        server_type.to_lowercase().replace(' ', "_"),
        original.to_lowercase().replace(' ', "_"))
}
```
Examples: `mcp__monument_sh__project`, `mcp__slack__send_message`

**Important:** Despite tracking tool names and having `ProxyToolRegistry`, the bridge does NOT actually register proxy tools in the registry. Tools are tracked in Connection structs only. All tool access goes through the single `mcp()` STRAP tool.

### Key Methods

**`sync_all(integrations)`** — Full reconciliation
- Disconnects stale (enabled in bridge but not in DB list)
- Connects new (enabled in DB, not yet in bridge)
- Skips OAuth integrations without completed auth (`connection_status.is_none()`)
- **Note:** passes `None` for access_token — relies on startup flow to handle tokens

**`connect(id, server_type, url, token)`**
- Disconnects existing connection first
- Calls `client.list_tools()`
- Tracks connection with tool name mappings

**`disconnect(id)`** — Removes from connections map, closes client session

**`connected_tools()`** → `Vec<(slug, Vec<original_name>)>`
- Uses `try_lock()` — returns empty vec if mutex is locked (non-blocking)
- Used by McpTool to build dynamic description

**`find_integration_for_tool(server_slug, tool_name)`** → `Option<integration_id>`
- Fuzzy match: `c.server_slug == slug || c.server_slug.contains(slug)`
- Exact match on tool name within that server

**`call_tool(id, name, input)`** — Delegates to `client.call_tool()`

## Encryption (crypto.rs)

### Encryptor

- **Algorithm:** AES-256-GCM (256-bit key, 12-byte random nonce, authenticated)
- **Storage format:** `nonce (12 bytes) + ciphertext + auth tag` → base64 for DB

### Key Resolution Priority

```
1. MCP_ENCRYPTION_KEY env → SHA-256 passphrase derivation
2. JWT_SECRET env → SHA-256 passphrase derivation
3. ~/.nebo/.mcp-key file → raw 32 bytes
4. Generate random → persist to ~/.nebo/.mcp-key
```

**Keyring integration** (in lib.rs): After resolving, key is stored in OS keyring (`auth::keyring::set`). On next startup, keyring is checked first (before the priority chain above).

### What Gets Encrypted

- OAuth access tokens (stored in `mcp_integration_credentials.credential_value`)
- OAuth refresh tokens (stored in `mcp_integration_credentials.refresh_token`)
- OAuth client secrets (stored in `mcp_integrations.oauth_client_secret`)
- PKCE code verifiers (stored in `mcp_integrations.oauth_pkce_verifier`)

## McpTool — STRAP Domain Tool (mcp_tool.rs)

### Registration

```rust
// In lib.rs startup:
let mcp_tool = tools::mcp_tool::McpTool::new(bridge.clone(), store.clone());
tool_registry.register(Box::new(mcp_tool)).await;
```

### Schema

```json
{
  "type": "object",
  "properties": {
    "server": { "type": "string", "description": "MCP server name" },
    "resource": { "type": "string", "description": "Tool name on the server" },
    "action": { "type": "string", "description": "Action to perform" }
  },
  "required": ["server", "resource"],
  "additionalProperties": true
}
```

### Execution Flow

1. Parse `server` + `resource` from input (both required)
2. Slugify server name: `monument.sh` → `monument_sh`
3. Match against `bridge.connected_tools()` (fuzzy: exact or contains)
4. Find integration_id via `bridge.find_integration_for_tool()`
5. Strip `server` and `resource` from input, pass rest as MCP arguments
6. **Proactive token refresh** if OAuth + expired (60s buffer)
7. Call `bridge.call_tool(integration_id, resource, remaining_input)`
8. **On 401 error:** attempt token refresh, retry once
9. On refresh failure: set connection_status=disconnected, return error with reconnect guidance

### Token Refresh Helper

```rust
pub async fn refresh_mcp_token(store, client, integration_id) -> Result<String, String>
```
Orchestrates: DB read OAuth config → decrypt client_secret → decrypt refresh_token → HTTP refresh → encrypt new tokens → DB write → update in-memory session

### Expiry Check

```rust
pub fn is_token_expired(expires_at: Option<i64>) -> bool
// True if now >= (expires_at - 60 seconds)
// False if no expiry info (assume valid)
```

## Server Startup Flow (lib.rs:540-628)

```
1. Resolve encryption key (keyring → env → file → generate)
2. Create McpClient with encryptor
3. Create Bridge with client + tool_registry
4. Set bridge on tool_registry (for ProxyToolRegistry access)
5. Register McpTool as STRAP tool
6. tokio::spawn async reconnection:
   a. list_mcp_integrations() from DB
   b. For each enabled integration with server_url:
      - Skip OAuth without completed auth
      - Decrypt stored OAuth token if auth_type=oauth
      - Slugify integration name for tool_prefix
      - bridge.connect(id, prefix, url, token)
      - Update connection_status in DB (connected/error)
```

**The 401 bug from the logs:** On startup reconnection, the stored OAuth token may be expired. The startup flow decrypts the stored token and passes it to `bridge.connect()`, which calls `list_tools()`. If the token is expired, the server returns 401. **The startup flow does NOT attempt token refresh before connecting** — it only decrypts the stored token. The `McpTool.maybe_refresh_token()` and retry-on-401 logic only applies to tool calls, not startup reconnection.

## REST API Handlers (integrations.rs)

### Endpoints

| Method | Path | Handler | Auth |
|--------|------|---------|------|
| GET | `/api/v1/integrations` | `list_integrations` | JWT |
| POST | `/api/v1/integrations` | `create_integration` | JWT |
| GET | `/api/v1/integrations/:id` | `get_integration` | JWT |
| PUT | `/api/v1/integrations/:id` | `update_integration` | JWT |
| DELETE | `/api/v1/integrations/:id` | `delete_integration` | JWT |
| POST | `/api/v1/integrations/:id/test` | `test_integration` | JWT |
| POST | `/api/v1/integrations/:id/connect` | `connect_integration` | JWT |
| GET | `/api/v1/integrations/:id/oauth-url` | `get_oauth_url` | JWT |
| GET | `/api/v1/integrations/oauth/callback` | `oauth_callback` | Public |
| GET | `/api/v1/integrations/registry` | `list_registry` | JWT |
| GET | `/api/v1/integrations/tools` | `list_tools` | JWT |
| GET | `/api/v1/mcp/servers` | `list_registry` | JWT |

### OAuth Flow (Full Sequence)

```
Frontend                    Nebo Server                  External MCP Server
   │                            │                              │
   │ GET /integrations/:id/     │                              │
   │     oauth-url              │                              │
   │───────────────────────────>│                              │
   │                            │ GET /.well-known/oauth-...   │
   │                            │─────────────────────────────>│
   │                            │<─────────────────────────────│
   │                            │  OAuthMetadata               │
   │                            │                              │
   │                            │ POST registration_endpoint   │
   │                            │─────────────────────────────>│ (DCR, if supported)
   │                            │<─────────────────────────────│
   │                            │  client_id, client_secret    │
   │                            │                              │
   │                            │ Generate PKCE (verifier +    │
   │                            │   challenge) + state         │
   │                            │ Encrypt & store in DB        │
   │                            │                              │
   │  { authUrl: "..." }        │                              │
   │<───────────────────────────│                              │
   │                            │                              │
   │ Open system browser ──────────────────────────────────────>│
   │                            │                              │
   │                            │ GET /integrations/oauth/     │
   │                            │     callback?code=X&state=Y  │
   │                            │<─────────────────────────────│ (redirect)
   │                            │                              │
   │                            │ Look up integration by state │
   │                            │ Decrypt PKCE verifier        │
   │                            │ POST token_endpoint          │
   │                            │─────────────────────────────>│
   │                            │<─────────────────────────────│
   │                            │  access_token, refresh_token │
   │                            │                              │
   │                            │ Encrypt & store tokens       │
   │                            │ bridge.connect() immediately │
   │                            │─────────────────────────────>│ (tools/list)
   │                            │<─────────────────────────────│
   │                            │                              │
   │  (Polling detects change)  │                              │
   │<───────────────────────────│                              │
```

### PKCE Implementation (RFC 7636)

- `generate_code_verifier()` — 32 random bytes → base64url (43 chars)
- `compute_code_challenge(verifier)` — SHA-256(verifier) → base64url
- `generate_state()` — 16 random bytes → base64url

### Dynamic Client Registration (RFC 7591)

```json
{
  "client_name": "Nebo Agent",
  "redirect_uris": ["http://localhost:27895/api/v1/integrations/oauth/callback"],
  "token_endpoint_auth_method": "none",
  "grant_types": ["authorization_code", "refresh_token"],
  "response_types": ["code"],
  "scope": "mcp:full offline_access"
}
```

### Connect Handler

`POST /api/v1/integrations/:id/connect`:
1. Get integration from DB
2. If OAuth: get credential, check expiry, refresh if needed (with fallback to possibly-expired token)
3. Slugify name for tool prefix
4. `bridge.connect()` → list tools
5. Update DB: `connection_status=connected`, `tool_count=N`
6. On error: `connection_status=error`, persist `last_error`

### Built-in Registry

`GET /api/v1/integrations/registry` returns 5 hardcoded stdio servers:
- filesystem, brave-search, github, sqlite, memory (all `npx -y @modelcontextprotocol/server-*`)

## MCP Server — Nebo as MCP Provider (mcp_server.rs)

### Endpoint

`POST /agent/mcp` — JSON-RPC 2.0, no authentication required

### Supported Methods

**`initialize`** — Returns protocol version and capabilities
```json
{
  "protocolVersion": "2025-03-26",
  "capabilities": { "tools": {}, "resources": {}, "prompts": {} },
  "serverInfo": { "name": "nebo", "version": "<cargo_version>" }
}
```

**`notifications/initialized`** — Returns empty success (HTTP can't be silent)

**`tools/list`** — Returns all registered Nebo tools + "nebo" service tool

**`tools/call`** — Two paths:
1. `name == "nebo"` → service tool dispatch
2. Any other name → `state.tools.execute()` with MCP context (auto-approval, user origin)

### "nebo" Service Tool

Provides agent chat and session management via MCP:

| Resource | Action | Description |
|----------|--------|-------------|
| `chat` | `send` | Send message to agent, collect full response |
| `sessions` | `list` | List all agent sessions |
| `<session_id>` | `history` | Get message history for session |
| `<session_id>` | `reset` | Clear session history |
| (any) | `emit` | Fire event to event bus |

**Chat execution:**
- Session key: `mcp-{session_id}` (default: `mcp-mcp-default`)
- Origin: `Origin::User`
- Channel: `"mcp"`
- Timeout: configurable (default 300s, max 600s), enforced by cancellation token watchdog
- **Auto-approves** all tool approval requests
- **Auto-answers** all ask requests with "yes"
- Collects: text, tool_call names, errors
- Returns response + `[Tools used: ...]` summary

## CLI MCP Serve (mcp_serve.rs)

### McpStdioBridge

Stdin-to-HTTP bridge for external MCP clients:

```
Claude Desktop / Cursor
    │ JSON-RPC stdin
    ▼
McpStdioBridge
    │ POST /agent/mcp
    ▼
Nebo Server (localhost:27895)
```

**Startup:** Health check (`GET /health`, 3 attempts, 5s timeout each)

**Loop:** Read line from stdin → POST to `/agent/mcp` → write response to stdout

**Tool filtering:** `--tools` (allowlist) and `--exclude-tools` (denylist) filter `tools/list` responses

### Config Generation

`nebo mcp config --target claude-desktop|cursor` outputs:
```json
{
  "mcpServers": {
    "nebo": {
      "command": "/path/to/nebo",
      "args": ["mcp", "serve"]
    }
  }
}
```

## Database Schema

### mcp_integrations (0018)

```sql
CREATE TABLE mcp_integrations (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    server_type TEXT NOT NULL,         -- "http", "stdio", "custom"
    server_url TEXT,                    -- MCP endpoint URL
    auth_type TEXT DEFAULT 'none',     -- "none", "api_key", "oauth"
    is_enabled INTEGER DEFAULT 1,
    connection_status TEXT,            -- "connected", "disconnected", "error"
    last_connected_at INTEGER,
    last_error TEXT,
    metadata TEXT,                     -- JSON blob
    created_at INTEGER,
    updated_at INTEGER,
    tool_count INTEGER DEFAULT 0       -- (added 0029)
);
```

**OAuth columns (added 0025):**
```sql
oauth_state TEXT,                      -- PKCE state parameter (flow in progress)
oauth_pkce_verifier TEXT,              -- encrypted code_verifier
oauth_client_id TEXT,                  -- from DCR or manual
oauth_client_secret TEXT,              -- encrypted (if any)
oauth_authorization_endpoint TEXT,
oauth_token_endpoint TEXT,
oauth_registration_endpoint TEXT
```

### mcp_integration_credentials (0018)

```sql
CREATE TABLE mcp_integration_credentials (
    id TEXT PRIMARY KEY,
    integration_id TEXT REFERENCES mcp_integrations(id),
    credential_type TEXT,              -- "api_key", "oauth_token"
    credential_value TEXT,             -- encrypted access_token
    refresh_token TEXT,                -- encrypted refresh_token
    expires_at INTEGER,                -- unix timestamp
    scopes TEXT,
    created_at INTEGER,
    updated_at INTEGER
);
```

### mcp_server_registry (0018)

Pre-populated known servers (Notion, GitHub, Linear, Slack) with icons, auth config, and API key URLs. Used by frontend to show available servers.

## Known Issues & Bugs

### 1. Startup Reconnection Ignores Token Expiry (ACTIVE — your 401 errors)

**Location:** `crates/server/src/lib.rs:597-605`

The startup reconnection flow decrypts the stored OAuth token and passes it directly to `bridge.connect()`. If the token is expired, `list_tools()` sends it anyway and gets 401. **No refresh is attempted before connecting.**

Fix: Check `is_token_expired()` on the credential before connecting. If expired, call `refresh_mcp_token()` first, then connect with the fresh token.

### 2. sync_all() Passes None for Token

**Location:** `crates/mcp/src/bridge.rs:101`

`sync_all()` always passes `None` for access_token when calling `connect()`. This means bridge-initiated reconnections (from `sync_bridge()` in handler code) won't send OAuth tokens. Only the startup flow and `connect_integration` handler properly retrieve tokens.

### 3. ProxyToolRegistry Trait Unused

The `Bridge` holds `Arc<dyn ProxyToolRegistry>` and `register_proxy()`/`unregister_proxy()` are defined, but `connect()`/`disconnect()` never call them. Tool registration is tracked internally only. The registry implementation exists in `registry.rs` (McpProxyTool) but is dead code.

### 4. Non-Blocking connected_tools()

`connected_tools()` and `find_integration_for_tool()` use `try_lock()`. If another operation holds the mutex, they return empty results. This could cause the McpTool to report "no servers connected" during reconnection.

### 5. No API Key Auth in Client

`call_tool()` only sends Bearer auth from session tokens. If auth_type is "api_key", the credential is never retrieved or sent. API key auth is only partially implemented (stored in DB but not used for tool calls).

## Frontend (app/src/routes/(app)/integrations/+page.svelte)

Single-page UI with:
- List of all integrations (name, auth type badge, connection status, tool count)
- Add Server modal (3-step wizard: URL → Auth Method → Confirm)
- Per-integration actions: Test, Connect, Enable/Disable toggle, Delete
- OAuth flow: opens system browser, polls for completion (3-minute timeout)
- Redirects from `/settings/integrations` and `/settings/mcp` and `/mcp`

## AppState Members

```rust
pub bridge: Arc<mcp::Bridge>,
pub mcp_context: Arc<tokio::sync::Mutex<tools::ToolContext>>,
```

## STRAP Documentation

File: `crates/agent/src/strap/mcp.txt`

```
Usage: mcp(server: "<server_name>", resource: "<tool>", action: "<action>", ...params)
Examples:
  mcp(server: "monument.sh", resource: "project", action: "list")
  mcp(server: "monument.sh", resource: "todo", action: "create", title: "Ship v2", todolist_id: "abc")
```

## Security Considerations

- All tokens encrypted at rest (AES-256-GCM)
- PKCE prevents authorization code interception
- OAuth state parameter prevents CSRF
- Token refresh with 60s expiry buffer
- On 401: single retry with refresh, then disconnect
- All MCP tool calls require user approval (`requires_approval() = true`)
- MCP server endpoint (`/agent/mcp`) has NO authentication — localhost only
- Auto-approval of all tool calls when accessed via MCP server (intended for local CLI)

## Key Files Quick Reference

| File | Purpose |
|------|---------|
| `crates/mcp/src/client.rs` | HTTP client, JSON-RPC, OAuth |
| `crates/mcp/src/bridge.rs` | Connection management |
| `crates/mcp/src/crypto.rs` | AES-256-GCM encryption |
| `crates/mcp/src/types.rs` | Error types, data models |
| `crates/tools/src/mcp_tool.rs` | STRAP domain tool |
| `crates/server/src/handlers/integrations.rs` | REST API (727 lines) |
| `crates/server/src/handlers/mcp_server.rs` | JSON-RPC server (410 lines) |
| `crates/server/src/lib.rs:540-628` | Startup initialization + reconnection |
| `crates/cli/src/mcp_serve.rs` | Stdio bridge for Claude Desktop/Cursor |
| `crates/db/src/queries/mcp_integrations.rs` | DB queries (312 lines) |
| `crates/db/migrations/0018_*.sql` | Main schema |
| `crates/db/migrations/0025_*.sql` | OAuth columns |
| `crates/db/migrations/0029_*.sql` | tool_count column |
| `app/src/routes/(app)/integrations/+page.svelte` | Frontend UI |
| `crates/agent/src/strap/mcp.txt` | STRAP prompt docs |
| `docs/publishers-guide/mcp.md` | Publisher guide |
