-- The tools each MCP server offered at its last sync (their own names). A
-- tool not among them is new: on a server whose default allows, it asks
-- first until the owner decides. Carried over from the list the old
-- per-server permissions kept (`tool_permissions.known`), so nothing the
-- owner has already seen becomes new at the upgrade.
ALTER TABLE mcp_integrations ADD COLUMN known_tools TEXT NOT NULL DEFAULT '[]';
UPDATE mcp_integrations
   SET known_tools = json_extract(tool_permissions, '$.known')
 WHERE json_valid(tool_permissions)
   AND json_type(tool_permissions, '$.known') = 'array';
