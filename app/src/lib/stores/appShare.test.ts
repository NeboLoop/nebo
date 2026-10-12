import { describe, it, expect } from 'vitest';
import { answersShareRequest, shareRequestOf, shareRequestPath, type ShareRequest } from './appShare';
import { clientId } from '$lib/websocket/origin';

const req: ShareRequest = { agentId: 'design-studio', chatId: 'c-1', artifact: '/api/v1/files/Design Studio/Launch.html', title: 'Launch.html' };

describe('an app asking to share a file', () => {
	it('is answered in the chat the app was open on, and no other', () => {
		expect(answersShareRequest(req, 'design-studio', 'c-1')).toBe(true);
		expect(answersShareRequest(req, 'design-studio', 'c-2')).toBe(false);
		expect(answersShareRequest(req, 'bookkeeper', 'c-1')).toBe(false);
		expect(answersShareRequest(null, 'design-studio', 'c-1')).toBe(false);
	});

	it('is answered in any of the app chats when it was open on none', () => {
		expect(answersShareRequest({ ...req, chatId: '' }, 'design-studio', 'c-9')).toBe(true);
		expect(answersShareRequest({ ...req, chatId: '' }, 'design-studio', '')).toBe(true);
	});

	it('goes to that chat', () => {
		expect(shareRequestPath(req)).toBe('/design-studio/threads/c-1');
		expect(shareRequestPath({ ...req, chatId: '' })).toBe('/design-studio/threads');
	});
});

describe('a share opens only on the device whose page asked', () => {
	// `app_share_requested` as the server stamps it (handlers/apps.rs share_file).
	const event = { agentId: 'design-studio', chatId: 'c-1', artifact: req.artifact, title: req.title, session_id: 'c-1' };
	const PHONE = 'phone-client-b';

	it('a share started on the phone (client B) opens no dialog here; the phone opens it', () => {
		expect(shareRequestOf({ ...event, client_id: PHONE })).toBeNull();
	});

	it('a share started here (client A) opens here', () => {
		expect(shareRequestOf({ ...event, client_id: clientId })).toEqual(req);
	});

	it('a share no client claims opens nowhere', () => {
		expect(shareRequestOf({ ...event, client_id: null })).toBeNull();
		expect(shareRequestOf(event)).toBeNull();
	});

	it('a malformed event opens nothing', () => {
		expect(shareRequestOf({ ...event, client_id: clientId, artifact: '' })).toBeNull();
		expect(shareRequestOf({ ...event, client_id: clientId, agentId: '' })).toBeNull();
		expect(shareRequestOf(null)).toBeNull();
	});
});
