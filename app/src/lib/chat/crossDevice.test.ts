/**
 * One conversation, two devices. This window shows the thread; the turn runs
 * from the phone (or another window): this window never sent anything, and
 * still shows the owner's message, the work as it happens, and its end. A
 * message sent while the turn runs (from either side) sits where the thread's
 * history puts it, and the work after it renders UNDER it: with the work
 * attached above the message, the window sat still under the owner's last
 * message until a refresh (2026-10-08).
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';

const handlers = new Map<string, Array<(data: any) => void>>();
const sent: Array<[string, Record<string, unknown>]> = [];

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
const api = vi.hoisted(() => ({
	getChatMessages: vi.fn(async () => ({
		messages: [{ id: 'r1', chatId: 'c1', role: 'user', content: 'Make the video.', createdAt: 1 }]
	})),
	getSessionGoal: vi.fn(async () => ({ goal: null }))
}));
vi.mock('$lib/api/nebo', () => api);

import { createChatController, type ChatMessage } from './controller.svelte';

const KEY = 'agent:a:thread:c1';
const PHONE = 'socket-phone';

function server(type: string, data: Record<string, unknown>) {
	for (const fn of handlers.get(type) ?? []) fn({ session_id: KEY, agentId: 'a', ...data });
}

/** The thread as rows a reader sees, top to bottom: the owner's words, each
 *  reply's text and the tools it ran. */
function rows(messages: ChatMessage[]): string[] {
	return messages.flatMap((m) => {
		if (m.type === 'user') return [`owner: ${m.content}`];
		if (m.type === 'assistant')
			return [...(m.content ? [`reply: ${m.content}`] : []), ...(m.tools ?? []).map((t) => `tool: ${t.toolId}:${t.status}`)];
		return [];
	});
}

describe('a turn another device started', () => {
	beforeEach(() => {
		handlers.clear();
		sent.length = 0;
	});

	it('shows the message, the stream, the tools and the end of a run this window never started', () => {
		const chat = createChatController({ agentId: 'a', sessionKey: KEY });
		server('chat_user_message', { id: 'u1', content: 'Make the video.', client_id: PHONE, createdAt: 1 });
		server('chat_created', {});
		expect(chat.isLoading).toBe(true);
		server('chat_stream', { content: 'Cutting it now.' });
		server('tool_start', { tool_id: 't1', tool: 'video', label: 'Cutting the video' });
		server('tool_result', { tool_id: 't1', outcome: 'Cut the video' });
		server('chat_stream', { content: 'Done.' });
		server('chat_complete', {});

		expect(chat.isLoading).toBe(false);
		expect(rows(chat.messages)).toEqual([
			'owner: Make the video.',
			'reply: Cutting it now.',
			'tool: t1:success',
			'reply: Done.'
		]);
		chat.destroy();
	});

	it('puts the work after a message from the phone under that message, not above it', () => {
		const chat = createChatController({ agentId: 'a', sessionKey: KEY });
		chat.send('Make the video.');
		server('chat_created', {});
		server('chat_stream', { content: 'Working on it.' });
		server('tool_start', { tool_id: 't1', tool: 'video' });
		// The phone says something while the turn runs.
		server('chat_user_message', { id: 'u2', content: 'Use all 8 clips.', client_id: PHONE, createdAt: 2 });
		server('chat_complete', { stop_reason: 'queued_into_running_turn', message_id: 'm2' });
		// The call that was running finishes; the next steps run tools with no
		// words between them, as models often do.
		server('tool_result', { tool_id: 't1' });
		server('tool_start', { tool_id: 't2', tool: 'video' });
		server('tool_result', { tool_id: 't2' });
		server('chat_taken_in', { message_ids: ['m2'] });
		server('tool_start', { tool_id: 't3', tool: 'video' });

		expect(rows(chat.messages)).toEqual([
			'owner: Make the video.',
			'reply: Working on it.',
			'tool: t1:success',
			'owner: Use all 8 clips.',
			'tool: t2:success',
			'tool: t3:running'
		]);
		// Taken in: no longer waiting.
		const last = chat.messages.filter((m) => m.type === 'user').at(-1) as { pending?: boolean };
		expect(last.pending).toBeFalsy();
		expect(chat.isLoading).toBe(true);

		server('tool_result', { tool_id: 't3' });
		server('chat_stream', { content: 'All 8 clips are in.' });
		server('chat_complete', {});
		expect(rows(chat.messages).slice(-2)).toEqual(['tool: t3:success', 'reply: All 8 clips are in.']);
		expect(chat.isLoading).toBe(false);
		chat.destroy();
	});

	it('keeps text that streamed before a message in the reply it streamed into', () => {
		const chat = createChatController({ agentId: 'a', sessionKey: KEY });
		server('chat_user_message', { id: 'u1', content: 'Start.', client_id: PHONE, createdAt: 1 });
		server('chat_created', {});
		// Buffered for the paced reveal, not yet drawn, when the message lands.
		server('chat_stream', { content: 'Half a sentence' });
		chat.send('One more thing.');
		server('chat_stream', { content: 'Next part.' });
		server('chat_complete', {});
		expect(rows(chat.messages)).toEqual([
			'owner: Start.',
			'reply: Half a sentence',
			'owner: One more thing.',
			'reply: Next part.'
		]);
		chat.destroy();
	});

	it('ignores another conversation', () => {
		const chat = createChatController({ agentId: 'a', sessionKey: KEY });
		for (const fn of handlers.get('chat_user_message') ?? [])
			fn({ session_id: 'agent:a:thread:other', id: 'x', content: 'Elsewhere.', client_id: PHONE });
		for (const fn of handlers.get('chat_stream') ?? []) fn({ session_id: 'agent:a:thread:other', agentId: 'a', content: 'Not here.' });
		expect(chat.messages).toEqual([]);
		chat.destroy();
	});
});

describe('a turn that ends with a call it never ran', () => {
	beforeEach(() => {
		handlers.clear();
		sent.length = 0;
	});

	it('reads the thread once, since the live view no longer matches it', async () => {
		const read = api.getChatMessages;
		const chat = createChatController({ agentId: 'a', sessionKey: KEY });
		await chat.loadHistory('c1');
		read.mockClear();
		server('chat_created', {});
		server('chat_stream', { content: 'On it.' });
		server('tool_start', { tool_id: 't1', tool: 'video' });
		server('tool_result', { tool_id: 't1' });
		// Announced, then the owner's message ended the turn before it ran.
		server('tool_start', { tool_id: 't2', tool: 'video' });
		server('chat_complete', {});
		await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(1));
		// The thread as kept: the call that never ran is not in it.
		await vi.waitFor(() => expect(chat.messages.some((m) => m.type === 'assistant' && m.tools?.some((t) => t.toolId === 't2'))).toBe(false));

		// A turn whose calls all finished needs no read.
		read.mockClear();
		server('chat_created', {});
		server('tool_start', { tool_id: 't3', tool: 'video' });
		server('tool_result', { tool_id: 't3' });
		server('chat_complete', {});
		await new Promise((r) => setTimeout(r, 50));
		expect(read).not.toHaveBeenCalled();
		chat.destroy();
	});
});
