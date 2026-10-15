/**
 * A page that joins a turn part way through its reply reads the thread again
 * when the turn ends. Live 2026-10-15 (the mobile app, the same history
 * door): "hey" in a new chat, and the reply read ". What are we getting into
 * today?". The bot kept "Hey. What are we getting into today?": "Hey" had
 * streamed before the thread was opened, the history held no reply yet, and
 * the rest of the stream became the whole reply on screen.
 */
import { describe, it, expect, vi } from 'vitest';

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
const hey = { id: 'u1', chatId: 'c1', role: 'user', content: 'hey', createdAt: 1 };
const api = vi.hoisted(() => ({
	getChatMessages: vi.fn(),
	getSessionGoal: vi.fn(async () => ({ goal: null }))
}));
vi.mock('$lib/api/nebo', () => api);

import { createChatController, type ChatMessage } from './controller.svelte';

const KEY = 'agent:assistant:thread:c1';

function server(type: string, data: Record<string, unknown>) {
	for (const fn of handlers.get(type) ?? []) fn({ session_id: KEY, agentId: 'assistant', ...data });
}

const replies = (messages: ChatMessage[]) =>
	messages.flatMap((m) => (m.type === 'assistant' && m.content ? [m.content] : []));

describe('a turn joined part way through its reply', () => {
	it('shows the reply as the bot kept it, not from where the page joined', async () => {
		api.getChatMessages
			.mockResolvedValueOnce({ messages: [hey], activeRun: { elapsedSecs: 1, toolCalls: 0, currentTool: '' } })
			.mockResolvedValueOnce({
				messages: [hey, { id: 'r1', chatId: 'c1', role: 'assistant', content: 'Hey. What are we getting into today?', createdAt: 2 }]
			});
		const chat = createChatController({ agentId: 'assistant', sessionKey: KEY });
		await chat.loadHistory('c1');
		server('chat_stream', { content: '. What are we getting into today?' });
		server('chat_complete', {});
		await vi.waitFor(() => expect(replies(chat.messages)).toEqual(['Hey. What are we getting into today?']));
		expect(api.getChatMessages).toHaveBeenCalledTimes(2);
		chat.destroy();
	});
});
