import { describe, it, expect } from 'vitest';
import { query } from './launcher';
import { clientId } from '$lib/websocket/origin';

describe("an app's page is opened naming this screen", () => {
	it('with the chat it is opened from and this client', () => {
		const q = new URLSearchParams(query('chat 1'));
		expect(q.get('thread')).toBe('chat 1');
		expect(q.get('client')).toBe(clientId);
	});

	it('names this client when opened from no chat', () => {
		const q = new URLSearchParams(query());
		expect(q.has('thread')).toBe(false);
		expect(q.get('client')).toBe(clientId);
	});
});
