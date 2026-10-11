import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('./notifications', () => ({
	askBandStatus: (s: string) => s,
	askNotificationId: (id: string) => `permission-ask:${id}`,
	setApprovalStatus: () => {}
}));

import { openAsks, settledAsks, askRaised, askSettled, chatAsksOf, placeAsks } from './permissionAsks';
import type { PermissionAskCard } from '$lib/api/neboComponents';

function ask(id: string, chatId: string, status = 'open', createdAt = 1): PermissionAskCard {
	return {
		id,
		kind: 'permission',
		agentId: 'ar',
		employee: 'Accounts Receivable Specialist',
		sessionKey: chatId ? `agent:ar:thread:${chatId}` : 'agent:ar:coworker:gm',
		sentence: 'sending an email to a new customer',
		reason: "It's the first time it would contact them.",
		allowAlways: true,
		thisOnce: true,
		status,
		chatId,
		blocking: false,
		createdAt
	};
}

describe('a chat shows only the asks its own flow raised', () => {
	beforeEach(() => {
		openAsks.set([]);
		settledAsks.set([]);
	});

	it("an ask from a scheduled run is in no chat; this chat's ask is in this chat only", () => {
		askRaised(ask('sched', ''));
		askRaised(ask('here', 't1'));
		askRaised(ask('there', 't2'));
		const open = get(openAsks);
		expect(chatAsksOf(open, [], 't1').map((a) => a.id)).toEqual(['here']);
		expect(chatAsksOf(open, [], 't2').map((a) => a.id)).toEqual(['there']);
		expect(chatAsksOf(open, [], '').map((a) => a.id)).toEqual([]);
		for (const chat of ['t1', 't2', 't3']) {
			expect(chatAsksOf(open, [], chat).some((a) => a.id === 'sched')).toBe(false);
		}
	});

	it('an answered ask keeps its place as a receipt, and the open ones stay last', () => {
		askRaised(ask('first', 't1', 'open', 1));
		askRaised(ask('second', 't1', 'open', 2));
		askSettled({ ...ask('first', 't1', 'declined', 1), answer: 'no' });
		const shown = chatAsksOf(get(openAsks), get(settledAsks), 't1');
		expect(shown.map((a) => [a.id, a.status])).toEqual([
			['first', 'declined'],
			['second', 'open']
		]);
	});
});

describe('an answered ask stays in line with the chat', () => {
	it('sits after the message that was last when it showed; open or unplaced ones sit at the bottom', () => {
		const asks = [ask('done', 't1', 'allowed', 1), ask('open', 't1', 'open', 2), ask('lost', 't1', 'declined', 3)];
		const { after, bottom } = placeAsks(asks, { done: 'm2', open: 'm3', lost: 'gone' }, ['m1', 'm2', 'm3', 'm4']);
		expect(after.get('m2')?.map((a) => a.id)).toEqual(['done']);
		expect(after.has('m3')).toBe(false);
		expect(bottom.map((a) => a.id)).toEqual(['open', 'lost']);
	});
});
