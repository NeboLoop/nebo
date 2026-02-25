# Integrations System — SME Deep-Dive

**Last updated:** 2026-02-24

Integrations = connecting Nebo's agent to **external MCP (Model Context Protocol) servers** so the agent gains new tools dynamically. Think of it as Nebo's plugin system for third-party services: Notion, GitHub, Linear, Slack, or any custom Streamable HTTP MCP server.

---

## Architecture Overview

```
┌──────────────────────────────────────────────────────────────────────┐
│  FRONTEND (Settings > Integrations)                                  │
│  app/src/routes/(app)/settings/integrations/+page.svelte             │
│  3-step wizard: URL → Auth → Name                                    │
│  Calls: list, create, update, delete, test, getOAuthURL              │
└─────────────────────────┬────────────────────────────────────────────┘
                          │ HTTP REST
                          ▼
┌──────────────────────────────────────────────────────────────────────┐
│  HANDLER (internal/handler/integration/)                             │
│  handler.go — CRUD + test + notify                                   │
│  oauth.go   — OAuth URL, disconnect, tools list, callback            │
│  Routes:  /api/v1/integrations/*  (12 endpoints, all protected)      │
└─────────────────────────┬────────────────────────────────────────────┘
                          │ svcCtx.DB + svcCtx.MCPClient
                          ▼
┌──────────────────────────────────────────────────────────────────────┐
│  MCP CLIENT (internal/mcp/client/)                                   │
│  client.go    — OAuth 2.1 flows (discover, PKCE, exchange, refresh)  │
│  transport.go — Session management, Streamable HTTP, health checks   │
│  callback.go  — OAuth redirect handler                               │
│  crypto.go    — AES-256-GCM encryption, key management (keychain)    │
└─────────────────────────┬────────────────────────────────────────────┘
                          │ mcp.ClientSession via go-sdk
                          ▼
┌──────────────────────────────────────────────────────────────────────┐
│  MCP BRIDGE (internal/mcp/bridge/bridge.go)                          │
│  Syncs enabled integrations → registers proxy tools in agent         │
│  Tool naming: mcp__{serverType}__{toolName}                          │
│  Wired to agent via cmd/nebo/agent.go                                │
└──────────────────────────────────────────────────────────────────────┘
```

---

## Database Schema (3 tables)

### `mcp_integrations` (migration 0018 + 0025 + 0029)
| Column | Type | Notes |
|--------|------|-------|
| id | TEXT PK | UUID |
| name | TEXT | User-friendly label |
| server_type | TEXT | hostname-derived or "custom" |
| server_url | TEXT | Streamable HTTP endpoint |
| auth_type | TEXT | "oauth", "api_key", "none" |
| is_enabled | INTEGER | 0/1 toggle |
| connection_status | TEXT | "connected", "disconnected", "error" |
| last_connected_at | INTEGER | unix epoch |
| last_error | TEXT | last failure message |
| metadata | TEXT | JSON for extra config |
| tool_count | INTEGER | cached count from last sync |
| oauth_state | TEXT | CSRF state for in-flight OAuth |
| oauth_pkce_verifier | TEXT | encrypted PKCE verifier |
| oauth_client_id | TEXT | from dynamic registration or default |
| oauth_client_secret | TEXT | encrypted, nullable for public clients |
| oauth_authorization_endpoint | TEXT | discovered from .well-known |
| oauth_token_endpoint | TEXT | discovered from .well-known |
| oauth_registration_endpoint | TEXT | for dynamic client reg |

### `mcp_integration_credentials`
| Column | Type | Notes |
|--------|------|-------|
| id | TEXT PK | UUID |
| integration_id | TEXT FK | CASCADE delete |
| credential_type | TEXT | "api_key" or "oauth_token" |
| credential_value | TEXT | AES-256-GCM encrypted |
| refresh_token | TEXT | encrypted, nullable |
| expires_at | INTEGER | unix epoch for token expiry |
| scopes | TEXT | comma-separated |

### `mcp_server_registry` (pre-populated catalog)
Pre-seeded entries: notion, github, linear, slack, filesystem, memory.
Used for display metadata (icons, API key URLs, OAuth scopes).
**Not actively used by the frontend wizard** — the wizard is URL-first, not registry-first.

---

## API Endpoints (12 routes, all under `/api/v1/integrations`)

