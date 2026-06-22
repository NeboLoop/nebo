// Multi-account plugin endpoints.
//
// These let one agent connect several accounts (e.g. multiple Gmail
// inboxes) to a single multi-account plugin. The generated `nebo.ts`
// client does not yet cover them, so they live here against the same
// `webapi` request layer (auth header + base URL handling included).

import { webapi } from './gocliRequest';

export interface PluginAccount {
	accountLabel: string;
	isPrimary: boolean;
	/** The account's OAuth token expired and can't be refreshed — the user must
	 * reconnect it. Surfaced as a "Reconnect" badge in Connected Accounts. */
	needsReauth?: boolean;
}

export interface ListPluginAccountsResponse {
	accounts: PluginAccount[];
}

/** GET /plugins/{slug}/accounts?agentId=<id> — accounts an agent has connected. */
export function listPluginAccounts(slug: string, agentId: string) {
	return webapi.get<ListPluginAccountsResponse>(
		`/api/v1/plugins/${slug}/accounts`,
		{ agentId }
	);
}

/**
 * POST /plugins/{slug}/accounts/login — start an OAuth login for a labelled
 * account. Returns immediately; completion arrives over the WebSocket
 * (`plugin_auth_complete` / `plugin_auth_error`, each carrying `plugin` and
 * `account`).
 */
export function startPluginAccountLogin(slug: string, agentId: string, accountLabel: string) {
	return webapi.post<{ started: boolean }>(
		`/api/v1/plugins/${slug}/accounts/login`,
		{ agentId, accountLabel }
	);
}

/**
 * DELETE /plugins/{slug}/accounts — disconnect one account from an agent
 * (removes the profile mapping + its credential directory). Query params are
 * inlined so they reach the backend's `Query` extractor regardless of client
 * serialization.
 */
export function disconnectPluginAccount(slug: string, agentId: string, accountLabel: string) {
	const qs = `agentId=${encodeURIComponent(agentId)}&accountLabel=${encodeURIComponent(accountLabel)}`;
	return webapi.delete<{ ok: boolean }>(`/api/v1/plugins/${slug}/accounts?${qs}`);
}
