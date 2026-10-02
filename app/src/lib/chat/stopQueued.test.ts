/**
 * Stop with a message queued behind the running work: the stop ends that
 * work, and the bot answers the queued message at once in a turn of its own
 * (it carries the message past the stop). The conversation shows that turn
 * working from its first moment, its reply lands under the message, and the
 * message stops reading as queued when it is answered. No second click.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';

const sent: Array<[string, Record<string, unknown>]> = [];
const handlers = new Map<string, Array<(data: any) => void>>();

vi.mock('$lib/websocket/client', () => ({
	getWebSocketClient: () => ({
		send: (type: string, data: Record<string, unknown>) => sent.push([type, data]),
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

import { createChatController } from './controller.svelte';

const KEY = 'agent:a:thread:c1';

function server(type: string, data: Record<string, unknown>) {
	for (const fn of handlers.get(type) ?? []) fn(data);
}

function userMessages(chat: ReturnType<typeof createChatController>) {
	return chat.messages.filter((m) => m.type === 'user') as Array<{ content: string; pending?: boolean }>;
}

describe('Stop with a queued message', () => {
	beforeEach(() => {
		sent.length = 0;
		handlers.clear();
	});

	it('runs the queued message at once: working again, its answer lands, it is no longer queued', () => {
		const chat = createChatController({ agentId: 'a', sessionKey: KEY });
		chat.send('Fix the renderer.');
		chat.send('Did you look at the screenshot?');
		// The bot queued the second behind the running work.
		server('chat_error', { session_id: KEY, stop_reason: 'queued_into_running_turn' });
		expect(userMessages(chat).at(-1)?.pending).toBe(true);

		chat.stop();
		expect(sent.filter(([t]) => t === 'cancel')).toHaveLength(1);
		server('chat_cancelled', { session_id: KEY });
		// The stop never sends the queued message again: the bot holds it.
		expect(sent.filter(([t]) => t === 'chat')).toHaveLength(2);

		// The bot's turn for it starts at once: the conversation is working
		// from that moment, before any token.
		server('chat_created', { session_id: KEY, agentId: 'a' });
		expect(chat.isLoading).toBe(true);

		server('tool_start', { session_id: KEY, agentId: 'a', tool_id: 't1', tool: 'read_file', label: 'Reading the screenshot' });
		server('chat_stream', { session_id: KEY, agentId: 'a', content: 'Looked at it: the scene renders black.' });
		server('chat_complete', { session_id: KEY, agentId: 'a' });

		expect(chat.isLoading).toBe(false);
		expect(userMessages(chat).every((m) => !m.pending)).toBe(true);
		const asked = chat.messages.findLastIndex((m) => m.type === 'user');
		const answer = chat.messages.slice(asked + 1) as Array<{ type: string; content?: string; tools?: unknown[] }>;
		expect(answer.every((m) => m.type === 'assistant')).toBe(true);
		expect(answer.some((m) => m.content?.includes('Looked at it'))).toBe(true);
		expect(answer.flatMap((m) => m.tools ?? [])).toHaveLength(1);
		chat.destroy();
	});

	it('a stop with nothing queued stays stopped', () => {
		const chat = createChatController({ agentId: 'a', sessionKey: KEY });
		chat.send('Fix the renderer.');
		chat.stop();
		server('chat_cancelled', { session_id: KEY });
		expect(chat.isLoading).toBe(false);
		chat.destroy();
	});

	it('a turn starting in another conversation does not mark this one working', () => {
		const chat = createChatController({ agentId: 'a', sessionKey: KEY });
		server('chat_created', { session_id: 'agent:a:thread:other', agentId: 'a' });
		expect(chat.isLoading).toBe(false);
		chat.destroy();
	});
});
