# MCP Integrations

External MCP (Model Context Protocol) servers can be connected to Nebo, making their tools available to the agent.

## How It Works

1. An MCP integration is configured with a server URL, server type, and auth settings
2. The MCP Bridge connects to the server and lists available tools
3. Each tool is registered as a proxy in the agent's tool registry with a namespaced name
4. The agent can call these tools like any other tool

## Tool Namespacing

MCP tools are namespaced as:

```
mcp__{server_type}__{tool_name}
```

- Server type and tool name are lowercased
- Spaces are replaced with underscores

Example: A tool called `Search Docs` from a server typed `confluence` becomes `mcp__confluence__search_docs`.

## Workflow Availability

MCP tools are available to the agent in conversational context but are **not** available to workflow activities. Workflow activities reference skills by qualified name (`@org/skills/name`) or interface binding (`{ "interface": "name" }`). MCP tools do not follow the qualified name format and are excluded from the workflow skill-filtering system.

---

## Bridge Sync Lifecycle

1. **sync_all** — Called with the full list of enabled integrations. Disconnects stale connections, connects new ones.
2. **connect** — Connects to a single MCP server, lists tools, registers proxies. OAuth integrations without completed auth are skipped.
3. **disconnect** — Unregisters all proxy tools and closes the session.
4. **call_tool** — Forwards a tool call to the connected MCP server with JSON input and returns the result.
