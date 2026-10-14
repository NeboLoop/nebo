// The Inbox store holds the server's rows as they are: status and answers
// come from the server, and an answer from anywhere moves a decision to
// History everywhere (`notification_settled`). The badge is the server's
// counts.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

const counts = { count: 3, needsYou: 1, unread: 2 };
vi.mock('$lib/api/nebo', () => ({
	unreadCount: vi.fn(async () => counts),
	deleteNotification: vi.fn(async () => ({ success: true })),
	markRead: vi.fn(async () => ({})),
	markAllRead: vi.fn(async () => ({}))
}));

import {
	notifications,
	history,
	inboxCounts,
	unreadCount,
	pushNotification,
	settleNotification,
	upsertItem,
	removeNotification,
	type InboxItem
} from './notifications';

const item = (id: string, over: Partial<InboxItem> = {}): InboxItem => ({
	id,
	userId: 'u',
	type: 'info',
	title: id,
	body: '',
	createdAt: 100,
	needsYou: false,
	options: [],
	status: 'update',
	...over
});

const flush = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
	notifications.set([]);
	history.set([]);
	inboxCounts.set({ needsYou: 0, unread: 0 });
});

describe('the Inbox store', () => {
	it('upserts a pushed item by id and keeps the newest first', () => {
		pushNotification(item('a', { createdAt: 1 }));
		pushNotification(item('b', { createdAt: 2 }));
		pushNotification(item('a', { createdAt: 1, title: 'restated' }));
		expect(get(notifications).map((n) => [n.id, n.title])).toEqual([
			['b', 'b'],
			['a', 'restated']
		]);
	});

	it('reads an older payload: message and link, and a needs-you item is open', () => {
		const n = pushNotification({ id: 'q:1', title: 'Which supplier?', message: 'Stock runs out', link: '/store', needsYou: true });
		expect([n.body, n.actionUrl, n.status, n.options]).toEqual(['Stock runs out', '/store', 'open', []]);
	});

	it('settles a decision answered anywhere: History, with the answer and the surface', () => {
		notifications.set([item('q:1', { needsYou: true, status: 'open' })]);
		settleNotification({ id: 'q:1', answer: 'Lumen · $14.20', answeredVia: 'mobile', status: 'answered' });
		const n = get(notifications)[0];
		expect([n.status, n.answer, n.answeredVia]).toEqual(['answered', 'Lumen · $14.20', 'mobile']);
		expect(n.answeredAt).toBeTruthy();
		expect(n.readAt).toBeTruthy();
		expect(get(history).map((h) => h.id)).toEqual(['q:1']);
	});

	it('takes the server copy of an answered item', () => {
		notifications.set([item('q:1', { needsYou: true, status: 'open' })]);
		upsertItem(item('q:1', { needsYou: true, status: 'answered', answer: 'Yes' }));
		expect(get(notifications)[0].status).toBe('answered');
		expect(get(history)[0].answer).toBe('Yes');
	});

	it('puts an item away into History; nothing is deleted', () => {
		notifications.set([item('u:1')]);
		removeNotification('u:1');
		expect(get(notifications)).toEqual([]);
		expect(get(history).map((h) => [h.id, h.status])).toEqual([['u:1', 'archived']]);
	});

	it("badges the server's counts: decisions still open and updates not read", async () => {
		pushNotification(item('x'));
		await flush();
		expect(get(inboxCounts)).toEqual({ needsYou: 1, unread: 2 });
		expect(get(unreadCount)).toBe(3);
	});
});