| Method | Path | Handler | Purpose |
|--------|------|---------|---------|
| GET | `/integrations` | ListMCPIntegrationsHandler | List all integrations |
| POST | `/integrations` | CreateMCPIntegrationHandler | Create new integration |
| GET | `/integrations/registry` | ListMCPServerRegistryHandler | List known server catalog |
| GET | `/integrations/tools` | ListMCPToolsHandler | List tools from all connected OAuth integrations |
| GET | `/integrations/{id}` | GetMCPIntegrationHandler | Get single integration |
| PUT | `/integrations/{id}` | UpdateMCPIntegrationHandler | Update name/URL/enabled/apiKey |
| DELETE | `/integrations/{id}` | DeleteMCPIntegrationHandler | Delete integration + credentials |
| POST | `/integrations/{id}/test` | TestMCPIntegrationHandler | Test connection, update status + tool count |
| GET | `/integrations/{id}/oauth-url` | GetMCPOAuthURLHandler | Get OAuth authorization URL |
| POST | `/integrations/{id}/disconnect` | DisconnectMCPIntegrationHandler | Revoke tokens, clear creds |
| GET | `/integrations/oauth/callback` | OAuthCallbackHandler | OAuth redirect endpoint |

All routes are **protected** (JWT required) — registered in `registerProtectedRoutes()`.

---

## Frontend: 3-Step Add Wizard

The `+page.svelte` implements a multi-step modal:

1. **Step 1: URL** — User enters the MCP server's Streamable HTTP endpoint. Validated as http/https URL.
2. **Step 2: Auth** — Radio selection: OAuth (recommended), API Key, or None. API Key shows inline password input.
3. **Step 3: Name** — Optional friendly name (defaults to hostname). Shows summary of URL + auth type.

On submit:
- Calls `POST /integrations` to create the DB record
- If auth=oauth, immediately calls `GET /integrations/{id}/oauth-url` → redirects browser to external auth server
- OAuth callback redirects back to `/settings/mcp?connected={id}` (note: redirects to /mcp, not /integrations — **minor routing mismatch**, MCP page redirects to integrations via `onMount`)

### Integration List Card
Each integration shows:
- Colored initial avatar (first letter of name)
- Name + auth type badge (OAUTH/API KEY/NONE)
- Server URL
- Connection status icon (green check / red X / gray X)
- Tool count when connected
- Last error if any
- Actions: Test button, dropdown menu (Enable/Disable, Delete)

### State Management
All state is component-local using Svelte 5 `$state` runes. No stores — data is fetched on mount and after mutations via `loadIntegrations()`.

---

## OAuth 2.1 Flow (Full Sequence)

```
User clicks "Connect with OAuth"
  │
  ├─ Frontend: POST /integrations (create record)
  ├─ Frontend: GET /integrations/{id}/oauth-url
  │     │
  │     ▼ MCP Client: StartOAuthFlow()
  │     ├─ Discover: GET {serverURL}/.well-known/oauth-authorization-server
  │     ├─ Generate PKCE (S256 challenge)
  │     ├─ Generate random state (CSRF protection)
  │     ├─ Get or create client credentials:
  │     │   1. Check DB for existing client_id/secret
  │     │   2. Try dynamic client registration (RFC 7591)
  │     │   3. Fall back to "nebo-agent-{integrationID}" public client
  │     ├─ Encrypt PKCE verifier + client secret with AES-256-GCM
  │     ├─ Store flow state in mcp_integrations columns
  │     └─ Return authorization URL with params:
  │         response_type=code, client_id, redirect_uri, state,
  │         code_challenge, code_challenge_method=S256, scope
  │
  ├─ Browser redirects to external auth server
  ├─ User authenticates and grants consent
  │
  └─ External server redirects to:
      GET /api/v1/integrations/oauth/callback?code=...&state=...
        │
        ▼ OAuthCallbackHandler
        ├─ Validate state (CSRF) — lookup integration by oauth_state
        ├─ ExchangeCode():
        │   ├─ Decrypt PKCE verifier
        │   ├─ POST to token_endpoint with:
        │   │   grant_type=authorization_code, code, redirect_uri,
        │   │   client_id, code_verifier
        │   └─ Store tokens: encrypt access_token + refresh_token
        ├─ Clear OAuth state columns
        ├─ Set connection_status = "connected"
        ├─ Notify agent: integrations_changed event
        └─ Redirect to /settings/mcp?connected={id}
```

