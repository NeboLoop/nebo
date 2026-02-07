# ADR-005: External MCP Tool Connections

**Status:** Proposed  
**Date:** 2026-02-07  
**Author:** Architecture Team  
**Tags:** mcp, integrations, tools, agent

---

## Context

Nebo needs a simple way to connect to external MCP servers so the agent gains tools from third-party sources (Notion, GitHub, Linear, custom servers, etc.). We have most of the plumbing built but it's not actually working end-to-end. This document audits what exists, identifies the gaps, and proposes the minimum changes to make it functional.

---

## What Already Exists

### Backend — Solid Foundation

| Layer | Status | Location |
|-------|--------|----------|
| **DB Schema** | ✅ Complete | `0018_integrations_channels.sql`, `0025_mcp_oauth_client.sql` |
| **DB Queries** | ✅ Complete | `internal/db/queries/mcp_integrations.sql` — CRUD, OAuth flow, credentials, registry |
| **API Endpoints** | ✅ Complete | `internal/server/server.go` lines 312-322 — full REST for integrations |
| **Handlers** | ✅ Complete | `internal/handler/integration/handler.go` — List, Create, Update, Delete, Test |
| **OAuth Client** | ✅ Complete | `internal/mcp/client/client.go` — Discover, PKCE, Dynamic Client Reg, Token Exchange, Refresh |
| **OAuth Callback** | ✅ Complete | `internal/mcp/client/callback.go` |
| **Auth Transport** | ✅ Complete | `internal/mcp/client/transport.go` — `AuthenticatedTransport` adds Bearer tokens |
| **Tool Proxy** | ⚠️ Partial | `transport.go` has `ListTools()` and `CallTool()` but uses raw HTTP, not the MCP SDK client |
| **Types** | ✅ Complete | `internal/types/types.go` — `MCPIntegration`, `MCPServerInfo`, request/response types |
| **MCP SDK** | ✅ Available | `github.com/modelcontextprotocol/go-sdk v1.2.0` in `go.mod` |
| **Server Registry** | ✅ Seeded | `mcp_server_registry` table pre-populated with Notion, GitHub, Linear, Slack, Filesystem, Memory |

### Frontend — Basic UI Exists

| Component | Status | Location |
|-----------|--------|----------|
| **Settings Page** | ✅ Complete | `app/src/routes/(app)/settings/mcp/+page.svelte` |
| **API Functions** | ✅ Generated | `app/src/lib/api/nebo.ts` — all CRUD + OAuth + test functions |
| **Add Modal** | ✅ Working | Service selection, custom URL, API key input |
| **Integration List** | ✅ Working | Shows status, test button, enable/disable, delete |
| **Sidebar Link** | ✅ Added | `Settings → MCP` |

### What's Missing (The Gaps)

| Gap | Severity | Description |
|-----|----------|-------------|
| **🔴 No tool bridge** | Critical | External MCP tools are never registered in the agent's `tools.Registry`. The agent has no way to call them. |
| **🔴 Test handler is fake** | Critical | `TestMCPIntegrationHandler` just checks if credentials exist — never actually connects to the server. |
| **🟡 Raw HTTP, not MCP SDK** | Medium | `transport.go` uses raw `POST /tools/list` and `POST /tools/call` — should use the MCP SDK's `ClientSession` for proper JSON-RPC and capability negotiation. |
| **🟡 No startup sync** | Medium | When Nebo starts, enabled integrations aren't loaded and their tools aren't registered. |
| **🟡 No tool namespacing** | Medium | If two MCP servers both expose a `search` tool, names will collide in the registry. |
| **🟡 Custom server needs transport type** | Medium | The "Custom MCP Server" option in the Add Modal doesn't let you pick `stdio` vs `http` transport. |
| **🟢 No connection health check** | Low | Connected integrations don't periodically verify they're still reachable. |
| **🟢 OAuth scopes config** | Low | No UI for customizing OAuth scopes per integration. |

---

## Decision

### Phase 1: Make It Work (MVP)

Ship the **minimum changes** to get external MCP tools usable by the agent.

#### 1.1 MCP Bridge Tool (`internal/agent/tools/mcp_bridge.go`)

A new tool that registers external MCP server tools into the agent's `tools.Registry`.

```
┌──────────────────────────────────────────────────────────┐
│  Agent Tool Registry                                     │
│                                                          │
│  file, shell, web, agent (built-in STRAP tools)          │
│  screenshot, vision, calendar, ... (platform tools)      │
│                                                          │
│  mcp__notion__search       ← bridge from Notion MCP      │
│  mcp__notion__create_page  ← bridge from Notion MCP      │
│  mcp__github__list_repos   ← bridge from GitHub MCP      │
│  mcp__github__create_issue ← bridge from GitHub MCP      │
│  mcp__custom__my_tool      ← bridge from custom server   │
│                                                          │
└──────────────────────────────────────────────────────────┘
```

