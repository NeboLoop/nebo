-- Per-server MCP tool permissions (Settings → MCP → Tool permissions):
-- tools::policy::McpServerPermissions JSON — a server-wide default
-- (allow/ask/deny) plus per-tool overrides and the tool list from the last
-- sync. NULL means never synced/edited (everything defaults to ask).
ALTER TABLE mcp_integrations ADD COLUMN tool_permissions TEXT;