### Token Refresh
- `GetAccessToken()` checks `expires_at - 60s` (1min buffer)
- If expired and refresh_token exists → `RefreshToken()` → POST to token_endpoint with `grant_type=refresh_token`
- If refresh response omits refresh_token, preserves the old one

---

## MCP Client Sessions (`transport.go`)

### Session Caching
- Global `sync.Map` keyed by integrationID → `*sessionEntry`
- Each entry: `mcp.ClientSession`, createdAt, lastHeartbeat
- Health check: session invalid if >30min old OR >10min since last use
- Double-checked locking on creation (fast path sync.Map, slow path mutex)

### Health Checker
- Background goroutine, ticks every 5 minutes
- Validates all cached sessions, reconnects stale ones
- Started via `StartHealthChecker()` in agent.go

### AuthenticatedTransport
- Custom `http.RoundTripper` that injects `Authorization: Bearer {token}`
- Token fetched/refreshed per-request via `GetAccessToken()`
- Only wrapped when `authType != "none"` — public servers skip auth header

### MCP SDK Integration
- Uses `modelcontextprotocol/go-sdk` — official Go MCP SDK
- `mcp.StreamableClientTransport` for Streamable HTTP transport
- `mcp.NewClient()` with implementation name "nebo", version "1.0.0"
- `client.Connect()` returns `*mcp.ClientSession`

### Retry Strategy (CallTool)
- Infinite retry with exponential backoff: 100ms base, 2^n scaling, max 10min
- ±25% jitter to prevent thundering herd at scale (1M+ users)
- Only stops on context cancellation
- Each retry closes stale session and reconnects

---

## MCP Bridge (`bridge.go`)

The bridge is the critical piece that makes external MCP tools available to the agent.

### How It Works
1. `SyncAll()` loads all enabled integrations from DB
2. Disconnects integrations that were removed/disabled
3. For each enabled integration with a server URL:
   - Calls `mcpClient.ListTools()` to discover tools
   - Creates `proxyTool` for each tool
   - Registers in agent's `tools.Registry` with namespaced name
4. Proxy tools forward `Execute()` calls to external MCP server via `mcpClient.CallTool()`

### Tool Naming Convention
```
mcp__{serverType}__{toolName}
```
Example: `mcp__github.com__search_repos`, `mcp__linear.app__create_issue`

### Proxy Tool Properties
- `RequiresApproval() = true` — always needs user approval
- Schema passed through from MCP server's InputSchema
- Execute extracts TextContent from MCP result

### Sync Triggers (3 mechanisms)
1. **Initial sync** — goroutine on agent start
2. **Periodic re-sync** — every 15 minutes via ticker
3. **Event-driven** — on `integrations_changed` WebSocket event from API handler

### Integration with Agent
```go
// cmd/nebo/agent.go ~line 1919
mcpBridge := mcpbridge.New(registry, db.New(sqlDB), opts.SvcCtx.MCPClient)
state.mcpBridge = mcpBridge
opts.SvcCtx.MCPClient.StartHealthChecker(ctx)
// Initial sync (goroutine)
go mcpBridge.SyncAll(ctx)
// Periodic sync (15 min)
go func() { ticker... mcpBridge.SyncAll(ctx) }()
defer mcpBridge.Close()
```

Agent event handler (~line 2809):
```go
case "integrations_changed":
    go state.mcpBridge.SyncAll(ctx)
```

---

## Encryption & Key Management (`crypto.go`)

All credentials stored at rest use **AES-256-GCM** encryption.

### Key Resolution Priority
1. **OS Keychain** (macOS Keychain, Windows Credential Manager, Linux Secret Service)
2. `MCP_ENCRYPTION_KEY` env var (hex-encoded 32 bytes)
3. `JWT_SECRET` env var (first 32 bytes)
4. Persistent file `{dataDir}/.mcp-key`
5. Generate new random key

Keys found in env/file are **promoted to keychain** automatically (and file deleted).

### What Gets Encrypted
- API keys (on create/update)
- OAuth access tokens
- OAuth refresh tokens
- PKCE verifiers (during OAuth flow)
- Client secrets (from dynamic registration)

