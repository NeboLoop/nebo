import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

const sent: { type: string; data: unknown }[] = [];
vi.mock('$lib/websocket/client', () => ({
	getWebSocketClient: () => ({ send: (type: string, data: unknown) => sent.push({ type, data }) })
}));
const answered: { id: string; answer: string; via: string }[] = [];
vi.mock('./permissionAsks', () => ({
	answerAsk: async (id: string, answer: string, via: string) => {
		answered.push({ id, answer, via });
	}
}));

import { waitingAsks, setWaitingAsks, answerWaiting, askPath } from './waitingAsks';
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
	createdAt: 1
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

	it('answers a permission ask with the answer its option is', async () => {
		await answerWaiting(permission, 1);
		expect(answered).toEqual([{ id: 'ask-1', answer: 'this_once', via: 'chat' }]);
		expect(sent).toEqual([]);
	});

	it('opens the conversation the ask waits in', () => {
		expect(askPath(question)).toBe('/nanna/threads/t1');
		expect(askPath({ ...question, chatId: '' })).toBe('/nanna');
	});
});
