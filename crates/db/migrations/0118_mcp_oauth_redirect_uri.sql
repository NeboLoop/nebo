-- The redirect_uri the OAuth flow STARTED with. RFC 6749 requires the token
-- exchange to present the exact authorize-time value; recomputing it from the
-- callback request's Host broke whenever a proxy hop (tunnel peer replica)
-- rewrote the Host header — the token endpoint answered "redirect_uri
-- mismatch". Stored with the flow state, cleared with it.
ALTER TABLE mcp_integrations ADD COLUMN oauth_redirect_uri TEXT;