**Naming convention:** `mcp__{server_type}__{tool_name}`

- Prevents collisions between servers
- Makes it clear to the LLM which integration a tool comes from
- Matches the `mcp__server__tool` pattern already used by Claude CLI

**Implementation pattern:**

```go
// MCPBridge manages connections to external MCP servers and registers their tools.
type MCPBridge struct {
    registry   *tools.Registry
    db         *db.Store
    mcpClient  *mcpclient.Client
    mu         sync.Mutex
    sessions   map[string]*mcp.ClientSession  // integrationID → live session
}

// Sync loads all enabled integrations, connects to their MCP servers,
// discovers tools, and registers them in the agent's tool registry.
func (b *MCPBridge) Sync(ctx context.Context) error { ... }

// Connect connects to a single integration's MCP server and registers tools.
func (b *MCPBridge) Connect(ctx context.Context, integrationID string) error { ... }

// Disconnect removes all tools from an integration and closes the session.
func (b *MCPBridge) Disconnect(integrationID string) error { ... }
```

Each discovered tool gets registered as a standalone `tools.Tool` that proxies calls through the MCP SDK `ClientSession.CallTool()`.

#### 1.2 Use MCP SDK Client (Replace Raw HTTP)

Replace the raw HTTP calls in `internal/mcp/client/transport.go` with the MCP SDK's `ClientSession`:

```go
import "github.com/modelcontextprotocol/go-sdk/mcp"

// Connect establishes a ClientSession to the external MCP server.
func (b *MCPBridge) connect(ctx context.Context, serverURL string, transport http.RoundTripper) (*mcp.ClientSession, error) {
    client := mcp.NewClient(&mcp.Implementation{
        Name:    "nebo-agent",
        Version: "1.0.0",
    }, nil)
    
    session, err := client.Connect(ctx, mcp.NewStreamableHTTPTransport(serverURL, transport))
    if err != nil {
        return nil, err
    }
    
    return session, nil
}
```

This gives us:
- Proper JSON-RPC framing
- MCP capability negotiation
- Server-sent tool list changes (notifications/tools/list_changed)
- Session management

#### 1.3 Real Test Connection

Replace the fake test handler with actual connection verification:

```go
func TestMCPIntegrationHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
    return func(w http.ResponseWriter, r *http.Request) {
        // 1. Get integration + credentials
        // 2. Create authenticated transport
        // 3. Attempt MCP session connection (with 10s timeout)
        // 4. Call tools/list to verify we get a response
        // 5. Update connection_status and last_connected_at
        // 6. Return tool count in response
    }
}
```

#### 1.4 Startup Sync

On agent boot, after tool registry is initialized:

```go
// In cmd/nebo/agent.go or wherever the agent starts:
bridge := tools.NewMCPBridge(registry, db, mcpClient)
bridge.Sync(ctx)  // Connect to all enabled integrations, register their tools
```

Also trigger sync when:
- An integration is created/enabled via the API
- An OAuth flow completes (callback handler)
- An integration is disabled/deleted (unregister tools)

#### 1.5 Frontend: Add Transport Type for Custom Servers

The "Custom MCP Server" option needs a transport picker:

```svelte
{#if selectedType === 'custom'}
    <select bind:value={transportType}>
        <option value="http">HTTP (Streamable HTTP)</option>
        <option value="stdio">Stdio (Local command)</option>
        <option value="sse">SSE (Server-Sent Events)</option>
    </select>
    
    {#if transportType === 'http'}
        <input placeholder="http://localhost:8080/mcp" ... />
    {:else if transportType === 'stdio'}
        <input placeholder="npx @modelcontextprotocol/server-filesystem /path" ... />
    {/if}
{/if}
```

For `stdio` transport, Nebo spawns the MCP server process and communicates over stdin/stdout using the MCP SDK's stdio transport.

### Phase 2: Polish (Post-MVP)

| Feature | Description |
|---------|-------------|
| **Health monitoring** | Heartbeat lane checks connected integrations every 5 minutes |
| **Tool caching** | Cache tool schemas in DB so agent starts fast (lazy reconnect) |
| **Scoped permissions** | Per-integration tool policies (e.g., GitHub read-only) |
| **Dynamic updates** | Listen for `tools/list_changed` notifications and re-sync |
| **OAuth scope UI** | Let users customize scopes during OAuth setup |
| **Tool explorer** | Settings page shows discovered tools per integration |
| **Error surfacing** | Show connection errors in the MCP settings page without requiring a test click |

---

## Architecture

### Data Flow: Adding a New MCP Integration

