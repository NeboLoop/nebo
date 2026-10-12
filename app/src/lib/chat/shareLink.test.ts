import { describe, it, expect, vi, beforeEach } from 'vitest';

// The transport under the generated client: every call is recorded, and the
// bot answers with the file's link.
const calls: { method: string; url: string; params?: unknown; body?: unknown }[] = [];
const link = {
	id: 's1',
	url: 'https://neboai.com/s/tok',
	filename: 'Go-Live-Checklist.md',
	access: 'password',
	hasPassword: true,
	expiresAt: '',
	createdAt: '2026-09-28T00:00:00Z',
	source: '/api/v1/files/Go-Live-Checklist.md',
	live: false,
	contentHash: 'h1'
};
vi.mock('$lib/api/gocliRequest', () => ({
	default: {
		get: async (url: string, params?: unknown) => (calls.push({ method: 'GET', url, params }), { share: link, outdated: true }),
		put: async (url: string, body?: unknown) => (calls.push({ method: 'PUT', url, body }), { share: link, outdated: false }),
		delete: async (url: string, params?: unknown) => (calls.push({ method: 'DELETE', url, params }), { outdated: false })
	}
}));

import { expiresAtFor, loadShareLink, saveShareLink, siteAddressFor, turnOffShareLink } from './shareLink';

const artifact = '/api/v1/files/Q3 plan & notes.md';

describe('share by link', () => {
	beforeEach(() => {
		calls.length = 0;
	});

	it('finds the file’s link by its Work-panel reference, encoded whole', async () => {
		expect(await loadShareLink(artifact)).toEqual({ share: link, outdated: true });
		expect(calls).toEqual([
			{ method: 'GET', url: '/api/v1/neboai/share', params: { artifact: encodeURIComponent(artifact) } }
		]);
	});

	it('creates or changes the link with one PUT; a password goes only with a password link', async () => {
		await saveShareLink(artifact, 'password', 'hunter22', '', true);
		await saveShareLink(artifact, 'password', '', '', true);
		await saveShareLink(artifact, 'link', 'ignored', '2026-10-05T12:00:00Z', false);
		expect(calls).toEqual([
			{ method: 'PUT', url: '/api/v1/neboai/share', body: { artifact, access: 'password', expiresAt: '', live: true, password: 'hunter22' } },
			// Saving without a new password keeps the one the link has.
			{ method: 'PUT', url: '/api/v1/neboai/share', body: { artifact, access: 'password', expiresAt: '', live: true } },
			{ method: 'PUT', url: '/api/v1/neboai/share', body: { artifact, access: 'link', expiresAt: '2026-10-05T12:00:00Z', live: false } }
		]);
	});

	it('puts the file as it is now behind a link that keeps its version', async () => {
		await saveShareLink(artifact, 'link', '', '', false, true);
		expect(calls).toEqual([
			{ method: 'PUT', url: '/api/v1/neboai/share', body: { artifact, access: 'link', expiresAt: '', live: false, newVersion: true } }
		]);
	});

	it('publishes as a site at an address, or stops with an empty one', async () => {
		await saveShareLink(artifact, 'link', '', '', true, false, 'grandview');
		await saveShareLink(artifact, 'link', '', '', true, false, '');
		expect(calls.map((c) => (c.body as Record<string, unknown>).address)).toEqual(['grandview', '']);
	});

	it('suggests a site address from the file’s title', () => {
		expect(siteAddressFor('Grandview Neighborhood.html')).toBe('grandview-neighborhood');
		expect(siteAddressFor('Café & Bar — Menu 2026.html')).toBe('cafe-bar-menu-2026');
	});

	it('turns the link off with a DELETE', async () => {
		await turnOffShareLink(artifact);
		expect(calls).toEqual([
			{ method: 'DELETE', url: '/api/v1/neboai/share', params: { artifact: encodeURIComponent(artifact) } }
		]);
	});

	it('an expiry is never, the link’s own, or days from now', () => {
		const now = new Date('2026-09-28T10:00:00.123Z');
		expect(expiresAtFor('never', '2026-10-01T00:00:00Z', now)).toBe('');
		expect(expiresAtFor('keep', '2026-10-01T00:00:00Z', now)).toBe('2026-10-01T00:00:00Z');
		expect(expiresAtFor('1', '', now)).toBe('2026-09-29T10:00:00Z');
		expect(expiresAtFor('30', '', now)).toBe('2026-10-28T10:00:00Z');
	});
});
