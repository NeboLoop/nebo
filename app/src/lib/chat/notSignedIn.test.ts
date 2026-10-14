/**
 * A turn no AI could take (the bot is not signed in to NeboAI). The owner's
 * report (10-14): the chat said "Stopped · No AI provider is connected for
 * janus." twice, as a red run card and again as a red banner. The bot now
 * sends the typed `not_signed_in` stop; the chat shows ONE plain sentence
 * with ONE action (Sign in), once, in the thread, and no banner — live and
 * after a reload.
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
import { NOT_SIGNED_IN } from './runEnd';
import ChatPane from '$lib/components/chat/ChatPane.svelte';

const KEY = 'agent:assistant:thread:c1';
const FALLBACK = 'Nebo needs to be signed in to NeboAI to answer.';

function server(type: string, data: Record<string, unknown>) {
	for (const fn of handlers.get(type) ?? []) fn(data);
}

beforeAll(() => {
	addMessages('en', en);
	init({ fallbackLocale: 'en', initialLocale: 'en' });
});

describe('not signed in to NeboAI', () => {
	beforeEach(() => handlers.clear());

	it('live: one entry in the thread, no banner', () => {
		const chat = createChatController({ agentId: 'assistant', sessionKey: KEY });
		chat.send('Hello');
		server('chat_error', { session_id: KEY, agentId: 'assistant', error: FALLBACK, stop_reason: NOT_SIGNED_IN });
		server('chat_complete', { session_id: KEY, agentId: 'assistant', stop_reason: NOT_SIGNED_IN, stop_notice: FALLBACK });

		expect(chat.chatError).toBe('');
		const stops = chat.messages.filter((m) => m.type === 'runError');
		expect(stops).toHaveLength(1);
		expect(stops[0]).toMatchObject({ reason: NOT_SIGNED_IN });
		expect(chat.isLoading).toBe(false);
		chat.destroy();
	});

	it('an untyped error still raises the banner and keeps its card', () => {
		const chat = createChatController({ agentId: 'assistant', sessionKey: KEY });
		chat.send('Hello');
		server('chat_error', { session_id: KEY, agentId: 'assistant', error: 'Could not connect to nebo-1-flash. Try again.' });
		expect(chat.chatError).toBe('Could not connect to nebo-1-flash. Try again.');
		expect(chat.messages.filter((m) => m.type === 'runError')).toHaveLength(1);
		chat.destroy();
	});

	it('reloaded: the stored row keeps its reason and raises no banner', () => {
		const rows = [
			{ id: 'u1', role: 'user', content: 'Hello', createdAt: 1 },
			{ id: 'e1', role: 'system', content: FALLBACK, createdAt: 2, metadata: JSON.stringify({ runError: true, stopReason: NOT_SIGNED_IN }) }
		] as any;
		expect(lastRunError(rows)).toBeNull();
		const msgs = parseMessages(rows);
		expect(msgs.map((m) => m.type)).toEqual(['user', 'runError']);
		expect(msgs[1]).toMatchObject({ reason: NOT_SIGNED_IN });
	});

	it('renders one plain sentence and one Sign in action, never the error words', () => {
		const html = render(ChatPane, {
			props: {
				agentName: 'Nebo',
				messages: [
					{ type: 'user', content: 'Hello' },
					{ type: 'runError', content: FALLBACK, reason: NOT_SIGNED_IN }
				],
				chatError: ''
			}
		}).body;
		expect(html.split(FALLBACK)).toHaveLength(2);
		expect(html.match(/href="\/settings\/account"[^>]*>Sign in</g)).toHaveLength(1);
		expect(html).not.toContain('chat-run-error');
		expect(html).not.toContain('Stopped');
		expect(html.toLowerCase()).not.toContain('provider');
		expect(html.toLowerCase()).not.toContain('janus');
	});
});
