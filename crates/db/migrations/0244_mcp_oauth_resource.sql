-- MCP sign-in, as the MCP SDK does it:
-- `oauth_resource`: the RFC 8707 resource indicator discovery found (from the
--   server's RFC 9728 protected-resource metadata), sent on authorize, code
--   exchange and every refresh. NULL when the server publishes none.
-- `oauth_client_redirect_uri`: the redirect_uri the stored client was
--   registered with (Dynamic Client Registration), so a later sign-in reuses
--   that client instead of registering a new one each time. NULL for clients
--   registered before this column, which register once more.
-- +goose Up
ALTER TABLE mcp_integrations ADD COLUMN oauth_resource TEXT;
ALTER TABLE mcp_integrations ADD COLUMN oauth_client_redirect_uri TEXT;

-- +goose Down
ALTER TABLE mcp_integrations DROP COLUMN oauth_client_redirect_uri;
ALTER TABLE mcp_integrations DROP COLUMN oauth_resource;
