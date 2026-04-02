# MCP System — Deep Dive Reference

> Last verified: 2026-04-02 against Rust codebase
> Audience: Claude Code SME context

## What Is It

Nebo's MCP (Model Context Protocol) system has two roles:
1. **MCP Client** — Nebo connects TO external MCP servers (monument.sh, neboloop, etc.) and exposes their tools to the agent via a unified `mcp()` STRAP domain tool.
2. **MCP Server** — External clients (Claude Desktop, Cursor, Claude Code) connect TO Nebo via JSON-RPC 2.0 and get access to all Nebo tools + a special `nebo` service tool for chat/sessions/events.

## Architecture Overview

```
                    +-------------------------------------------------+
                    |            External MCP Servers                  |
                    |  (monument.sh, neboloop.com/mcp, etc.)          |
                    +----------------------+--------------------------+
                                           | JSON-RPC 2.0 over HTTP
                    +----------------------v--------------------------+
                    |          McpClient (client.rs)                   |
                    |  list_tools(), call_tool(), OAuth, SSE parsing   |
                    +----------------------+--------------------------+
                                           |
                    +----------------------v--------------------------+
                    |           Bridge (bridge.rs)                    |
                    |  Connection tracking, tool naming, sync_all()   |
                    +--------+-------------+-------------------------+
                             |             |
                +------------v---+  +------v-----------+  +----------v----------+
                | McpTool (STRAP)|  | integrations.rs  |  | Startup sync        |
                | mcp(server,   |  | REST API handlers |  | (lib.rs:598-660)    |
                |  resource,    |  | CRUD + OAuth flow |  | reconnect with      |
                |  action, ...) |  | + connect/test    |  | stored tokens       |
                +---------------+  +-------------------+  +---------------------+

                +-----------------------------------------------------+
                |            MCP SERVER (nebo as server)               |
                | POST /agent/mcp - JSON-RPC 2.0 handler              |
                | Middleware: mcp_api_key_auth (opt-in)                |
                | tools/list - all Nebo tools + "nebo" service        |
                | tools/call - execute any tool or chat via "nebo"    |
                +-------------------------+---------------------------+
                                          |
                +-------------------------v---------------------------+
                |         McpStdioBridge (mcp_serve.rs)               |
                |  stdin <-> HTTP bridge for Claude Desktop/Cursor    |
                |  `nebo mcp serve [--tools ...] [--exclude-tools]`   |
                +-----------------------------------------------------+
```

## Crate Structure

**`crates/mcp/`** — 4 source files, pure library (no server deps):

| File | Lines | Purpose |
|------|-------|---------|
| `lib.rs` | 9 | Re-exports: `McpClient`, `Bridge`, types, crypto |
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
    sessions: RwLock<HashMap<String, Session>>,  // integration_id -> Session
}

struct Session {
    server_url: String,
    tokens: Option<OAuthTokens>,
    _tools: Vec<McpToolDef>,        // cached tool list
}
```

### Key Methods

**`discover_oauth(server_url)`** — RFC 8414 well-known discovery
- Extracts origin from URL: `https://monument.sh/mcp` -> `https://monument.sh`
- GETs `{origin}/.well-known/oauth-authorization-server`
- Returns `OAuthMetadata` (auth/token/registration/revocation endpoints)

**`list_tools(integration_id, server_url, access_token)`** — JSON-RPC `tools/list`
- POSTs to server URL directly (Streamable HTTP)
- Headers: `Content-Type: application/json`, `Accept: application/json, text/event-stream`
- Bearer auth if token provided
- Parses response as raw JSON or SSE (extracts last `data:` line)
- Caches tools in session

**`call_tool(integration_id, tool_name, input)`** — JSON-RPC `tools/call`
- Reads session from cache (fails if no session -> "not found")
- Sends to cached server_url with cached bearer token
- Parses content blocks (type "text" only, joined with newlines)
- Returns `McpToolResult { content, is_error }`

**`refresh_token(endpoint, client_id, secret, refresh_token)`** — RFC 6749 token refresh
- Form-encoded POST with `grant_type=refresh_token`, 15s timeout
- Returns `RefreshResult { access_token, refresh_token, expires_in, scope }`

**`update_session_token(integration_id, tokens)`** — Updates in-memory session after refresh

### SSE Parsing

```rust
fn parse_sse_json(text: &str) -> Result<serde_json::Value, McpError>
```
Scans all lines for `data:` prefix, returns JSON from the last `data:` line.

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
    registry: Arc<dyn ProxyToolRegistry>,  // exists but unused for registration
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

