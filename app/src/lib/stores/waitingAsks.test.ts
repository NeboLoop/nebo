import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

const sent: { type: string; data: unknown }[] = [];
vi.mock('$lib/websocket/client', () => ({
	getWebSocketClient: () => ({ send: (type: string, data: unknown) => sent.push({ type, data }) })
}));
const answered: { id: string; value: string; via: string }[] = [];
vi.mock('./permissionAsks', () => ({ answerAsk: async () => {} }));
vi.mock('$lib/inbox/item', async (orig) => ({
	...(await orig<typeof import('$lib/inbox/item')>()),
	answerItem: async (id: string, value: string, via: string) => {
		answered.push({ id, value, via });
	}
}));

import { waitingAsks, setWaitingAsks, answerWaiting, askPath, relayedAsks } from './waitingAsks';
import type { WaitingAsk } from '$lib/api/neboComponents';

const question: WaitingAsk = {
	id: 'bf8e2c67',
	kind: 'question',
	agentId: 'nanna',
	employee: 'Nanna',
	sessionKey: 'agent:nanna:thread:t1',
	chatId: 't1',
	question: 'One daily schedule at 7 AM instead of sub-agents?',
	options: ['Yes, set it up', 'No'],
	values: ['Yes, set it up', 'No'],
	freeText: true,
	answerable: true,
	blocking: true,
	createdAt: 1,
	relayed: false
};
const permission: WaitingAsk = {
	...question,
	id: 'ask-1',
	kind: 'permission',
	options: ['Allow always', 'This once', 'No'],
	values: ['allow_always', 'this_once', 'no'],
	freeText: false
};

describe('waitingAsks', () => {
	beforeEach(() => {
		sent.length = 0;
		answered.length = 0;
	});

	it('takes the whole list from asks_waiting', () => {
		setWaitingAsks({ asks: [question, permission] });
		expect(get(waitingAsks).map((a) => a.id)).toEqual(['bf8e2c67', 'ask-1']);
		setWaitingAsks({ asks: [] });
		expect(get(waitingAsks)).toEqual([]);
		setWaitingAsks(null);
		expect(get(waitingAsks)).toEqual([]);
	});

	it("answers a question on the socket with the option's value", async () => {
		await answerWaiting(question, 0);
		expect(sent).toEqual([{ type: 'ask_response', data: { request_id: 'bf8e2c67', value: 'Yes, set it up' } }]);
		expect(answered).toEqual([]);
	});

	it('answers a permission ask through its Inbox item with the answer its option is', async () => {
		await answerWaiting(permission, 1);
		expect(answered).toEqual([{ id: 'permission-ask:ask-1', value: 'this_once', via: 'chat' }]);
		expect(sent).toEqual([]);
	});

	it('answers a relayed question through its Inbox item', async () => {
		await answerWaiting({ ...question, relayed: true }, 1);
		expect(answered).toEqual([{ id: 'question-ask:bf8e2c67', value: 'No', via: 'chat' }]);
		expect(sent).toEqual([]);
	});

	it('opens the conversation the ask waits in', () => {
		expect(askPath(question)).toBe('/nanna/threads/t1');
		expect(askPath({ ...question, chatId: '' })).toBe('/nanna');
	});
});

describe('relayed asks', () => {
	it("only what work the owner is not in raised comes to the assistant's chat; a card from a chat he asked in stays there", () => {
		const relayed = { ...permission, id: 'ask-relayed', chatId: '', relayed: true };
		expect(relayedAsks([question, permission, relayed]).map((a) => a.id)).toEqual(['ask-relayed']);
	});
});
