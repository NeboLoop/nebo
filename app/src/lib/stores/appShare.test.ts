import { describe, it, expect } from 'vitest';
import { answersShareRequest, shareRequestPath, type ShareRequest } from './appShare';

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