### Key Methods

| Method | Purpose |
|--------|---------|
| `sync_all(integrations)` | Disconnect stale, connect new. Skips OAuth without completed auth. |
| `connect(id, server_type, url, token)` | Disconnect existing, list tools, track connection |
| `disconnect(id)` | Remove from map, close client session |
| `connected_tools()` | `Vec<(slug, Vec<original_name>)>` — uses `try_lock()`, non-blocking |
| `find_integration_for_tool(slug, name)` | Fuzzy match: exact or contains |
| `call_tool(id, name, input)` | Delegate to `client.call_tool()` |
| `connected_ids()` | List connected integration IDs |
| `client()` | Access underlying McpClient for OAuth/encryption |

## Encryption (crypto.rs)

**Algorithm:** AES-256-GCM (256-bit key, 12-byte random nonce, authenticated)
**Storage format:** `nonce (12 bytes) + ciphertext + auth tag` -> base64

### Key Resolution Priority

```
1. MCP_ENCRYPTION_KEY env -> SHA-256 passphrase derivation
2. JWT_SECRET env -> SHA-256 passphrase derivation
3. ~/.nebo/.mcp-key file -> raw 32 bytes
4. Generate random -> persist to ~/.nebo/.mcp-key
```

After resolving, key is stored in OS keyring (`auth::keyring::set`). On next startup, keyring is checked first.

### What Gets Encrypted

- OAuth access tokens (`mcp_integration_credentials.credential_value`)
- OAuth refresh tokens (`mcp_integration_credentials.refresh_token`)
- OAuth client secrets (`mcp_integrations.oauth_client_secret`)
- PKCE code verifiers (`mcp_integrations.oauth_pkce_verifier`)

## McpTool — STRAP Domain Tool (mcp_tool.rs)

### Registration

```rust
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
2. Slugify server name: `monument.sh` -> `monument_sh`
3. Match against `bridge.connected_tools()` (fuzzy: exact or contains)
4. Find integration_id via `bridge.find_integration_for_tool()`
5. Strip `server` and `resource` from input, pass rest as MCP arguments
6. **Proactive token refresh** if OAuth + expired (60s buffer)
7. Call `bridge.call_tool(integration_id, resource, remaining_input)`
8. **On 401:** attempt token refresh, retry once
9. On refresh failure: set `connection_status=disconnected`, return error with reconnect guidance

### Token Refresh Helper

```rust
pub async fn refresh_mcp_token(store, client, integration_id) -> Result<String, String>
```
Orchestrates: DB read OAuth config -> decrypt client_secret -> decrypt refresh_token -> HTTP refresh -> encrypt new tokens -> DB write -> update in-memory session

### Expiry Check

```rust
pub fn is_token_expired(expires_at: Option<i64>) -> bool
// True if now >= (expires_at - 60 seconds)
// False if no expiry info (assume valid)
```

## Server Startup Flow (lib.rs:580-660)

```
1. Resolve encryption key (keyring -> env -> file -> generate)
2. Init credential system with encryptor
3. Create McpClient with encryptor
4. Create Bridge with client + tool_registry
5. Register McpTool as STRAP tool
6. Create MCP ToolContext (Origin::Mcp, user_id="mcp-client", session_key="mcp")
7. tokio::spawn async reconnection:
   a. list_mcp_integrations() from DB
   b. For each enabled integration with server_url:
      - Skip OAuth without completed auth
      - If OAuth: check token expiry, refresh if needed (with fallback to stored token)
      - Decrypt stored OAuth token
      - Slugify integration name for tool_prefix
      - bridge.connect(id, prefix, url, token)
      - Update connection_status in DB (connected/error)
