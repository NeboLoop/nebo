import { describe, expect, it } from 'vitest';
import { conversationTitle, isSessionKey, threadIdFromKey, threadKey } from './sessionKey';

const EMP = 'ae6bc1be-9981-400e-95e2-6d3e1a713c29';
const WEB = `agent:${EMP}:web`;

describe('threadKey', () => {
	it('keys a thread as agent:<id>:thread:<chat>', () => {
		expect(threadKey(EMP, 'c1')).toBe(`agent:${EMP}:thread:c1`);
		expect(threadIdFromKey(threadKey(EMP, 'c1'))).toBe('c1');
	});

	it('uses a legacy conversation whose id is its session key as it is', () => {
		// The app console's Send to writes in agent:<id>:web; the desktop must
		// write in the same session, not agent:<id>:thread:agent:<id>:web.
		expect(threadKey(EMP, WEB)).toBe(WEB);
	});
});

describe('conversationTitle: no raw session key in the header or the sidebar', () => {
	it('shows the employee name in place of a chat titled with its session key', () => {
		expect(isSessionKey(WEB)).toBe(true);
		expect(conversationTitle({ title: WEB, name: WEB }, 'Flip-Flap')).toBe('Flip-Flap');
	});

	it('keeps a real title', () => {
		expect(conversationTitle({ title: 'Ten levels', name: 'Ten levels' }, 'Flip-Flap')).toBe('Ten levels');
		expect(conversationTitle({ title: 'agent: who owns billing', name: '' }, 'X')).toBe('agent: who owns billing');
	});

	it('falls back while the conversation is not known yet', () => {
		expect(conversationTitle(undefined, 'Flip-Flap')).toBe('Flip-Flap');
		expect(conversationTitle({ title: '', name: '' }, 'Chat')).toBe('Chat');
	});
});
