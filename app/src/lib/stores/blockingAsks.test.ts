import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

const sent: { type: string; data: unknown }[] = [];
vi.mock('$lib/websocket/client', () => ({
	getWebSocketClient: () => ({ send: (type: string, data: unknown) => sent.push({ type, data }) })
}));
const navigated: string[] = [];
vi.mock('$lib/nav', () => ({ goto: async (to: string) => navigated.push(to) }));
const answered: { id: string; answer: string; via: string }[] = [];
vi.mock('./permissionAsks', () => ({
	answerAsk: async (id: string, answer: string, via: string) => {
		answered.push({ id, answer, via });
	}
}));

import { waitingAsks } from './waitingAsks';
import {
	openAskCard,
	openAsk,
	closeAsk,
	newBlockingAsks,
	seedBlockingAsks,
	askHome,
	answerById,
	inAsksChat
} from './blockingAsks';
import type { WaitingAsk } from '$lib/api/neboComponents';

// Live 2026-10-02: Bookkeeper's workflow step parked on the owner's OK while
// he was in Flip-Flap's chat.
const bookkeeper: WaitingAsk = {
	id: 'ask-bk',
	kind: 'permission',
	agentId: 'bookkeeper',
	employee: 'Bookkeeper',
	sessionKey: 'agent:bookkeeper:workflow:w1:triage::1',
	chatId: '',
	question: 'OK to go ahead with Draft: Debit Ask Client $17,519.79? This needs your OK every time.',
	options: ['Allow always', 'This once', 'No'],
	values: ['allow_always', 'this_once', 'no'],
	freeText: false,
	answerable: true,
	blocking: true,
	createdAt: 1
};
const quiet: WaitingAsk = { ...bookkeeper, id: 'ask-quiet', blocking: false };

describe('blocking asks', () => {
	beforeEach(() => {
		sent.length = 0;
		answered.length = 0;
		navigated.length = 0;
		closeAsk();
		waitingAsks.set([bookkeeper, quiet]);
	});

	it('tells of each new blocking ask once, and never of one nothing waits on', () => {
		const fresh = { ...bookkeeper, id: 'ask-new' };
		expect(newBlockingAsks([quiet, fresh]).map((a) => a.id)).toEqual(['ask-new']);
		expect(newBlockingAsks([quiet, fresh])).toEqual([]);
	});

	it('what already waits when the app starts raises no burst of notices', () => {
		const backlog = { ...bookkeeper, id: 'ask-backlog' };
		seedBlockingAsks([backlog]);
		expect(newBlockingAsks([backlog])).toEqual([]);
	});

	it('a click opens the card over the current screen, with no navigation', () => {
		openAsk('ask-bk');
		expect(get(openAskCard)).toBe('ask-bk');
		expect(navigated).toEqual([]);
		closeAsk();
		expect(get(openAskCard)).toBeNull();
	});

	it('answering from a notification while in another chat settles it by id and moves nothing', async () => {
		await answerById('ask-bk', 2);
		expect(answered).toEqual([{ id: 'ask-bk', answer: 'no', via: 'chat' }]);
		expect(navigated).toEqual([]);
		expect(get(openAskCard)).toBeNull();
	});

	it('an ask answered elsewhere first is not answered again', async () => {
		waitingAsks.set([]);
		await answerById('ask-bk', 0);
		expect(answered).toEqual([]);
	});

	it('is in its chat only on that chat, never on another or with no chat', () => {
		const inChat = { ...bookkeeper, chatId: 'c-9' };
		expect(inAsksChat(inChat, '/bookkeeper/threads/c-9')).toBe(true);
		expect(inAsksChat(inChat, '/bookkeeper/threads/c-9/files')).toBe(true);
		expect(inAsksChat(inChat, '/bookkeeper/threads/c-90')).toBe(false);
		expect(inAsksChat(inChat, '/flip-flap/threads/c-1')).toBe(false);
		expect(inAsksChat(bookkeeper, '/bookkeeper')).toBe(false);
	});

	it('See conversation goes to the chat it waits in, or its Inbox item when no chat raised it', () => {
		expect(askHome(bookkeeper)).toBe('/inbox?m=permission-ask%3Aask-bk');
		expect(askHome({ ...bookkeeper, chatId: 'c-9' })).toBe('/bookkeeper/threads/c-9');
	});
});
