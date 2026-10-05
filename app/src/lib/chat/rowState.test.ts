import { describe, it, expect } from 'vitest';
import {
	agentUnread,
	conversationRead,
	endWork,
	openNeedsRead,
	replyArrived,
	rowMark,
	seedAgentUnread,
	seedTeamUnread,
	snapshotWork,
	startWork,
	teamWorking,
	type Working
} from './rowState';

const KEY = 'agent:ap:thread:c1';
const closed = { key: '', visible: true };

describe('unread', () => {
	it('a reply that arrives while the conversation is closed is unread', () => {
		const r = replyArrived(new Set(), KEY, true, closed);
		expect([...r.unread]).toEqual([KEY]);
		expect(r.markRead).toBe(false);
		expect(agentUnread(r.unread, 'ap')).toBe(true);
		expect(agentUnread(r.unread, 'cfo')).toBe(false);
	});

	it('opening the conversation reads it', () => {
		const unread = new Set([KEY]);
		expect(openNeedsRead(unread, { key: KEY, visible: true })).toBe(true);
		const after = conversationRead(unread, KEY);
		expect(after.size).toBe(0);
		expect(openNeedsRead(after, { key: KEY, visible: true })).toBe(false);
	});

	it('a reply that arrives while the conversation is open and on screen stays read', () => {
		const r = replyArrived(new Set(), KEY, true, { key: KEY, visible: true });
		expect(r.unread.size).toBe(0);
		expect(r.markRead).toBe(true);
	});

	it('open but the window hidden: unread until the owner looks', () => {
		const hidden = { key: KEY, visible: false };
		const r = replyArrived(new Set(), KEY, true, hidden);
		expect([...r.unread]).toEqual([KEY]);
		expect(openNeedsRead(r.unread, hidden)).toBe(false);
		expect(openNeedsRead(r.unread, { key: KEY, visible: true })).toBe(true);
	});

	it('a turn that left nothing new to read clears nothing it should not and lights nothing', () => {
		const r = replyArrived(new Set(), KEY, false, closed);
		expect(r.unread.size).toBe(0);
		expect(r.markRead).toBe(false);
		// The bot says it is read (another surface read it): the dot goes.
		expect(replyArrived(new Set([KEY]), KEY, false, closed).unread.size).toBe(0);
	});

	it('read on another surface clears it here', () => {
		expect(conversationRead(new Set([KEY, 'team:t1']), KEY)).toEqual(new Set(['team:t1']));
	});

	it('the roster and the team list each replace only their own', () => {
		let unread = new Set(['agent:old:web', 'team:t1']);
		unread = seedAgentUnread(unread, [KEY]);
		expect(unread).toEqual(new Set([KEY, 'team:t1']));
		unread = seedTeamUnread(unread, ['t2']);
		expect(unread).toEqual(new Set([KEY, 'team:t2']));
	});
});

describe('working', () => {
	it('a run start marks the employee working and its end clears it', () => {
		let w: Working = {};
		w = startWork(w, 'ap', KEY);
		expect(w.ap).toEqual({ [KEY]: '' });
		w = startWork(w, 'ap', KEY, 'reading a file');
		expect(w.ap[KEY]).toBe('reading a file');
		// A thinking event keeps the last verb.
		w = startWork(w, 'ap', KEY);
		expect(w.ap[KEY]).toBe('reading a file');
		w = endWork(w, 'ap', KEY);
		expect(w.ap).toBeUndefined();
	});

	it('the main employee runs with an empty agentId', () => {
		expect(Object.keys(startWork({}, '', 'agent:assistant:web'))).toEqual(['assistant']);
	});

	it('the snapshot drops an employee no run names any more', () => {
		const w = startWork(startWork({}, 'ap', KEY), 'cfo', 'agent:cfo:web');
		const next = snapshotWork(w, [{ entityId: 'cfo', sessionKey: 'agent:cfo:web', activity: 'drafting' }]);
		expect(Object.keys(next)).toEqual(['cfo']);
		expect(next.cfo['agent:cfo:web']).toBe('drafting');
	});

	it('a team works while a member works in its seat for the team', () => {
		const w = startWork({}, 'ap', 'agent:ap:coworker:team:t1');
		expect(teamWorking(w, 't1')).toBe(true);
		expect(teamWorking(w, 't2')).toBe(false);
		expect(teamWorking(startWork({}, 'ap', 'agent:ap:coworker:cfo'), 't1')).toBe(false);
	});

	it('working wins; a finished run with a new reply becomes the unread dot', () => {
		expect(rowMark(true, true)).toBe('working');
		expect(rowMark(false, true)).toBe('unread');
		expect(rowMark(false, false)).toBe(null);
	});
});
