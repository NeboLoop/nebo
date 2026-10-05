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

import { isSignInFor, startPluginSignIn } from './pluginSignIn';

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