### Credential Migration
`credential/migrate.go` handles upgrading plaintext creds to encrypted.
Runs on startup, idempotent (skips `enc:` prefixed values).
Covers: auth_profiles, mcp_integration_credentials, app_oauth_grants, plugin_settings.

---

## Notification Pipeline

When any integration is created/updated/deleted/tested:

```
Handler calls notifyIntegrationsChanged(svcCtx)
  → svcCtx.AgentHub.Broadcast({Type: "event", Method: "integrations_changed"})
    → Agent's event handler receives it
      → go state.mcpBridge.SyncAll(ctx)
        → Disconnects removed, connects new, registers/unregisters proxy tools
```

This means tool additions/removals propagate to the agent **within seconds** of a settings change.

---

## Key Files Reference

| File | Purpose |
|------|---------|
| `app/src/routes/(app)/settings/integrations/+page.svelte` | Frontend UI — list, add wizard, test, delete |
| `internal/handler/integration/handler.go` | CRUD handlers + test + notify |
| `internal/handler/integration/oauth.go` | OAuth URL, disconnect, tools list, callback |
| `internal/mcp/client/client.go` | OAuth 2.1 client (discover, PKCE, exchange, refresh, disconnect) |
| `internal/mcp/client/transport.go` | MCP session management, SDK transport, health checker, ListTools, CallTool |
| `internal/mcp/client/callback.go` | OAuth redirect handlers (HTML redirect + JSON) |
| `internal/mcp/client/crypto.go` | AES-256-GCM encryption + key management |
| `internal/mcp/bridge/bridge.go` | Bridge: syncs integrations → registers proxy tools in agent |
| `internal/types/types.go:972-1080` | All integration-related request/response types |
| `internal/db/queries/mcp_integrations.sql` | 16 SQL queries |
| `internal/db/mcp_integrations.sql.go` | sqlc-generated Go code |
| `internal/db/models.go:206-227` | McpIntegration model (20 columns) |
| `internal/db/migrations/0018_integrations_channels.sql` | Core tables + registry seed data |
| `internal/db/migrations/0025_mcp_oauth_client.sql` | OAuth state columns |
| `internal/db/migrations/0029_mcp_tool_count.sql` | tool_count column |
| `internal/credential/migrate.go` | Plaintext → encrypted migration |
| `internal/svc/servicecontext.go:75,381` | MCPClient initialization in service context |
| `cmd/nebo/agent.go:1919-1949,2809-2817` | Bridge wiring + integrations_changed handler |
| `app/src/lib/api/nebo.ts:438-511` | TypeScript API client functions |
| `internal/server/server.go:410-421` | Route registration |

---

## Known Issues & Quirks

1. **OAuth callback redirect mismatch**: Callback redirects to `/settings/mcp?connected={id}`, but the MCP page (`mcp/+page.svelte`) immediately redirects to `/settings/integrations`. This works but adds an unnecessary hop.

2. **ListMCPToolsHandler only lists OAuth tools**: The tools endpoint at `GET /integrations/tools` only queries integrations with `authType == "oauth"` AND `connectionStatus == "connected"`. API key integrations are excluded from this endpoint (though they work fine via the bridge).

3. **No token revocation on disconnect**: `Disconnect()` has a TODO comment — it deletes credentials and clears state but doesn't call the server's revocation endpoint.

4. **Dynamic client registration fallback**: If dynamic registration fails AND no registration endpoint exists, falls back to `"nebo-agent-{integrationID}"` as a public client_id. This is a placeholder that may not work with strict OAuth servers.

5. **Proxy tools always require approval**: All MCP proxy tools have `RequiresApproval() = true`, regardless of the external tool's nature. This is intentional (security) but may feel heavy for read-only tools.

6. **Infinite retry on CallTool**: The retry loop in `CallTool()` never gives up — only context cancellation stops it. At 10min max backoff, a permanently broken server will retry ~6 times/hour forever.

7. **Server registry not used by wizard**: The pre-seeded `mcp_server_registry` table exists but the frontend wizard doesn't reference it. The wizard is purely URL-based.

8. **Tool count staleness**: `tool_count` in the DB is only updated on explicit `test` or `bridge.Connect()` — not on periodic re-syncs (SyncAll calls Connect which does update it, but only for integrations that pass the auth/status filter).
