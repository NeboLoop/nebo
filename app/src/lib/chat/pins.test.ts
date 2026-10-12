import { describe, expect, it } from 'vitest';
import { pinnedFirst, withPin } from './pins';

// Newest first, as GET /agents/{id}/chats lists them.
const listed = [
	{ id: 'today', pinned: false, pinnedAt: null },
	{ id: 'yesterday', pinned: true, pinnedAt: 100 },
	{ id: 'last-week', pinned: false, pinnedAt: null },
	{ id: 'last-year', pinned: true, pinnedAt: 200 }
];
const ids = (rows: { id: string }[]) => rows.map((r) => r.id);

describe('pinned conversations', () => {
	it('come first, most recently pinned first, then the rest newest first', () => {
		expect(ids(pinnedFirst(listed))).toEqual(['last-year', 'yesterday', 'today', 'last-week']);
	});

	it('leave the list itself in the bot order, so its first row is still the latest', () => {
		pinnedFirst(listed);
		expect(listed[0].id).toBe('today');
	});

	it('move to the top when pinned and back to their place by time when unpinned', () => {
		const pinned = withPin(listed, 'last-week', true, 300);
		expect(ids(pinnedFirst(pinned))).toEqual(['last-week', 'last-year', 'yesterday', 'today']);
		const unpinned = withPin(pinned, 'last-year', false, 200);
		expect(unpinned.find((c) => c.id === 'last-year')).toEqual({ id: 'last-year', pinned: false, pinnedAt: null });
		expect(ids(pinnedFirst(unpinned))).toEqual(['last-week', 'yesterday', 'today', 'last-year']);
	});
});