```

## MCP Server — Nebo as MCP Provider (mcp_server.rs)

### Endpoint

`POST /agent/mcp` — JSON-RPC 2.0

### Security

**Middleware:** `mcp_api_key_auth` (opt-in)
- If `NEBO_MCP_API_KEY` env var is NOT set -> endpoint is open (localhost use case)
- If set -> requires `Authorization: Bearer <key>` header, returns JSON-RPC error on mismatch

**Origin tracking:** MCP requests use `Origin::Mcp` (not `Origin::User`)
- `Origin::Mcp` has no entries in the default deny list, so it has full tool access (same as User)
- Can be restricted by adding Mcp to `default_origin_deny_list()` in `policy.rs`

**Non-localhost binding:**
- If host is not `127.0.0.1`/`localhost`/`::1`, server refuses to start unless `NEBO_ALLOW_REMOTE=true` is set
- If remote access enabled without `NEBO_MCP_API_KEY`, a strong warning is printed

### Tool Execution Context

```rust
ToolContext {
    origin: Origin::Mcp,       // Dedicated MCP origin
    user_id: "mcp-client",     // Hardcoded for audit trail
    session_key: "mcp",        // Shared MCP session
    allowed_paths: vec![],     // Unrestricted
    entity_permissions: None,  // No restrictions
    resource_grants: None,     // No restrictions
}
```

### Supported Methods

**`initialize`** — Returns protocol version and capabilities
```json
{
  "protocolVersion": "2025-03-26",
  "capabilities": { "tools": {} },
  "serverInfo": { "name": "nebo", "version": "<cargo_version>" }
}
```

**`notifications/initialized`** — Returns empty success

**`tools/list`** — Returns all registered Nebo tools + "nebo" service tool

**`tools/call`** — Two paths:
1. `name == "nebo"` -> service tool dispatch
2. Any other name -> `state.tools.execute()` with MCP context

### "nebo" Service Tool

| Resource | Action | Description |
|----------|--------|-------------|
| `chat` | `send` | Send message to agent, collect full response (300-600s timeout) |
| `sessions` | `list` | List all agent sessions |
| `<session_id>` | `history` | Get message history for session |
| `<session_id>` | `reset` | Clear session history |
| (any) | `emit` | Fire event to event bus |

**Chat execution:**
- Session key: `mcp-{session_id}` (default: `mcp-mcp-default`)
- **Auto-approves** all tool approval requests
- **Auto-answers** all ask requests with "yes"
- Collects: text, tool_call names, errors
- Returns response + `[Tools used: ...]` summary

## CLI MCP Serve (mcp_serve.rs + main.rs)

### Commands

```bash
nebo mcp serve                           # Start stdio bridge
nebo mcp serve --tools system,web,bot    # Allowlist specific tools
nebo mcp serve --exclude-tools desktop   # Denylist specific tools
nebo mcp config --target claude-desktop  # Print config for Claude Desktop
nebo mcp config --target cursor          # Print config for Cursor
```

### McpStdioBridge

```
Claude Desktop / Cursor
    | JSON-RPC stdin
    v
McpStdioBridge
    | POST /agent/mcp (660s timeout)
    v
Nebo Server (localhost:27895)
```

**Startup:** Health check (`GET /health`, 3 attempts, 1s between, 5s per attempt)
**Loop:** Read line from stdin -> POST to `/agent/mcp` -> filter tools if tools/list -> write to stdout
**Tool filtering:** `--tools` (allowlist) and `--exclude-tools` (denylist) filter `tools/list` responses only

### Config Output

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

Paths: Claude Desktop `~/Library/Application Support/Claude/claude_desktop_config.json`, Cursor `.cursor/mcp.json`

## REST API Handlers (integrations.rs)

### Endpoints

| Method | Path | Auth | Purpose |
|--------|------|------|---------|
| GET | `/api/v1/integrations` | JWT | List all integrations |
| POST | `/api/v1/integrations` | JWT | Create integration |
| GET | `/api/v1/integrations/:id` | JWT | Get single |
| PUT | `/api/v1/integrations/:id` | JWT | Update |
| DELETE | `/api/v1/integrations/:id` | JWT | Delete + disconnect |
| POST | `/api/v1/integrations/:id/test` | JWT | Test connection (10s) |
| POST | `/api/v1/integrations/:id/connect` | JWT | Connect + list tools |
| GET | `/api/v1/integrations/:id/oauth-url` | JWT | Start OAuth flow |
| GET | `/api/v1/integrations/oauth/callback` | Public | OAuth redirect handler |
| GET | `/api/v1/integrations/registry` | JWT | Built-in server list |
| GET | `/api/v1/integrations/tools` | JWT | All registered tools |
| GET | `/api/v1/mcp/servers` | JWT | Alias for registry |

### OAuth Flow

```
Frontend                    Nebo Server                  External MCP Server
   |                            |                              |
   | GET /integrations/:id/     |                              |
   |     oauth-url              |                              |
   |--------------------------->|                              |
   |                            | GET /.well-known/oauth-...   |
   |                            |----------------------------->|
   |                            |<-----------------------------|
   |                            |  OAuthMetadata               |
   |                            |                              |
   |                            | POST registration_endpoint   |
   |                            |----------------------------->| (DCR, if supported)
   |                            |<-----------------------------|
   |                            |  client_id, client_secret    |
   |                            |                              |
   |                            | Generate PKCE + state        |
   |                            | Encrypt & store in DB        |
   |                            |                              |
   |  { authUrl: "..." }        |                              |
   |<---------------------------|                              |
   |                            |                              |
   | Open browser -------------------------------------------------->|
   |                            |                              |
   |                            | GET /oauth/callback?code&state     |
   |                            |<-----------------------------|
   |                            |                              |
   |                            | Decrypt PKCE verifier        |
   |                            | POST token_endpoint          |
   |                            |----------------------------->|
   |                            |<-----------------------------|
   |                            |  access_token, refresh_token |
   |                            |                              |
   |                            | Encrypt & store tokens       |
   |                            | bridge.connect() immediately |
   |                            |----------------------------->| (tools/list)
   |                            |<-----------------------------|
