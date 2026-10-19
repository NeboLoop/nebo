/**
 * Hiring the Bookkeeper connected QuickBooks through the shared login, so the
 * Bookkeeper had no account of its own, and a sign-in for one employee
 * answered every card for the plugin (2026-10-05).
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';

const authLogin = vi.fn();
const authLoginAccount = vi.fn();
vi.mock('$lib/api/nebo', () => ({
	authLogin: (...a: unknown[]) => authLogin(...a),
	authLoginAccount: (...a: unknown[]) => authLoginAccount(...a),
}));
const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...a: unknown[]) => invoke(...a) }));

import { isSignInFor, openSignInLink, openSignInWindow, signInFinished, startPluginSignIn } from './pluginSignIn';

beforeEach(() => {
	authLogin.mockClear();
	authLoginAccount.mockClear();
});

describe('startPluginSignIn', () => {
	it('signs a hire in to its own account for a plugin with one per employee', async () => {
		await startPluginSignIn('quickbooks', { multiAccount: true, agentId: 'bk' });
		expect(authLoginAccount).toHaveBeenCalledWith('quickbooks', { agentId: 'bk', accountLabel: 'Primary', accountNumber: '' });
		expect(authLogin).not.toHaveBeenCalled();
	});

	it('runs the shared login for a plugin with one sign-in', async () => {
		await startPluginSignIn('xero', { multiAccount: false, agentId: 'bk' });
		expect(authLogin).toHaveBeenCalledWith('xero');
		expect(authLoginAccount).not.toHaveBeenCalled();
	});
});

describe('isSignInFor', () => {
	it("never lets Chief's sign-in answer the Bookkeeper's card", () => {
		expect(isSignInFor({ plugin: 'quickbooks', agentId: 'chief' }, 'quickbooks', 'bk')).toBe(false);
		expect(isSignInFor({ plugin: 'quickbooks', agentId: 'bk' }, 'quickbooks', 'bk')).toBe(true);
	});

	it('lets a shared sign-in answer every card for its plugin, and no other', () => {
		expect(isSignInFor({ plugin: 'xero', agentId: null }, 'xero', 'bk')).toBe(true);
		expect(isSignInFor({ plugin: 'xero' }, 'quickbooks', 'bk')).toBe(false);
		expect(isSignInFor(undefined, 'xero', 'bk')).toBe(false);
	});
});

describe('openSignInLink', () => {
	// A cloud bot opened in the browser reaches the bot through its tunnel, so
	// the login answers with the link instead of broadcasting it. Nothing
	// opened it, and every Connect did nothing (2026-10-11).
	it('opens the link a tunnelled login hands back', async () => {
		const open = vi.fn().mockReturnValue({});
		vi.stubGlobal('window', { open });
		authLogin.mockResolvedValueOnce({ started: true, authUrl: 'https://accounts.google.com/o/oauth2/v2/auth?x=1' });
		await startPluginSignIn('gmail', { multiAccount: false });
		expect(open).toHaveBeenCalledWith('https://accounts.google.com/o/oauth2/v2/auth?x=1', 'nebo-sign-in', 'popup,width=520,height=720');
		vi.unstubAllGlobals();
	});

	it('opens nothing when the bot opens the link itself', () => {
		const open = vi.fn();
		vi.stubGlobal('window', { open });
		openSignInLink({ started: true } as { authUrl?: string });
		expect(open).not.toHaveBeenCalled();
		vi.unstubAllGlobals();
	});
});

/**
 * Every sign-in opened in a tab of the default browser: Nebo disappeared
 * behind it and the person was lost (2026-10-19). The desktop app opens it in
 * a window of its own and comes back to the front when it finishes.
 */
describe('openSignInWindow', () => {
	beforeEach(() => invoke.mockReset().mockResolvedValue(undefined));
	const flush = () => new Promise((r) => setTimeout(r, 0));

	it('asks the desktop app for a sign-in window, and brings Nebo back when it finishes', async () => {
		const open = vi.fn();
		vi.stubGlobal('window', { open, __TAURI_INTERNALS__: {} });
		expect(openSignInWindow('https://neboai.com/oauth/authorize?state=s')).toBe(true);
		await flush();
		expect(invoke).toHaveBeenCalledWith('open_sign_in', { url: 'https://neboai.com/oauth/authorize?state=s' });
		expect(open).not.toHaveBeenCalled();
		signInFinished();
		await flush();
		expect(invoke).toHaveBeenLastCalledWith('sign_in_finished');
		vi.unstubAllGlobals();
	});

	it('never pulls Nebo forward for a sign-in this page did not open', async () => {
		vi.stubGlobal('window', { open: vi.fn(), __TAURI_INTERNALS__: {} });
		signInFinished();
		await flush();
		expect(invoke).not.toHaveBeenCalled();
		vi.unstubAllGlobals();
	});

	it('says so when a browser page blocks the popup', () => {
		vi.stubGlobal('window', { open: vi.fn().mockReturnValue(null) });
		expect(openSignInWindow('https://neboai.com/oauth/authorize')).toBe(false);
		vi.unstubAllGlobals();
	});
});
