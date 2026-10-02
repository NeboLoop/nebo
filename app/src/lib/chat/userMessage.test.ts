import { describe, expect, it } from 'vitest';
import { userMessageRow } from './userMessage';

const KEY = 'agent:app-1:thread:t1';

describe('userMessageRow', () => {
	it('shows a message sent from elsewhere in the open conversation', () => {
		const row = userMessageRow(
			{ session_id: KEY, id: 'm1', content: 'These errors came up in Ostrion', createdAt: 5, client_id: null },
			KEY,
			'this-page'
		);
		expect(row).toEqual({ id: 'm1', content: 'These errors came up in Ostrion', createdAt: 5 });
	});

	it('never shows twice what this page typed', () => {
		expect(userMessageRow({ session_id: KEY, content: 'hi', client_id: 'this-page' }, KEY, 'this-page')).toBeNull();
	});

	it('shows what another window or the phone typed', () => {
		expect(userMessageRow({ session_id: KEY, content: 'hi', client_id: 'phone' }, KEY, 'this-page')?.content).toBe('hi');
	});

	it('belongs only to its own conversation', () => {
		expect(userMessageRow({ session_id: 'agent:app-1:thread:t2', content: 'hi' }, KEY, 'p')).toBeNull();
		expect(userMessageRow({ session_id: KEY, content: 'hi' }, null, 'p')).toBeNull();
		expect(userMessageRow({ session_id: KEY, content: '  ' }, KEY, 'p')).toBeNull();
	});
});