```
User                     Frontend              API                 MCPBridge            External MCP
  │                         │                    │                     │                     │
  │ Select "Notion"         │                    │                     │                     │
  │ Enter API key           │                    │                     │                     │
  │ Click "Add"             │                    │                     │                     │
  │────────────────────────>│                    │                     │                     │
  │                         │ POST /integrations │                     │                     │
  │                         │───────────────────>│                     │                     │
  │                         │                    │ Save to DB          │                     │
  │                         │                    │ Trigger bridge      │                     │
  │                         │                    │────────────────────>│                     │
  │                         │                    │                     │ MCP Initialize      │
  │                         │                    │                     │────────────────────>│
  │                         │                    │                     │ tools/list          │
  │                         │                    │                     │────────────────────>│
  │                         │                    │                     │<────────────────────│
  │                         │                    │                     │ [search, create_page│
  │                         │                    │                     │  update_page, ...]  │
  │                         │                    │                     │                     │
  │                         │                    │                     │ Register:           │
  │                         │                    │                     │ mcp__notion__search │
  │                         │                    │                     │ mcp__notion__create │
  │                         │                    │                     │ ...                 │
  │                         │                    │<────────────────────│                     │
  │                         │ { connected, 4 tools }                  │                     │
  │                         │<───────────────────│                     │                     │
  │ "Notion connected       │                    │                     │                     │
  │  (4 tools)"             │                    │                     │                     │
  │<────────────────────────│                    │                     │                     │
```

### Data Flow: Agent Using an External MCP Tool

```
User: "Create a page in Notion about our Q1 goals"
  │
  ▼
Agent LLM sees tool: mcp__notion__create_page
  │
  ▼
Agent calls: registry.Execute(ctx, "mcp__notion__create_page", input)
  │
  ▼
MCPBridge proxy tool:
  1. Look up session for "notion" integration
  2. session.CallTool(ctx, "create_page", input)
  3. MCP SDK handles JSON-RPC, auth headers, retries
  │
  ▼
External Notion MCP server processes and returns result
  │
  ▼
Agent receives result, continues conversation
```

### Transport Support

| Transport | Use Case | MCP SDK Support |
|-----------|----------|-----------------|
| **Streamable HTTP** | Remote servers (Notion, GitHub, etc.) | `mcp.NewStreamableHTTPTransport()` |
| **SSE** | Legacy remote servers | `mcp.NewSSETransport()` |
| **Stdio** | Local servers (`npx`, `uvx`, Docker) | `mcp.NewStdioTransport()` |

For stdio, the `MCPBridge` manages the child process lifecycle:
- Spawns the command when connecting
- Kills the process when disconnecting
- Restarts on crash (with backoff)

---

## Database Changes

### Add `transport_type` and `command` columns to `mcp_integrations`

```sql
-- +goose Up
ALTER TABLE mcp_integrations ADD COLUMN transport_type TEXT NOT NULL DEFAULT 'http';
-- http, stdio, sse
ALTER TABLE mcp_integrations ADD COLUMN command TEXT;
-- For stdio: the command to spawn (e.g., "npx @modelcontextprotocol/server-notion")
ALTER TABLE mcp_integrations ADD COLUMN command_args TEXT;
-- JSON array of args
ALTER TABLE mcp_integrations ADD COLUMN command_env TEXT;
-- JSON object of env vars (e.g., {"NOTION_API_KEY": "..."})
```

### Add `tool_count` column for display

```sql
ALTER TABLE mcp_integrations ADD COLUMN tool_count INTEGER DEFAULT 0;
```

---

## API Changes

### Update `CreateMCPIntegrationRequest`

```go
type CreateMCPIntegrationRequest struct {
    Name          string `json:"name"`
    ServerType    string `json:"serverType"`
    ServerUrl     string `json:"serverUrl,omitempty"`
    AuthType      string `json:"authType"`
    ApiKey        string `json:"apiKey,omitempty"`
    TransportType string `json:"transportType,omitempty"` // NEW: http, stdio, sse
    Command       string `json:"command,omitempty"`       // NEW: for stdio transport
    CommandArgs   []string `json:"commandArgs,omitempty"` // NEW: for stdio transport
    CommandEnv    map[string]string `json:"commandEnv,omitempty"` // NEW: for stdio env vars
}
```

### Update `TestMCPIntegrationResponse`

```go
type TestMCPIntegrationResponse struct {
    Success   bool     `json:"success"`
    Message   string   `json:"message"`
    ToolCount int      `json:"toolCount,omitempty"` // NEW
    Tools     []string `json:"tools,omitempty"`     // NEW: tool names discovered
}
```

### New endpoint: `POST /integrations/{id}/reconnect`

Force-reconnect: disconnect existing session, re-connect, re-discover tools.

---

