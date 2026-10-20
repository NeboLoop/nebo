/**
 * A turn whose balance was held by work already running (Janus's
 * `BALANCE_RESERVED`, past the bot's waits; the balance covers it). The bot
 * sends the typed `balance_reserved` stop: the chat words it from that, in
 * the owner's language, once, in the thread, with no banner and never as out
 * of balance or as an error — live and after a reload.
 */
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render } from 'svelte/server';
import { addMessages, init } from 'svelte-i18n';
import en from '$lib/i18n/locales/en.json';

const handlers = new Map<string, Array<(data: any) => void>>();

vi.mock('$lib/websocket/client', () => ({
	getWebSocketClient: () => ({
		send: () => {},
		on: (type: string, fn: (data: any) => void) => {
			handlers.set(type, [...(handlers.get(type) ?? []), fn]);
			return () => {};
		},
		onStatus: () => () => {},
		getDisruptionCount: () => 0
	})
}));
vi.mock('$lib/api/gocliRequest', () => ({ sendClientEvent: () => {} }));
vi.mock('$lib/marketplace/installCodes', () => ({ sendInstallCode: () => false }));
// The live-desktop viewer's VNC client reaches for `window` on import.
vi.mock('@novnc/novnc', () => ({ default: class {} }));

import { createChatController } from './controller.svelte';
import { lastRunError, parseMessages } from './history';
import { BALANCE_RESERVED } from './runEnd';
import ChatPane from '$lib/components/chat/ChatPane.svelte';

const KEY = 'agent:assistant:thread:c1';
const FALLBACK = "Your balance is busy with work that's running. Try again in a moment.";

function server(type: string, data: Record<string, unknown>) {
	for (const fn of handlers.get(type) ?? []) fn(data);
}

beforeAll(() => {
	addMessages('en', en);
	init({ fallbackLocale: 'en', initialLocale: 'en' });
});

describe('balance held by running work', () => {
	beforeEach(() => handlers.clear());

	it('live: one entry in the thread, no banner', () => {
		const chat = createChatController({ agentId: 'assistant', sessionKey: KEY });
		chat.send('Hello');
		server('chat_error', { session_id: KEY, agentId: 'assistant', error: FALLBACK, stop_reason: BALANCE_RESERVED });
		expect(chat.chatError).toBe('');
		const stops = chat.messages.filter((m) => m.type === 'runError');
		expect(stops).toHaveLength(1);
		expect(stops[0]).toMatchObject({ reason: BALANCE_RESERVED });
		chat.destroy();
	});

	it('reloaded: the stored row keeps its reason and raises no banner', () => {
		const rows = [
			{ id: 'u1', role: 'user', content: 'Hello', createdAt: 1 },
			{ id: 'e1', role: 'system', content: FALLBACK, createdAt: 2, metadata: JSON.stringify({ runError: true, stopReason: BALANCE_RESERVED }) }
		] as any;
		expect(lastRunError(rows)).toBeNull();
		expect(parseMessages(rows)[1]).toMatchObject({ reason: BALANCE_RESERVED });
	});

	it('renders the busy sentence from the stop, never the error card or out of balance', () => {
		const html = render(ChatPane, {
			props: {
				agentName: 'Nebo',
				messages: [
					{ type: 'user', content: 'Hello' },
					// Words that are not the sentence: the stop decides what is shown.
					{ type: 'runError', content: 'whatever the bot sent', reason: BALANCE_RESERVED }
				],
				chatError: ''
			}
		}).body;
		expect(html.split('Your balance is busy with work')).toHaveLength(2);
		expect(html).not.toContain('whatever the bot sent');
		expect(html).not.toContain('chat-run-error');
		expect(html).not.toContain('Stopped');
		expect(html.toLowerCase()).not.toContain('plan');
	});
});
