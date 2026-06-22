-- Track per-account OAuth health for multi-account plugins.
--
-- The proactive token refresher (spawn_plugin_token_refresher) periodically runs
-- each connected account's manifest-declared auth `status` command. When an
-- account's token can no longer be refreshed (e.g. Google Workspace RAPT reauth,
-- revoked refresh token), it sets needs_reauth = 1 so the Connected Accounts UI
-- can show a "Reconnect" badge, and fires ONE notification (reauth_notified = 1)
-- so the user isn't nagged every tick. Both clear automatically once the account
-- is healthy again.
ALTER TABLE plugin_account_profiles ADD COLUMN needs_reauth INTEGER NOT NULL DEFAULT 0;
ALTER TABLE plugin_account_profiles ADD COLUMN reauth_notified INTEGER NOT NULL DEFAULT 0;