## File Changes Summary

| File | Change |
|------|--------|
| `internal/agent/tools/mcp_bridge.go` | **NEW** — MCPBridge, proxy tools, session management |
| `internal/agent/tools/registry.go` | Add `OnChange` callback support (already exists, verify) |
| `internal/handler/integration/handler.go` | Real test connection, trigger bridge sync on create/update/delete |
| `internal/mcp/client/transport.go` | Replace raw HTTP with MCP SDK `ClientSession` |
| `internal/types/types.go` | Add `TransportType`, `Command`, `CommandArgs`, `CommandEnv`, `ToolCount` fields |
| `internal/db/migrations/00XX_mcp_transport.sql` | **NEW** — Add columns |
| `internal/db/queries/mcp_integrations.sql` | Update queries for new columns |
| `cmd/nebo/agent.go` | Initialize MCPBridge, call `Sync()` on startup |
| `app/src/routes/(app)/settings/mcp/+page.svelte` | Transport picker for custom servers, show tool count |
| `app/src/lib/api/neboComponents.ts` | Regenerate with `make gen` |

---

## User Experience Flow

### Adding a Known Service (e.g., Notion)

1. Go to **Settings → MCP**
2. Click **Add Integration**
3. Select **Notion** from dropdown
4. Enter API key (link to Notion's integration page provided)
5. Click **Add Integration**
6. See "Notion — Connected (4 tools)" in the list
7. Agent now has `mcp__notion__search`, `mcp__notion__create_page`, etc.

### Adding a Custom MCP Server

1. Go to **Settings → MCP**
2. Click **Add Integration**
3. Select **Custom MCP Server**
4. Choose transport: **HTTP** / **Stdio** / **SSE**
5. For HTTP: enter URL (e.g., `http://localhost:3001/mcp`)
6. For Stdio: enter command (e.g., `npx -y @modelcontextprotocol/server-filesystem /Users/me/docs`)
7. Click **Add Integration**
8. See tool count and connection status

### Using Connected Tools

User just talks naturally:

> "Search my Notion for the product roadmap"

Agent sees `mcp__notion__search` in its tool list, calls it, returns results. No special syntax needed.

---

## Security Considerations

1. **API keys encrypted at rest** — Already handled by `internal/mcp/client/crypto.go`
2. **Stdio commands sandboxed** — Only allow user-approved commands; show confirmation dialog
3. **Tool policies apply** — External MCP tools go through the same `Policy` system as built-in tools
4. **Origin tracking** — External MCP tool calls get `OriginPlugin` or a new `OriginMCP` origin for policy enforcement
5. **No auto-approval** — New integrations require explicit user action to add
6. **Credential rotation** — OAuth refresh tokens handled automatically; API keys require manual update

---

## Alternatives Considered

### A: JSON config file (`mcp_servers.json`)

Like Claude Desktop's approach. User edits a JSON file to add MCP servers.

**Rejected because:**
- No UI, not user-friendly
- Can't do OAuth flows
- No credential encryption
- We already have DB + UI, just need to wire it up

### B: Register external tools as a single meta-tool

One `mcp` STRAP tool: `mcp(resource: "notion", action: "search", query: "...")`.

**Rejected because:**
- LLM can't see individual tool schemas — would need to guess parameters
- Defeats the purpose of MCP's schema-rich tool discovery
- Extra indirection with no benefit

### C: Proxy at the provider level

Don't register tools; instead, modify the AI provider to inject external tool schemas into the `ChatRequest`.

**Rejected because:**
- Breaks the clean separation between tools and providers
- Would need changes in every provider implementation
- Tool results would need special handling
- The registry pattern already works perfectly for this

---

## Success Criteria

- [ ] User can add a custom HTTP MCP server via Settings → MCP and use its tools in conversation within 60 seconds
- [ ] User can add a stdio MCP server (e.g., `npx` command) and use its tools
- [ ] Known services (Notion, GitHub) work with API key entry
- [ ] Test Connection actually verifies connectivity and shows tool count
- [ ] Agent starts up with previously configured MCP tools available immediately
- [ ] Removing an integration immediately removes its tools from the agent
- [ ] Tool names are namespaced (no collisions between servers)
- [ ] All existing functionality preserved — no regressions in built-in tools

---

## Implementation Order

1. **Migration** — Add `transport_type`, `command`, `command_args`, `command_env`, `tool_count` columns
2. **MCPBridge** — Core bridge with HTTP transport support
3. **Startup sync** — Load enabled integrations on boot
4. **Real test** — Replace fake test handler
5. **API trigger** — Sync on create/update/delete/enable/disable
6. **Frontend** — Transport picker, tool count display
7. **Stdio transport** — Support for local MCP servers
8. **SSE transport** — Support for legacy servers
