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
import { get } from 'svelte/store';
import { t } from 'svelte-i18n';
import { authLogin, authLoginAccount } from '$lib/api/nebo';
import { addToast } from '$lib/stores/toast';
import { inDesktopApp } from '$lib/api/engineService';

/** Whether a sign-in event answers a sign-in of `plugin` for `agentId`. */
export function isSignInFor(data: Record<string, unknown> | undefined, plugin: string | null | undefined, agentId: string | undefined): boolean {
	if (!data || !plugin || data.plugin !== plugin) return false;
	return !data.agentId || data.agentId === agentId;
}

/** Start `slug`'s sign-in for the employee `agentId`: its own account for a
 *  plugin with one per employee, the shared login otherwise. */
export async function startPluginSignIn(slug: string, opts: { multiAccount?: boolean; agentId?: string }) {
	const resp =
		opts.multiAccount && opts.agentId
			? await authLoginAccount(slug, { agentId: opts.agentId, accountLabel: 'Primary', accountNumber: '' })
			: await authLogin(slug);
	openSignInLink(resp);
	return resp;
}

/** A page that reached the bot through its tunnel (a cloud bot in the
 *  browser, the phone) gets the sign-in link back instead of a
 *  `plugin_auth_url` broadcast: open it here, or nothing opens at all. */
export function openSignInLink(resp: { authUrl?: string } | undefined) {
	if (!resp?.authUrl || typeof window === 'undefined') return;
	if (!openSignInWindow(resp.authUrl)) {
		addToast(get(t)('agentSettings.signInPopupBlocked'), 'error', 8000);
	}
}

/** The size of a browser page's sign-in popup (the desktop app's window is
 *  sized by the app: `browser::default_browser`). */
const POPUP_FEATURES = 'popup,width=520,height=720';

/** A sign-in this page opened that has not finished yet. */
let signInOpen = false;

/**
 * The ONE way a sign-in opens: NeboAI's, a plugin's, a connector's.
 *
 * The desktop app opens it in a window of its own in the person's default
 * browser (`open_sign_in`): their real browser and profile, which Google
 * requires, and closing the window leaves them with Nebo rather than a tab
 * among theirs. A browser page opens it as a popup window. `false` when the
 * browser blocked the popup: the caller shows the link instead.
 */
export function openSignInWindow(url: string): boolean {
	signInOpen = true;
	if (inDesktopApp()) {
		void import('@tauri-apps/api/core')
			.then(({ invoke }) => invoke('open_sign_in', { url }))
			// An app shell from before `open_sign_in` hands the link to the
			// browser the way it opens every other link.
			.catch(() => window.open(url, '_blank'));
		return true;
	}
	if (window.open(url, 'nebo-sign-in', POPUP_FEATURES)) return true;
	signInOpen = false;
	return false;
}

/** A sign-in finished, signed in or not. If this page opened one, the
 *  desktop app comes back to the front over the browser window it left the
 *  person in (a browser page's popup closes itself). */
export function signInFinished() {
	if (!signInOpen) return;
	signInOpen = false;
	if (!inDesktopApp()) return;
	void import('@tauri-apps/api/core')
		.then(({ invoke }) => invoke('sign_in_finished'))
		.catch(() => {});
}
