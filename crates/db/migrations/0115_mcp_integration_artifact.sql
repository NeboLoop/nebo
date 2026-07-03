-- Link marketplace-installed MCP integrations back to their connector artifact
-- (CONN- code) so the update system can find and reconcile the rows when the
-- connector publishes a new version. NULL for manually added servers.
ALTER TABLE mcp_integrations ADD COLUMN artifact_id TEXT;
