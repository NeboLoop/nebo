// The assistant's chat is the owner's front door: what waits on him from work
// he is not in is relayed there, each item its employee's own card, with the
// way back to that employee. A connect card relayed there connects the
// employee who asked, never the assistant (live 2026-10-05: a QuickBooks
// card connected Chief instead of the Bookkeeper). A card from a chat the
// owner asked in stays in that chat.
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render } from 'svelte/server';
import { addMessages, init } from 'svelte-i18n';
import en from '$lib/i18n/locales/en.json';

vi.mock('$lib/api/nebo', () => ({
	authLoginAccount: vi.fn(),
	listPlugins: vi.fn(async () => ({ plugins: [] })),
	submitCode: vi.fn()
}));
vi.mock('$lib/websocket/client', () => ({
	getWebSocketClient: () => ({ on: () => () => {}, send: () => {} })
}));
vi.mock('$lib/nav', () => ({ goto: vi.fn() }));

import type { WaitingAsk } from '$lib/api/neboComponents';
import { waitingAsks } from '$lib/stores/waitingAsks';
import { approvals } from '$lib/stores/approvals';
import RelayedAsks, { relayedCard } from './RelayedAsks.svelte';
import { connectTarget } from './AskWidget.svelte';

beforeAll(() => {
	addMessages('en', en);
	init({ fallbackLocale: 'en', initialLocale: 'en' });
});

const text = (html: string) => html.replace(/<!--[^>]*-->/g, '').replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').trim();

// The owner asked the assistant to have the store reconciled; the Store
// Manager's turn parked on Shopify's connect card.
const connect: WaitingAsk = {
	id: 'req-1',
	kind: 'question',
	agentId: 'store',
	employee: 'Ecommerce Store Manager',
	sessionKey: 'agent:store:coworker:assistant',
	chatId: 'c-store',
	question: "I need Ecommerce Store Manager's Shopify connected to continue.",
	options: [],
	values: [],
	freeText: false,
	answerable: false,
	blocking: true,
	createdAt: 1,
	relayed: true,
	widgets: [{ type: 'connect_account', plugin: 'shopify', agentId: 'store', label: 'Shopify', employee: 'Ecommerce Store Manager' }]
};
// The owner asked the Bookkeeper in its own chat: the card stays there.
const direct: WaitingAsk = {
	...connect,
	id: 'req-2',
	agentId: 'bk',
	employee: 'Bookkeeper',
	sessionKey: 'agent:bk:thread:c-bk',
	chatId: 'c-bk',
	relayed: false,
	widgets: [{ type: 'connect_account', plugin: 'books', agentId: 'bk', label: 'Books', employee: 'Bookkeeper' }]
};

describe("the assistant's chat relays, it never answers", () => {
	beforeEach(() => {
		waitingAsks.set([connect, direct]);
		approvals.set([]);
	});

	it('a relayed connect card connects the employee who asked, never the assistant', () => {
		expect(connectTarget(relayedCard(connect)[0])).toEqual({ plugin: 'shopify', agentId: 'store' });
		expect(connectTarget(relayedCard(connect)[0])?.agentId).not.toBe('assistant');
	});

	it('names who asked, shows their card, and links to their thread', () => {
		const shown = text(render(RelayedAsks).body);
		expect(shown).toContain('Ecommerce Store Manager wants to connect Shopify');
		expect(shown).toContain('Shopify for Ecommerce Store Manager');
		expect(shown).toContain('View in Ecommerce Store Manager');
	});

	it('a card raised in a direct owner-employee chat is not relayed', () => {
		const shown = text(render(RelayedAsks).body);
		expect(shown).not.toContain('Bookkeeper');
		waitingAsks.set([direct]);
		expect(render(RelayedAsks).body).not.toContain('relayed-ask');
	});
});
