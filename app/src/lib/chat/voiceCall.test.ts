/**
 * A call in an open conversation. The owner's spoken words land as their
 * row (`voice_message`, by id), and the work the employee does for the call
 * streams in as a typed turn's does: its text, its steps and its end, with
 * no read of the thread. The window used to read the whole thread again on
 * every spoken row, and the written work showed a turn late, or only after
 * the call (2026-10-20).
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';

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
const api = vi.hoisted(() => ({
	getChatMessages: vi.fn(async () => ({
		messages: [{ id: 'r1', chatId: 'c1', role: 'user', content: 'Morning.', createdAt: 1 }]
	})),
	getSessionGoal: vi.fn(async () => ({ goal: null }))
}));
vi.mock('$lib/api/nebo', () => api);

import { createChatController, type ChatMessage } from './controller.svelte';

const KEY = 'agent:a:thread:c1';

function emit(type: string, data: Record<string, unknown>) {
	for (const fn of handlers.get(type) ?? []) fn(data);
}
function run(type: string, data: Record<string, unknown>) {
	emit(type, { session_id: KEY, agentId: 'a', turn_id: 'turn-1', ...data });
}

function rows(messages: ChatMessage[]): string[] {
	return messages.flatMap((m) => {
		if (m.type === 'user') return [`owner: ${m.content}`];
		if (m.type === 'assistant')
			return [...(m.content ? [`reply: ${m.content}`] : []), ...(m.tools ?? []).map((t) => `tool: ${t.toolId}:${t.status}`)];
		return [];
	});
}

describe('a call in the open conversation', () => {
	beforeEach(() => {
		handlers.clear();
		api.getChatMessages.mockClear();
	});

	it('shows the spoken words and streams the written work live, without reading the thread', async () => {
		const chat = createChatController({ agentId: 'a', sessionKey: KEY });
		await chat.loadHistory('c1');
		api.getChatMessages.mockClear();

		emit('voice_message', { id: 'v1', chatId: 'c1', role: 'user', content: 'Find me a flight to Denver.' });
		// The same row announced again is the same row.
		emit('voice_message', { id: 'v1', chatId: 'c1', role: 'user', content: 'Find me a flight to Denver.' });
		// Another conversation's call is not this one's.
		emit('voice_message', { id: 'x1', chatId: 'other', role: 'user', content: 'Elsewhere.' });

		run('chat_stream', { content: 'Looking now. ' });
		run('tool_start', { tool_id: 't1', tool: 'web' });
		run('tool_result', { tool_id: 't1' });
		run('chat_stream', { content: 'Three flights leave tomorrow.' });
		expect(chat.isLoading).toBe(true);
		run('chat_complete', { artifacts: [] });

		expect(rows(chat.messages)).toEqual([
			'owner: Morning.',
			'owner: Find me a flight to Denver.',
			'reply: Looking now. ',
			'tool: t1:success',
			'reply: Three flights leave tomorrow.'
		]);
		expect(chat.isLoading).toBe(false);
		await new Promise((r) => setTimeout(r, 50));
		expect(api.getChatMessages).not.toHaveBeenCalled();
		chat.destroy();
	});

	it('puts a spoken reply the server kept in the thread as its own row', async () => {
		const chat = createChatController({ agentId: 'a', sessionKey: KEY });
		await chat.loadHistory('c1');
		emit('voice_message', { id: 'v1', chatId: 'c1', role: 'user', content: 'Hello?' });
		emit('voice_message', { id: 'v2', chatId: 'c1', role: 'assistant', content: 'Hi, I am here.' });
		expect(rows(chat.messages)).toEqual(['owner: Morning.', 'owner: Hello?', 'reply: Hi, I am here.']);
		chat.destroy();
	});
});
