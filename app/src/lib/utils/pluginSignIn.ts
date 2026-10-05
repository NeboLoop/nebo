/**
 * A plugin sign-in on behalf of one employee: which login starts it, and
 * which `plugin_auth_complete` / `plugin_auth_error` events answer it.
 *
 * A plugin that keeps an account per employee (`multiAccount`, its manifest's
 * `auth.profileDirEnv`) signs in to THAT employee's own account, and the bot's
 * events name the employee (`agentId`) — so Chief's sign-in never ticks the
 * Bookkeeper's card (2026-10-05). A shared sign-in's events name no one and
 * answer every card for the plugin.
 */
import { authLogin, authLoginAccount } from '$lib/api/nebo';

/** Whether a sign-in event answers a sign-in of `plugin` for `agentId`. */
export function isSignInFor(data: Record<string, unknown> | undefined, plugin: string | null | undefined, agentId: string | undefined): boolean {
	if (!data || !plugin || data.plugin !== plugin) return false;
	return !data.agentId || data.agentId === agentId;
}

/** Start `slug`'s sign-in for the employee `agentId`: its own account for a
 *  plugin with one per employee, the shared login otherwise. */
export function startPluginSignIn(slug: string, opts: { multiAccount?: boolean; agentId?: string }) {
	if (opts.multiAccount && opts.agentId) {
		return authLoginAccount(slug, { agentId: opts.agentId, accountLabel: 'Primary', accountNumber: '' });
	}
	return authLogin(slug);
}
