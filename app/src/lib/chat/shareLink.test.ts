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
	createdAt: '2026-09-28T00:00:00Z'
};
vi.mock('$lib/api/gocliRequest', () => ({
	default: {
		get: async (url: string, params?: unknown) => (calls.push({ method: 'GET', url, params }), { share: link }),
		put: async (url: string, body?: unknown) => (calls.push({ method: 'PUT', url, body }), { share: link }),
		delete: async (url: string, params?: unknown) => (calls.push({ method: 'DELETE', url, params }), { share: null })
	}
}));

import { expiresAtFor, loadShareLink, saveShareLink, turnOffShareLink } from './shareLink';

const artifact = '/api/v1/files/Q3 plan & notes.md';

describe('share by link', () => {
	beforeEach(() => {
		calls.length = 0;
	});

	it('finds the file’s link by its Work-panel reference, encoded whole', async () => {
		expect(await loadShareLink(artifact)).toEqual(link);
		expect(calls).toEqual([
			{ method: 'GET', url: '/api/v1/neboai/share', params: { artifact: encodeURIComponent(artifact) } }
		]);
	});

	it('creates or changes the link with one PUT; a password goes only with a password link', async () => {
		await saveShareLink(artifact, 'password', 'hunter22', '');
		await saveShareLink(artifact, 'password', '', '');
		await saveShareLink(artifact, 'link', 'ignored', '2026-10-05T12:00:00Z');
		expect(calls).toEqual([
			{ method: 'PUT', url: '/api/v1/neboai/share', body: { artifact, access: 'password', expiresAt: '', password: 'hunter22' } },
			// Saving without a new password keeps the one the link has.
			{ method: 'PUT', url: '/api/v1/neboai/share', body: { artifact, access: 'password', expiresAt: '' } },
			{ method: 'PUT', url: '/api/v1/neboai/share', body: { artifact, access: 'link', expiresAt: '2026-10-05T12:00:00Z' } }
		]);
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
