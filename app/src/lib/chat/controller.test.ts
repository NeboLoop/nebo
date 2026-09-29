/**
 * The chat's Stop names exactly the conversation its turn runs in: the
 * cancel frame carries the same employee, session key and channel as the
 * chat frame that started the turn, which the server resolves into the run
 * registry's key. A Stop that named less (a session with no employee, or an
 * employee with no channel) could resolve to a key no run is under.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';

const sent: Array<[string, Record<string, unknown>]> = [];

vi.mock('$lib/websocket/client', () => ({
	getWebSocketClient: () => ({
		send: (type: string, data: Record<string, unknown>) => sent.push([type, data]),
		on: () => () => {},
		onStatus: () => () => {},
		getDisruptionCount: () => 0
	})
}));
vi.mock('$lib/api/gocliRequest', () => ({ sendClientEvent: () => {} }));
vi.mock('$lib/marketplace/installCodes', () => ({ sendInstallCode: () => false }));

import { createChatController } from './controller.svelte';

function frame(type: string): Record<string, unknown> {
	const found = sent.filter(([t]) => t === type);
	expect(found).toHaveLength(1);
	return found[0][1];
}

/** The fields that name a turn's conversation. */
function naming(data: Record<string, unknown>) {
	const { agent_id, session_id, channel } = data;
	return Object.fromEntries(
		Object.entries({ agent_id, session_id, channel }).filter(([, v]) => v !== undefined)
	);
}

describe('chat Stop', () => {
	beforeEach(() => {
		sent.length = 0;
	});

	it('names the thread by the key its turn is registered under', () => {
		const chat = createChatController({ agentId: 'a', sessionKey: 'agent:a:thread:c1' });
		chat.send('hello');
		chat.stop();
		expect(frame('cancel')).toEqual({ agent_id: 'a', session_id: 'agent:a:thread:c1' });
		expect(naming(frame('cancel'))).toEqual(naming(frame('chat')));
		chat.destroy();
	});

	it('names the employee and channel when there is no session key, as its send did', () => {
		const chat = createChatController({ agentId: 'a', channel: 'help:mail' });
		chat.send('hello');
		chat.stop();
		expect(frame('cancel')).toEqual({ agent_id: 'a', channel: 'help:mail' });
		expect(naming(frame('cancel'))).toEqual(naming(frame('chat')));
		chat.destroy();
	});

	it('follows the session the controller moved to', () => {
		const chat = createChatController({ agentId: 'a', sessionKey: 'agent:a:thread:c1' });
		chat.setSessionKey('agent:a:thread:c2');
		chat.stop();
		expect(frame('cancel')).toEqual({ agent_id: 'a', session_id: 'agent:a:thread:c2' });
		chat.destroy();
	});
});