```

### PKCE (RFC 7636)

- `generate_code_verifier()` — 32 random bytes -> base64url (43 chars)
- `compute_code_challenge(verifier)` — SHA-256(verifier) -> base64url
- Challenge method: `S256`

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
Fallback client_id: `nebo-agent-{integration_id}`

### sync_bridge()

Called after integration CRUD. Replaced the old `sync_all(None)` pattern with per-integration token resolution:
1. Disconnect integrations no longer in the enabled set
2. For each enabled integration: resolve/refresh OAuth token, then `bridge.connect()` individually
3. Updates `connection_status` and `tool_count` in DB

## Database Schema

### mcp_integrations (migration 0018 + 0025 + 0029)

```sql
CREATE TABLE mcp_integrations (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    server_type TEXT NOT NULL,             -- "http", "stdio", "custom"
    server_url TEXT,                        -- MCP endpoint URL
    auth_type TEXT DEFAULT 'none',         -- "none", "api_key", "oauth"
    is_enabled INTEGER DEFAULT 1,
    connection_status TEXT,                -- "connected", "disconnected", "error"
    last_connected_at INTEGER,
    last_error TEXT,
    metadata TEXT,                         -- JSON blob
    tool_count INTEGER DEFAULT 0,          -- (0029)
    -- OAuth flow columns (0025):
    oauth_state TEXT,                      -- PKCE state (flow in progress)
    oauth_pkce_verifier TEXT,              -- encrypted code_verifier
    oauth_client_id TEXT,
    oauth_client_secret TEXT,              -- encrypted
    oauth_authorization_endpoint TEXT,
    oauth_token_endpoint TEXT,
    oauth_registration_endpoint TEXT,
    created_at INTEGER, updated_at INTEGER
);
```

### mcp_integration_credentials (migration 0018)

```sql
CREATE TABLE mcp_integration_credentials (
    id TEXT PRIMARY KEY,
    integration_id TEXT REFERENCES mcp_integrations(id) ON DELETE CASCADE,
    credential_type TEXT,                  -- "api_key", "oauth_token"
    credential_value TEXT,                 -- encrypted access_token
    refresh_token TEXT,                    -- encrypted refresh_token
    expires_at INTEGER,                    -- unix timestamp (seconds)
    scopes TEXT,
    created_at INTEGER, updated_at INTEGER
);
```

### mcp_server_registry (migration 0018)

Pre-populated known servers (Notion, GitHub, Linear, Slack, filesystem, brave-search, memory) with icons, auth config, and setup URLs. Used by the frontend.

### DB Query Functions (mcp_integrations.rs)

| Function | Return | Purpose |
|----------|--------|---------|
| `list_mcp_integrations()` | `Vec<McpIntegration>` | All integrations |
| `get_mcp_integration(id)` | `Option<McpIntegration>` | Single by ID |
| `create_mcp_integration(...)` | `McpIntegration` | Create new |
| `update_mcp_integration(...)` | `()` | Update fields |
| `delete_mcp_integration(id)` | `()` | Delete |
| `set_mcp_server_type(id, type)` | `()` | Fix legacy |
| `set_mcp_connection_status(id, status, count)` | `()` | Update status |
| `set_mcp_oauth_state(...)` | `()` | Save OAuth flow state |
| `get_mcp_integration_by_oauth_state(state)` | `Option<...>` | Lookup by state |
| `clear_mcp_oauth_state(id)` | `()` | Clear after callback |
| `store_mcp_credentials(...)` | `()` | Store encrypted creds |
| `get_mcp_credential(id, type)` | `Option<(String, Option<String>)>` | Get value + refresh |
| `get_mcp_credential_full(id, type)` | `Option<McpCredentialFull>` | Full with expiry |
| `get_mcp_oauth_config(id)` | `Option<McpOAuthConfig>` | Config for refresh |

## Known Issues

### 1. ProxyToolRegistry Trait Unused

The `Bridge` holds `Arc<dyn ProxyToolRegistry>` but never calls register/unregister. Tools are tracked internally only. All access goes through the single `mcp()` STRAP tool. Dead code but harmless.

### 2. Non-Blocking connected_tools()

Uses `try_lock()` — returns empty results if mutex is locked during reconnection. The McpTool may briefly report "no servers connected."

### 3. No API Key Auth in Client

`call_tool()` only sends Bearer auth. If `auth_type` is "api_key", the credential is stored but never retrieved or sent as a header for tool calls.

### 4. No MCP Resources/Prompts

The server only implements `tools` capability. `resources/list`, `resources/read`, `prompts/list`, `prompts/get`, `logging/setLevel`, and `sampling` are not implemented.

## Constants

| Constant | Value | Location |
|----------|-------|----------|
| HTTP client timeout | 30s | client.rs |
| Token refresh timeout | 15s | client.rs |
| Stdio bridge timeout | 660s | mcp_serve.rs |
| Chat default timeout | 300s | mcp_server.rs |
| Chat max timeout | 600s | mcp_server.rs |
| Token expiry buffer | 60s | mcp_tool.rs |
| AES nonce size | 12 bytes | crypto.rs |
| AES key size | 32 bytes | crypto.rs |
| Health check retries | 3 | mcp_serve.rs |
| Health check delay | 1s | mcp_serve.rs |
| Health check timeout | 5s | mcp_serve.rs |
| DCR timeout | 10s | integrations.rs |
| Integration test timeout | 10s | integrations.rs |

## Security Summary

| Aspect | Status |
|--------|--------|
| Credentials at rest | AES-256-GCM encrypted |
| PKCE | Code verifier + S256 challenge |
| OAuth state / CSRF | Random 16-byte state param |
| Token refresh | 60s expiry buffer, proactive + 401 retry |
| MCP endpoint auth | Opt-in API key (`NEBO_MCP_API_KEY`) |
| MCP origin | `Origin::Mcp` (no deny list, full access) |
| Non-localhost binding | Blocked unless `NEBO_ALLOW_REMOTE=true` |
| Auto-approval | All MCP tool calls and ask requests auto-approved |
| Server binding | Default `127.0.0.1:27895` (localhost only) |

## Key Files Quick Reference

| File | Purpose |
|------|---------|
| `crates/mcp/src/client.rs` | HTTP client, JSON-RPC, OAuth |
| `crates/mcp/src/bridge.rs` | Connection management |
| `crates/mcp/src/crypto.rs` | AES-256-GCM encryption |
| `crates/mcp/src/types.rs` | Error types, data models |
| `crates/tools/src/mcp_tool.rs` | STRAP domain tool |
| `crates/tools/src/origin.rs` | Origin enum (includes Mcp) |
| `crates/tools/src/policy.rs` | Policy levels, origin deny lists |
| `crates/server/src/handlers/integrations.rs` | REST API + OAuth flow |
| `crates/server/src/handlers/mcp_server.rs` | JSON-RPC server handler |
| `crates/server/src/middleware.rs` | MCP API key auth middleware |
| `crates/server/src/lib.rs:580-660` | Startup init + reconnection |
| `crates/cli/src/main.rs` | CLI commands (mcp serve/config) |
| `crates/cli/src/mcp_serve.rs` | Stdio bridge implementation |
| `crates/db/src/queries/mcp_integrations.rs` | DB queries |
| `crates/db/migrations/0018_*.sql` | Main schema |
| `crates/db/migrations/0025_*.sql` | OAuth columns |
| `crates/db/migrations/0029_*.sql` | tool_count column |
| `app/src/routes/(app)/integrations/+page.svelte` | Frontend UI |
| `crates/agent/src/strap/mcp.txt` | STRAP prompt docs |

## AppState Members

```rust
pub bridge: Arc<mcp::Bridge>,
pub mcp_context: Arc<tokio::sync::Mutex<tools::ToolContext>>,
```
