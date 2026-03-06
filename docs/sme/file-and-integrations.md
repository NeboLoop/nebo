# File Serving and MCP Integrations: Complete Migration Reference

**Source:** `nebo/internal/handler/files/`, `nebo/internal/mcp/`, `nebo/internal/handler/integration/` | **Target:** `nebo-rs/crates/server/src/handlers/files.rs`, `nebo-rs/crates/mcp/src/`, `nebo-rs/crates/server/src/handlers/integrations.rs` | **Status:** Draft

This document consolidates two Go SME references -- FILE_SERVING and INTEGRATIONS -- into a single Rust migration guide. It covers file serving from `data_dir/files/`, the ToolResult ImageURL contract, the screenshot/image pipeline, MCP integration architecture, OAuth 2.1 flows, the MCP bridge, encryption, API endpoints, frontend behavior, and current Rust implementation status.

---

## Table of Contents

1. [File Serving Architecture](#1-file-serving-architecture)
2. [ToolResult ImageURL Contract](#2-toolresult-imageurl-contract)
3. [Screenshot and Image Pipeline](#3-screenshot-and-image-pipeline)
4. [MCP Integration Architecture](#4-mcp-integration-architecture)
5. [OAuth 2.1 Flow](#5-oauth-21-flow)
6. [MCP Bridge](#6-mcp-bridge)
7. [Encryption and Key Management](#7-encryption-and-key-management)
8. [Integration API Endpoints](#8-integration-api-endpoints)
9. [Frontend Reference](#9-frontend-reference)
10. [Rust Implementation Status](#10-rust-implementation-status)

---

## 1. File Serving Architecture

**File(s):** `filehandler.go`, `server.go` (Go) -- `files.rs`, `lib.rs` (Rust)

### 1.1 Storage Location

All agent-produced files (screenshots, downloads, generated images) are stored in a single flat directory:

```
<data_dir>/files/
```

Platform-specific `data_dir` locations:

| Platform | Path |
|----------|------|
| macOS | `~/Library/Application Support/Nebo/files/` |
| Linux | `~/.config/nebo/files/` |
| Windows | `%AppData%\Nebo\files\` |
| Override | `NEBO_DATA_DIR` env var |

The directory is created on demand. In Go this uses `os.MkdirAll`; in Rust the config crate's `data_dir()` provides the base path and the handler reads from `data_dir.join("files")`.

### 1.2 Data Flow

Tool execution saves file to `<data_dir>/files/<name>.png` and returns a `ToolResult` with `image_url: "/api/v1/files/<name>.png"`. The hub extracts `image_url`, streams an `"image"` WebSocket event to the browser in real time, and persists it as a `contentBlock` in `chat_messages.metadata`. On history load, the frontend parses metadata, extracts contentBlocks, and renders `<img src={block.imageURL}>`. The browser fetches the image via `GET /api/v1/files/<name>.png`, which the `serve_file` handler resolves against `<data_dir>/files/`.

### 1.3 HTTP Endpoints

Two routes are registered under `/api/v1`:

| Method | Path | Handler | Purpose |
|--------|------|---------|---------|
| GET | `/files/{*path}` | `serve_file` | Serve any file from `data_dir/files/` |
| POST | `/files/browse` | `browse` | Browse filesystem directories |

Both routes are protected (require auth in Go; currently public in Rust -- see Section 10).

### 1.4 Content-Type Mapping

The file handler determines Content-Type from the file extension:

| Extension | Content-Type |
|-----------|-------------|
| `.png` | `image/png` |
| `.jpg`, `.jpeg` | `image/jpeg` |
| `.gif` | `image/gif` |
| `.svg` | `image/svg+xml` |
| `.pdf` | `application/pdf` |
| `.json` | `application/json` |
| `.txt`, `.log` | `text/plain` |
| `.html` | `text/html` |
| `.css` | `text/css` |
| `.js` | `application/javascript` |
| (other) | `application/octet-stream` |

The Rust handler extends the Go mapping with `.json`, `.txt`, `.log`, `.html`, `.css`, and `.js` types. The Go handler also sets `.webp` -> `image/webp` which the Rust handler does NOT yet include.

### 1.5 Security Layers

**Go implementation (3 checks):**
1. `filepath.Clean` rejects `..` traversal
2. `strings.HasPrefix(fullPath, baseDir)` ensures the resolved path stays within the files dir
3. Only serves files, NOT directories

**Rust implementation (current):**
- Joins `data_dir/files/` with the path parameter via `Path::join`
- Checks `full_path.exists()` before reading
- Does NOT yet perform explicit path traversal validation (canonicalization + prefix check)

**CRITICAL:** The Rust handler MUST add path canonicalization and prefix validation before production. Axum's path extractor does some normalization, but an explicit `canonicalize()` + `starts_with()` check is needed to match Go's security guarantees.

### 1.6 Caching

The Go handler sets `Cache-Control: public, max-age=3600` (1 hour). The Rust handler does NOT yet set any caching headers.

### 1.7 Browse Handler Differences

**Go:** Opens a native OS file picker dialog via a callback installed during Wails desktop initialization. Returns `501 Not Implemented` in headless mode.

**Rust:** Implements a directory listing endpoint that accepts a `path` JSON parameter, expands `~` via `shellexpand::tilde`, reads directory entries, sorts them (directories first, then alphabetical), and returns structured JSON with `name`, `path`, `isDir`, and `size` fields. This is a DIFFERENT behavior from Go -- Rust provides a REST-based file browser rather than a native dialog.

---

## 2. ToolResult ImageURL Contract

**File(s):** `registry.go` (Go) -- `registry.rs` (Rust)

### 2.1 Struct Definition

**Go:**
```go
type ToolResult struct {
    Content  string `json:"content"`
    IsError  bool   `json:"is_error,omitempty"`
    ImageURL string `json:"image_url,omitempty"`
}
```

**Rust:**
```rust
pub struct ToolResult {
    pub content: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
}
```

### 2.2 Contract Rules

1. `image_url` MUST be a path that the file handler can resolve -- typically `/api/v1/files/<filename>`
2. The referenced file MUST exist in `<data_dir>/files/` for the URL to work
3. When `image_url` is present, the hub streams an `"image"` WebSocket event to the browser immediately
4. The URL is persisted in `chat_messages.metadata` as part of contentBlocks for history replay

### 2.3 Constructors

The Rust `ToolResult` provides two constructors that set `image_url` to `None`:

```rust
impl ToolResult {
    pub fn ok(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: false, image_url: None }
    }
    pub fn error(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: true, image_url: None }
    }
}
```

Tools that produce images must construct `ToolResult` manually with `image_url: Some(...)`.

---

## 3. Screenshot and Image Pipeline

**File(s):** `screenshot.go` (Go) -- `desktop_tool.rs`, `web_tool.rs` (Rust)

### 3.1 Go Screenshot Pipeline

Go captures a screenshot, saves to `<data_dir>/files/screenshot_YYYYMMDD_HHMMSS.png`, and returns `ToolResult { ImageURL: "/api/v1/files/screenshot_YYYYMMDD_HHMMSS.png" }`. The hub streams an `"image"` WS event, ChatContext persists the URL in `chat_messages.metadata`, and the frontend renders `<img src={block.imageURL}>`.

**See action (annotated snapshots):** Uses `screenshot_see_YYYYMMDD_HHMMSS.png` -- always saves to `<data_dir>/files/`.

### 3.2 Go Known Bug: Custom Output Path

When the agent specifies a custom `output` parameter, the file is saved to that arbitrary location, but `ImageURL` is still constructed from `filepath.Base(outputPath)`. The file handler looks in `<data_dir>/files/` where the file does NOT exist, resulting in a 404 and a broken image. The fix is to always save a copy to `<data_dir>/files/` regardless of custom output path.

### 3.3 Rust Screenshot Implementation

The Rust `desktop_tool.rs` captures screenshots on macOS via `screencapture`, reads the PNG into memory, deletes the temp file, base64-encodes the bytes, and returns `image_url: Some("data:image/png;base64,...")`.

**Key difference from Go:** Rust returns a `data:` URI instead of a file-serving URL. Consequences:
- Screenshot is NOT persisted to `<data_dir>/files/`
- Image is embedded inline in WS event payload
- History replay depends on the full base64 string in `chat_messages.metadata`
- Large screenshots significantly increase DB storage and WS message size

**CRITICAL:** The Rust implementation should be updated to match Go's approach -- save to `<data_dir>/files/` and return a URL path. The frontend already supports both rendering modes (base64 inline and URL-based), so this is a backend-only change.

### 3.4 Real-Time Streaming Pipeline (Go Reference)

The pipeline has 4 steps: (1) tool result payload includes `image_url`, (2) ChatContext appends `contentBlock{Type:"image", ImageURL}` to the pending request, (3) an `"image"` WS event is sent immediately with `session_id` and `image_url`, (4) the frontend `handleImage()` appends the block to `currentStreamingMessage.contentBlocks`.

### 3.5 Database Persistence

ContentBlocks are serialized into `chat_messages.metadata` as JSON. The `contentBlock` struct has fields: `type` ("text"/"tool"/"image"), `text`, `toolCallIndex`, and `imageURL`. Example stored JSON:

```json
{
  "contentBlocks": [
    { "type": "text", "text": "Here is the screenshot..." },
    { "type": "tool", "toolCallIndex": 0 },
    { "type": "image", "imageURL": "/api/v1/files/screenshot_20260222_150405.png" }
  ],
  "toolCalls": [...],
  "thinking": "..."
}
```

### 3.6 Known Issues (Both Implementations)

| Issue | Go | Rust |
|-------|-----|------|
| Custom output path bug | Y -- file saved to wrong dir | N/A -- not implemented |
| No file cleanup/rotation | Y -- files accumulate indefinitely | Y -- same |
| No deduplication | Y -- same-second screenshots overwrite | N/A -- uses PID-based naming |
| Large base64 in metadata | N -- uses URL | Y -- embeds full data URI |

---

## 4. MCP Integration Architecture

**File(s):** `handler.go`, `oauth.go` (Go) -- `integrations.rs` (Rust), `client.rs`, `bridge.rs`, `types.rs`

### 4.1 Overview

Integrations connect Nebo's agent to external MCP (Model Context Protocol) servers, giving the agent new tools dynamically. Think of it as Nebo's plugin system for third-party services: Notion, GitHub, Linear, Slack, or any custom Streamable HTTP MCP server.

### 4.2 Layer Stack

Four layers from frontend to external servers:

1. **Frontend** -- Settings > Integrations page with 3-step wizard (URL -> Auth -> Name)
2. **Handler** -- `handlers/integrations.rs`: CRUD + test + bridge sync (8 endpoints)
3. **MCP Client** -- `crates/mcp/src/client.rs`: OAuth 2.1 discovery, session management, tool invocation; sessions cached in `RwLock<HashMap<String, Session>>`
4. **MCP Bridge** -- `crates/mcp/src/bridge.rs`: syncs enabled integrations, registers proxy tools via `ProxyToolRegistry` trait, naming: `mcp__{serverType}__{toolName}`

### 4.3 Database Schema (3 Tables)

#### `mcp_integrations` (migration 0018 + 0025 + 0029)

| Column | Type | Notes |
|--------|------|-------|
| id | TEXT PK | UUID |
| name | TEXT NOT NULL | User-friendly label |
| server_type | TEXT NOT NULL | hostname-derived, "stdio", or "custom" |
| server_url | TEXT | Streamable HTTP endpoint |
| auth_type | TEXT NOT NULL | "oauth", "api_key", "none" |
| is_enabled | INTEGER | 0/1 toggle, default 1 |
| connection_status | TEXT | "connected", "disconnected", "error" |
| last_connected_at | INTEGER | unix epoch |
| last_error | TEXT | last failure message |
| metadata | TEXT | JSON for extra config |
| tool_count | INTEGER | cached count from last sync (migration 0029) |
| oauth_state | TEXT | CSRF state for in-flight OAuth (migration 0025) |
| oauth_pkce_verifier | TEXT | encrypted PKCE verifier (migration 0025) |
| oauth_client_id | TEXT | from dynamic registration or default (migration 0025) |
| oauth_client_secret | TEXT | encrypted, nullable for public clients (migration 0025) |
| oauth_authorization_endpoint | TEXT | discovered from .well-known (migration 0025) |
| oauth_token_endpoint | TEXT | discovered from .well-known (migration 0025) |
| oauth_registration_endpoint | TEXT | for dynamic client reg (migration 0025) |
| created_at | INTEGER NOT NULL | default unixepoch() |
| updated_at | INTEGER NOT NULL | default unixepoch() |

#### `mcp_integration_credentials`

| Column | Type | Notes |
|--------|------|-------|
| id | TEXT PK | UUID |
| integration_id | TEXT FK | CASCADE delete on mcp_integrations |
| credential_type | TEXT NOT NULL | "api_key" or "oauth_token" |
| credential_value | TEXT NOT NULL | AES-256-GCM encrypted |
| refresh_token | TEXT | encrypted, nullable |
| expires_at | INTEGER | unix epoch for token expiry |
| scopes | TEXT | comma-separated |
| created_at | INTEGER NOT NULL | default unixepoch() |
| updated_at | INTEGER NOT NULL | default unixepoch() |

#### `mcp_server_registry` (pre-populated catalog)

Pre-seeded entries: notion, github, linear, slack, filesystem, memory. Used for display metadata (icons, API key URLs, OAuth scopes). The frontend wizard does NOT actively use this table -- it is URL-first, not registry-first.

In Rust, the `list_registry` handler returns a hardcoded JSON list instead of querying this table. The hardcoded list includes: filesystem, brave-search, github, sqlite, memory.

### 4.4 McpIntegration Model (Rust)

The Rust `McpIntegration` model currently reads 13 columns (id through tool_count). The 7 OAuth columns from migration 0025 are present in the DB but NOT yet mapped to the Rust model struct. This is intentional -- OAuth flows are not yet implemented in Rust.

---

## 5. OAuth 2.1 Flow

**File(s):** `client.go`, `callback.go` (Go) -- `client.rs` (Rust, partial)

### 5.1 Full OAuth Sequence (Go Reference)

1. User clicks "Connect with OAuth". Frontend creates the integration via `POST /integrations`, then calls `GET /integrations/{id}/oauth-url`.
2. **StartOAuthFlow():** Discover metadata at `{serverURL}/.well-known/oauth-authorization-server`. Generate PKCE (S256) and random state (CSRF). Get or create client credentials (DB -> dynamic registration RFC 7591 -> fallback public client `"nebo-agent-{integrationID}"`). Encrypt PKCE verifier + client secret. Store flow state in `mcp_integrations` OAuth columns. Return authorization URL with params: `response_type=code, client_id, redirect_uri, state, code_challenge, code_challenge_method=S256, scope`.
3. Browser redirects to external auth server. User authenticates and grants consent.
4. External server redirects to `GET /api/v1/integrations/oauth/callback?code=...&state=...`.
5. **OAuthCallbackHandler:** Validate state (CSRF lookup). Decrypt PKCE verifier. POST to token_endpoint (`grant_type=authorization_code, code, redirect_uri, client_id, code_verifier`). Encrypt and store tokens. Clear OAuth state columns. Set `connection_status = "connected"`. Notify agent via `integrations_changed`. Redirect to `/settings/mcp?connected={id}`.

### 5.2 PKCE (Proof Key for Code Exchange)

- Method: `S256` (SHA-256 hash of code_verifier, base64url-encoded)
- code_verifier: random 32 bytes, base64url-encoded (43 chars)
- code_challenge: SHA-256(code_verifier), base64url-encoded
- Verifier is encrypted with AES-256-GCM and stored in `oauth_pkce_verifier` column during the flow

### 5.3 Dynamic Client Registration (RFC 7591)

If the server exposes a `registration_endpoint` in its OAuth metadata:

1. POST to registration_endpoint with `{ client_name: "Nebo Agent", redirect_uris: [...] }`
2. Store returned client_id and client_secret (encrypted)
3. If registration fails, fall back to public client: `"nebo-agent-{integrationID}"`

### 5.4 Token Refresh

```
GetAccessToken() called
  |
  +-- Check expires_at - 60s (1 minute buffer)
  |
  +-- If expired AND refresh_token exists:
  |     POST to token_endpoint
  |     grant_type=refresh_token, refresh_token, client_id
  |     +-- If response includes new refresh_token: store it
  |     +-- If response omits refresh_token: preserve old one
  |
  +-- Return access_token
```

### 5.5 Rust OAuth Implementation Status

The Rust `McpClient` provides `discover_oauth()` which fetches `.well-known/oauth-authorization-server` metadata. The following are NOT yet implemented in Rust: PKCE generation, dynamic client registration, authorization URL construction, OAuth callback handler, token exchange, token refresh, OAuth state/CSRF management.

---

## 6. MCP Bridge

**File(s):** `bridge.go` (Go) -- `bridge.rs` (Rust)

### 6.1 Purpose

The bridge is the CRITICAL piece that makes external MCP tools available to the agent. It syncs enabled integrations from the database, discovers their tools via `list_tools`, and registers proxy tools in the agent's tool registry.

### 6.2 How It Works

`sync_all()` loads all enabled integrations, disconnects stale ones, then for each enabled integration with a server URL: calls `list_tools()`, creates a proxy tool per discovered tool, and registers it via `ProxyToolRegistry` with the namespaced name. Proxy tools forward execution to the external server via `call_tool()`.

### 6.3 Tool Naming Convention

Format: `mcp__{server_type}__{tool_name}`. Examples: `mcp__github.com__search_repos`, `mcp__brave-search__web_search`. The Rust `make_tool_name()` lowercases `server_type` and replaces spaces with underscores.

### 6.4 ProxyToolRegistry Trait

```rust
pub trait ProxyToolRegistry: Send + Sync {
    fn register_proxy(&self, name: &str, original_name: &str,
        description: &str, schema: Option<serde_json::Value>, integration_id: &str);
    fn unregister_proxy(&self, name: &str);
}
```

The `tools::Registry` implements this trait. In Go, all proxy tools have `RequiresApproval() = true` (always needs user approval). Schema is passed through from the MCP server's InputSchema. Execute extracts TextContent from the MCP result.

### 6.5 Sync Triggers

**Go (3 mechanisms):**
1. **Initial sync** -- goroutine on agent start
2. **Periodic re-sync** -- every 15 minutes via ticker
3. **Event-driven** -- on `integrations_changed` WebSocket event from API handler

**Rust (2 mechanisms implemented):**
1. **Initial sync** -- `tokio::spawn` on server start (in `lib.rs`)
2. **Event-driven** -- `sync_bridge()` called after create/update/delete in handler

**NOT yet implemented in Rust:** Periodic 15-minute re-sync ticker.

### 6.6 Connection Management

The Rust bridge uses `tokio::sync::Mutex<HashMap<String, Connection>>` to track live connections:

```rust
struct Connection {
    integration_id: String,
    server_type: String,
    tool_names: Vec<String>, // namespaced names registered in the tool registry
}
```

### 6.7 Notification Pipeline

When any integration is created/updated/deleted, the Rust handler calls `sync_bridge(&state)` which loads all integrations from the DB and calls `bridge.sync_all()` directly. In Go, the handler broadcasts an `integrations_changed` event via AgentHub, and the agent's event handler picks it up -- the Rust approach is simpler (direct call) but skips the event-based decoupling.

---

## 7. Encryption and Key Management

**File(s):** `crypto.go` (Go) -- `crypto.rs` (Rust)

### 7.1 Algorithm

All credentials at rest use **AES-256-GCM** encryption.

- Key size: 32 bytes (256 bits)
- Nonce size: 12 bytes
- Output format: `nonce || ciphertext` (nonce prepended to ciphertext)
- Encoding: base64 for storage

### 7.2 Key Resolution Priority

**Go (5 levels):** OS Keychain -> `MCP_ENCRYPTION_KEY` env (hex-decoded) -> `JWT_SECRET` env (first 32 bytes) -> persistent file `{dataDir}/.mcp-key` -> generate new. Keys found in env/file are promoted to keychain automatically.

**Rust (4 levels):** `MCP_ENCRYPTION_KEY` env (SHA-256 hashed) -> `JWT_SECRET` env (SHA-256 hashed) -> persistent file `{dataDir}/.mcp-key` (raw 32 bytes) -> generate and persist to `.mcp-key`.

### 7.3 Key Differences from Go

| Feature | Go | Rust |
|---------|-----|------|
| OS Keychain | Y -- first priority | N -- not implemented |
| Env var handling | hex-decode for MCP_ENCRYPTION_KEY, first 32 bytes for JWT_SECRET | SHA-256 hash of passphrase for both |
| Key promotion | Y -- env/file keys promoted to keychain | N -- no keychain |
| File key persistence | Y -- deleted after keychain promotion | Y -- persisted permanently |

**NOTE:** The env var handling difference means Go and Rust will derive DIFFERENT keys from the same env var values. This is acceptable since Go and Rust do NOT share the same database.

### 7.4 Encryptor API

The `Encryptor` struct wraps a 32-byte key and provides: `new(key)`, `from_passphrase(str)` (SHA-256 derivation), `generate()` (random), `key_bytes()`, `encrypt/decrypt` (raw bytes), and `encrypt_b64/decrypt_b64` (base64 encoded).

### 7.5 What Gets Encrypted

| Item | Go | Rust |
|------|-----|------|
| API keys (on create/update) | Y | N -- not yet |
| OAuth access tokens | Y | N -- OAuth not implemented |
| OAuth refresh tokens | Y | N -- OAuth not implemented |
| PKCE verifiers (during flow) | Y | N -- OAuth not implemented |
| Client secrets (dynamic reg) | Y | N -- OAuth not implemented |
| Token encrypt/decrypt helpers | Y | Y -- `encrypt_token` / `decrypt_token` on McpClient |

### 7.6 Credential Migration

Go has a `credential/migrate.go` that upgrades plaintext credentials to encrypted on startup. It is idempotent (skips values prefixed with `enc:`). Covers: auth_profiles, mcp_integration_credentials, app_oauth_grants, plugin_settings.

Rust does NOT yet have credential migration.

---

## 8. Integration API Endpoints

**File(s):** `handler.go`, `oauth.go` (Go) -- `integrations.rs` (Rust)

### 8.1 Go Endpoints (12 routes)

| # | Method | Path | Handler | Purpose | Rust? |
|---|--------|------|---------|---------|-------|
| 1 | GET | `/integrations` | ListMCPIntegrationsHandler | List all integrations | Y |
| 2 | POST | `/integrations` | CreateMCPIntegrationHandler | Create new integration | Y |
| 3 | GET | `/integrations/registry` | ListMCPServerRegistryHandler | List known server catalog | Y |
| 4 | GET | `/integrations/tools` | ListMCPToolsHandler | List tools from connected integrations | Y |
| 5 | GET | `/integrations/{id}` | GetMCPIntegrationHandler | Get single integration | Y |
| 6 | PUT | `/integrations/{id}` | UpdateMCPIntegrationHandler | Update name/URL/enabled/apiKey | Y |
| 7 | DELETE | `/integrations/{id}` | DeleteMCPIntegrationHandler | Delete integration + credentials | Y |
| 8 | POST | `/integrations/{id}/test` | TestMCPIntegrationHandler | Test connection, update status | Y |
| 9 | GET | `/integrations/{id}/oauth-url` | GetMCPOAuthURLHandler | Get OAuth authorization URL | N |
| 10 | POST | `/integrations/{id}/disconnect` | DisconnectMCPIntegrationHandler | Revoke tokens, clear creds | N |
| 11 | GET | `/integrations/oauth/callback` | OAuthCallbackHandler | OAuth redirect endpoint | N |
| 12 | GET | `/integrations/{id}/tools` | ListIntegrationToolsHandler (inferred) | Per-integration tool list | N |

### 8.2 Route Registration

Go registers 12 routes under `/api/v1/integrations` via chi. Rust registers 8 routes under the same prefix via Axum (see table above for which are implemented).

### 8.3 Handler Behavior Differences

**Create:**
- Go creates the record then calls `notifyIntegrationsChanged(svcCtx)` to broadcast via AgentHub
- Rust creates the record then calls `sync_bridge(&state).await` directly

**Test:**
- Go connects via the MCP client, updates `connection_status` and `tool_count` in DB
- Rust makes a simple HTTP GET to the server_url and checks reachability. Does NOT list tools or update DB status fields

**List tools:**
- Go queries only OAuth integrations with `connectionStatus == "connected"`
- Rust returns ALL registered tools from the tool registry (built-in + MCP), not integration-specific

**List registry:**
- Go queries the `mcp_server_registry` DB table
- Rust returns a hardcoded JSON array of known MCP servers (filesystem, brave-search, github, sqlite, memory)

---

## 9. Frontend Reference

**File(s):** `+page.svelte` (integrations), `+page.svelte` (agent), `MessageGroup.svelte`

### 9.1 Integration Wizard (3-Step Add Flow)

The frontend implements a multi-step modal for adding integrations:

1. **Step 1: URL** -- User enters the MCP server's Streamable HTTP endpoint. Validated as http/https URL.
2. **Step 2: Auth** -- Radio selection: OAuth (recommended), API Key, or None. API Key shows inline password input.
3. **Step 3: Name** -- Optional friendly name (defaults to hostname). Shows summary of URL + auth type.

On submit:
- Calls `POST /integrations` to create the DB record
- If auth=oauth, immediately calls `GET /integrations/{id}/oauth-url` and redirects browser to external auth server
- OAuth callback redirects back to `/settings/mcp?connected={id}` (note: minor routing mismatch -- MCP page redirects to integrations via `onMount`)

### 9.2 Integration List Card

Each integration displays:
- Colored initial avatar (first letter of name)
- Name + auth type badge (OAUTH / API KEY / NONE)
- Server URL
- Connection status icon (green check / red X / gray X)
- Tool count when connected
- Last error if any
- Actions: Test button, dropdown menu (Enable/Disable, Delete)

### 9.3 Image Rendering in Chat

Two rendering modes in `MessageGroup.svelte`:
1. **Base64 inline** -- `data:image/png;base64,...` (for images sent directly in the stream)
2. **URL-based** -- `/api/v1/files/...` (standard path, browser fetches from file handler)

The `ContentBlock` TypeScript interface has fields: `type` ('text'/'tool'/'image'), `text?`, `toolCallIndex?`, `imageData?`, `imageMimeType?`, `imageURL?`.

Real-time handling: `handleImage()` extracts `image_url` from the WS event data and appends a `{type:'image', imageURL}` block to `currentStreamingMessage.contentBlocks`.

### 9.4 State Management

All integration state is component-local using Svelte 5 `$state` runes. No stores -- data is fetched on mount and after mutations via `loadIntegrations()`. The frontend is SHARED between Go and Rust backends. It is built once and served via `rust-embed` in the Rust binary from `../../app/build/`.

---

## 10. Rust Implementation Status

### 10.1 File Serving

| Feature | Go | Rust | Status |
|---------|-----|------|--------|
| GET /files/* serve handler | Y | Y | Implemented |
| POST /files/browse | Y (native dialog) | Y (directory listing) | Different behavior |
| Path traversal validation | Y (Clean + HasPrefix) | N | NOT implemented |
| Content-Type mapping | 7 types | 11 types | Rust has more, missing .webp |
| Cache-Control header | Y (1h) | N | NOT implemented |
| JWT auth on file routes | Y | N | NOT implemented |
| Screenshot save to files/ | Y | N (uses base64 data URI) | Needs migration |

### 10.2 MCP Client

| Feature | Go | Rust | Status |
|---------|-----|------|--------|
| OAuth discovery (.well-known) | Y | Y | Implemented |
| List tools from server | Y | Y | Implemented |
| Call tool on server | Y | Y | Implemented |
| Session caching | sync.Map keyed by ID | RwLock<HashMap> | Implemented (different concurrency model) |
| Health checker (5min tick) | Y | N | NOT implemented |
| Session staleness check (30min/10min) | Y | N | NOT implemented |
| Authenticated transport (Bearer) | Y | Y | Implemented |
| PKCE generation | Y | N | NOT implemented |
| Dynamic client registration | Y | N | NOT implemented |
| Token exchange | Y | N | NOT implemented |
| Token refresh | Y | N | NOT implemented |
| Retry with exponential backoff | Y (infinite, max 10min) | N | NOT implemented |

### 10.3 MCP Bridge

| Feature | Go | Rust | Status |
|---------|-----|------|--------|
| sync_all (load enabled, disconnect stale) | Y | Y | Implemented |
| connect (list tools, register proxy) | Y | Y | Implemented |
| disconnect (remove proxy tools) | Y | Y | Implemented |
| close (disconnect all) | Y | Y | Implemented |
| call_tool (forward to external server) | Y | Y | Implemented |
| Tool naming: mcp__{type}__{name} | Y | Y | Implemented |
| ProxyToolRegistry trait | N/A (direct) | Y | Implemented |
| Initial sync on startup | Y (goroutine) | Y (tokio::spawn) | Implemented |
| Periodic re-sync (15min) | Y | N | NOT implemented |
| Event-driven sync (integrations_changed) | Y (via hub event) | Y (direct call) | Implemented (different mechanism) |

### 10.4 API Endpoints

| Endpoint | Go | Rust | Status |
|----------|-----|------|--------|
| GET /integrations | Y | Y | Implemented |
| POST /integrations | Y | Y | Implemented |
| GET /integrations/registry | Y (DB query) | Y (hardcoded) | Different source |
| GET /integrations/tools | Y (OAuth only) | Y (all tools) | Different scope |
| GET /integrations/{id} | Y | Y | Implemented |
| PUT /integrations/{id} | Y | Y | Implemented |
| DELETE /integrations/{id} | Y | Y | Implemented |
| POST /integrations/{id}/test | Y (full MCP test) | Y (HTTP reachability only) | Partial |
| GET /integrations/{id}/oauth-url | Y | N | NOT implemented |
| POST /integrations/{id}/disconnect | Y | N | NOT implemented |
| GET /integrations/oauth/callback | Y | N | NOT implemented |

### 10.5 Encryption

| Feature | Go | Rust | Status |
|---------|-----|------|--------|
| AES-256-GCM encrypt/decrypt | Y | Y | Implemented |
| Base64 encoding | Y | Y | Implemented |
| OS Keychain priority | Y | N | NOT implemented |
| Env var key resolution | Y | Y | Implemented (different derivation) |
| File key persistence | Y | Y | Implemented |
| Key generation | Y | Y | Implemented |
| Credential migration (plaintext -> encrypted) | Y | N | NOT implemented |
| Token encrypt/decrypt on McpClient | Y | Y | Implemented |

### 10.6 Priority Implementation Backlog

The following items are listed in recommended implementation order:

1. **Path traversal validation** in `serve_file` -- security-critical
2. **Screenshot save to files/** -- switch from base64 data URI to file-based URL
3. **Cache-Control headers** on file responses
4. **.webp Content-Type** mapping
5. **OAuth flow** -- PKCE, token exchange, callback handler, refresh
6. **Health checker** -- background task to validate cached MCP sessions
7. **Periodic re-sync** -- 15-minute ticker for bridge sync_all
8. **Retry with backoff** on `call_tool` failures
9. **Session staleness** checks (30min age, 10min idle)
10. **Credential migration** -- plaintext to encrypted upgrade on startup
11. **OS Keychain** integration for key storage
12. **Full test handler** -- connect via MCP client, list tools, update DB status

---

## Key Files Reference

### Go Source Files

| File | Purpose |
|------|---------|
| `internal/handler/files/filehandler.go` | Serve files + native file picker |
| `internal/agent/tools/screenshot.go` | Capture, file save, ImageURL |
| `internal/agent/tools/registry.go` | ToolResult struct |
| `internal/realtime/chat.go` | contentBlock, image capture, metadata |
| `internal/handler/integration/handler.go` | CRUD + test + notify |
| `internal/handler/integration/oauth.go` | OAuth URL, disconnect, callback |
| `internal/mcp/client/client.go` | OAuth 2.1 (PKCE, exchange, refresh) |
| `internal/mcp/client/transport.go` | Sessions, health checker |
| `internal/mcp/client/crypto.go` | AES-256-GCM encryption |
| `internal/mcp/bridge/bridge.go` | Sync, register proxy tools |
| `internal/credential/migrate.go` | Plaintext -> encrypted migration |

### Rust Source Files

| File | Purpose |
|------|---------|
| `crates/server/src/handlers/files.rs` | Serve files + directory browser |
| `crates/server/src/handlers/integrations.rs` | CRUD + test + bridge sync |
| `crates/server/src/lib.rs` | Routes, bridge init |
| `crates/server/src/state.rs` | AppState (bridge field) |
| `crates/mcp/src/types.rs` | McpError, McpToolDef, OAuthTokens |
| `crates/mcp/src/client.rs` | OAuth discovery, list/call tools |
| `crates/mcp/src/bridge.rs` | sync_all, ProxyToolRegistry |
| `crates/mcp/src/crypto.rs` | AES-256-GCM, key resolution |
| `crates/tools/src/registry.rs` | ToolResult (image_url) |
| `crates/tools/src/desktop_tool.rs` | macOS screenshot (base64) |
| `crates/db/src/queries/mcp_integrations.rs` | SQLite queries |
| `crates/db/migrations/0018_*.sql` | Core tables |
| `crates/db/migrations/0025_*.sql` | OAuth columns |
| `crates/db/migrations/0029_*.sql` | tool_count column |

---

## Known Issues and Quirks

1. **OAuth callback redirect mismatch (Go):** Callback redirects to `/settings/mcp?connected={id}`, but the MCP page immediately redirects to `/settings/integrations`. Works but adds an unnecessary hop.

2. **ListMCPToolsHandler scope difference:** Go only lists OAuth tools with `connectionStatus == "connected"`. Rust lists all registered tools regardless of source.

3. **No token revocation on disconnect (Go):** `Disconnect()` deletes credentials and clears state but does NOT call the server's revocation endpoint.

4. **Dynamic client registration fallback (Go):** If dynamic registration fails, falls back to `"nebo-agent-{integrationID}"` as a public client_id. This placeholder may NOT work with strict OAuth servers.

5. **Proxy tools always require approval (Go):** All MCP proxy tools have `RequiresApproval() = true`, regardless of the external tool's nature. Intentional for security.

6. **Infinite retry on CallTool (Go):** The retry loop never gives up -- only context cancellation stops it. At 10min max backoff, a permanently broken server retries ~6 times/hour forever.

7. **Server registry not used by wizard:** The pre-seeded `mcp_server_registry` table exists but the frontend wizard does NOT reference it. The wizard is purely URL-based.

8. **Tool count staleness:** `tool_count` is only updated on explicit `test` or `bridge.Connect()` -- NOT on periodic re-syncs.

9. **Rust screenshot uses base64 data URI:** Unlike Go which saves files and returns URLs, Rust embeds the full PNG as a data URI. This inflates DB storage and WS message size.

10. **Rust file handler missing path traversal checks:** The `serve_file` handler does NOT canonicalize the path or verify it stays within the files directory. This is a security gap.

11. **Rust MCP session cache uses RwLock:** Go uses `sync.Map` for lockless reads. Rust uses `tokio::sync::RwLock<HashMap>`, which is correct for async but may have different contention characteristics under high concurrent tool calls.

12. **No file cleanup/rotation (both):** Files accumulate in `<data_dir>/files/` indefinitely. No TTL, no size limit, no cleanup mechanism.

---

*Generated: 2026-03-04*
