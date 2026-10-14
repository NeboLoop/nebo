// One helper decides what an Inbox item is and how it is answered: from the
// server's row (status, options, type), never from a status fetched on the
// side. These pin the kinds, the answers, the top-3 of Needs you, the
// grouping of updates by employee, and the one answer path.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

const answered: { id: string; req: unknown }[] = [];
vi.mock('$lib/api/nebo', () => ({
	answerNotification: vi.fn(async (id: string, req: { value: string; via: string }) => {
		answered.push({ id, req });
		return { notification: { ...open(id), status: 'answered', answer: req.value, answeredVia: req.via } };
	}),
	unreadCount: vi.fn(async () => ({ count: 0, needsYou: 0, unread: 0 }))
}));

import type { WaitingAsk } from '$lib/api/neboComponents';
import { notifications } from '$lib/stores/notifications';
import {
	answerItem,
	answerVia,
	answersOf,
	answeredElsewhere,
	canRevertLearning,
	cardKindOf,
	groupUpdates,
	ruleIdOf,
	sectionOf,
	splitGroups,
	surfaceKey,
	takesOwnWords,
	topNeedsYou,
	undoOf,
	waitingItemId,
	type InboxItem
} from './item';

const opts = (...v: string[]) => v.map((value) => ({ label: value, value }));
function update(id: string, agentId: string, createdAt = 1, over: Partial<InboxItem> = {}): InboxItem {
	return { id, type: 'info', title: id, body: '', agentId, createdAt, needsYou: false, options: [], status: 'update', ...over };
}
function open(id: string, over: Partial<InboxItem> = {}): InboxItem {
	return { id, type: 'info', title: id, body: '', createdAt: 1, needsYou: true, options: opts('a', 'b'), status: 'open', ...over };
}

describe('kinds', () => {
	it('reads the section from the server status', () => {
		expect(sectionOf(open('q:1'))).toBe('needs_you');
		expect(sectionOf(update('u:1', 'store'))).toBe('updates');
		expect(sectionOf(open('q:1', { status: 'answered' }))).toBe('history');
		expect(sectionOf(update('u:1', 'store', 1, { status: 'archived' }))).toBe('history');
		expect(sectionOf(update('brief:2026-10-14', 'assistant', 1, { type: 'brief' }))).toBe('brief');
	});

	it('draws the offer, the handled update and the brief specially', () => {
		expect(cardKindOf(open('learn:p1', { type: 'rule_offer' }))).toBe('offer');
		expect(cardKindOf(update('handled:r1:1', 'assistant', 1, { type: 'handled' }))).toBe('handled');
		expect(cardKindOf(open('permission-ask:a1', { type: 'permission_ask' }))).toBe('needs_you');
		expect(cardKindOf(update('u:1', 'store'))).toBe('update');
		// An answered offer is History, whatever its type.
		expect(cardKindOf(open('learn:p1', { type: 'rule_offer', status: 'answered' }))).toBe('history');
	});
});

describe('answers', () => {
	it("takes a handled item's Undo out of its answers", () => {
		const h = update('handled:r1:7', 'assistant', 1, { type: 'handled', options: [{ label: 'Undo', value: 'undo' }] });
		expect(undoOf(h)).toEqual({ label: 'Undo', value: 'undo' });
		expect(answersOf(h)).toEqual([]);
		expect(undoOf(update('handled:r1:8', 'assistant', 1, { type: 'handled' }))).toBeNull();
		expect(ruleIdOf(h)).toBe('r1');
		expect(ruleIdOf(update('u:1', 'store'))).toBeNull();
	});

	it("an employee's own question takes the owner's words; a fixed decision does not", () => {
		expect(takesOwnWords(open('q:1'))).toBe(true);
		expect(takesOwnWords(open('permission-ask:a', { type: 'permission_ask' }))).toBe(false);
		expect(takesOwnWords(open('approval:t1', { type: 'approval' }))).toBe(false);
		expect(takesOwnWords(open('learn:p1', { type: 'rule_offer' }))).toBe(false);
		expect(takesOwnWords(open('artifact-update:skill:x:1.0', {}))).toBe(false);
		expect(takesOwnWords(open('q:1', { status: 'answered' }))).toBe(false);
	});

	it('a decision with no options is answered on its own screen', () => {
		expect(answeredElsewhere(open('question-ask:r1', { options: [] }))).toBe(true);
		expect(answeredElsewhere(open('q:1'))).toBe(false);
	});

	it('an approved proposal can be undone from History; a rule offer cannot', () => {
		expect(canRevertLearning(open('learn:p1', { status: 'answered', answer: 'Approved' }))).toBe(true);
		expect(canRevertLearning(open('learn:p1', { status: 'answered', answer: 'Declined' }))).toBe(false);
		expect(canRevertLearning(open('learn:p1', { type: 'rule_offer', status: 'answered', answer: 'Approved' }))).toBe(false);
	});

	it('names where the owner answered in plain words', () => {
		expect(surfaceKey('mobile')).toBe('inbox.viaMobile');
		expect(surfaceKey('desktop')).toBe('inbox.viaDesktop');
		expect(surfaceKey('web')).toBe('inbox.viaWeb');
		expect(surfaceKey('chat')).toBe('inbox.viaChat');
		expect(surfaceKey(undefined)).toBeNull();
	});

	it('a waiting ask is its Inbox item when it has one', () => {
		const ask = { id: 'x1', kind: 'permission', relayed: false } as WaitingAsk;
		expect(waitingItemId(ask)).toBe('permission-ask:x1');
		expect(waitingItemId({ ...ask, kind: 'question' })).toBeNull();
		expect(waitingItemId({ ...ask, kind: 'question', relayed: true })).toBe('question-ask:x1');
	});
});

describe('Needs you', () => {
	it('shows the newest three, then "Show N more"', () => {
		const items = [1, 2, 3, 4, 5].map((i) => open(`q:${i}`, { createdAt: i }));
		const top = topNeedsYou([...items, update('u:1', 's')]);
		expect(top.shown.map((n) => n.id)).toEqual(['q:5', 'q:4', 'q:3']);
		expect(top.more).toBe(2);
		expect(topNeedsYou(items, true).shown).toHaveLength(5);
		expect(topNeedsYou(items.slice(0, 2)).more).toBe(0);
	});
});

describe('Updates by employee', () => {
	it('groups by employee, newest group first, and counts unread and handled', () => {
		const groups = groupUpdates([
			update('a1', 'ads', 5, { readAt: 6 }),
			update('s1', 'store', 3),
			update('s2', 'store', 9),
			update('h1', 'primary', 4, { type: 'handled' }),
			open('q:1', { agentId: 'store' }),
			update('b', 'assistant', 10, { type: 'brief' })
		]);
		expect(groups.map((g) => [g.agentId, g.items.map((n) => n.id), g.unread, g.handled])).toEqual([
			['store', ['s2', 's1'], 2, 0],
			['ads', ['a1'], 0, 0],
			['assistant', ['h1'], 1, 1]
		]);
	});

	it('collapses the tail behind "<Name> and N more employees", never for just one', () => {
		const g = (id: string, at: number) => groupUpdates([update(id, id, at)])[0];
		const five = [g('a', 5), g('b', 4), g('c', 3), g('d', 2), g('e', 1)];
		const split = splitGroups(five);
		expect(split.shown.map((x) => x.agentId)).toEqual(['a', 'b', 'c']);
		expect(split.rest.map((x) => x.agentId)).toEqual(['d', 'e']);
		expect(splitGroups(five.slice(0, 4)).rest).toEqual([]);
		expect(splitGroups(five, true).rest).toEqual([]);
	});
});

describe('the one answer path', () => {
	beforeEach(() => {
		answered.length = 0;
		notifications.set([open('q:1')]);
	});

	it('answers through the answer endpoint and settles the Inbox copy', async () => {
		const n = await answerItem('q:1', 'lumen', 'chat');
		expect(answered).toEqual([{ id: 'q:1', req: { value: 'lumen', via: 'chat' } }]);
		expect(n.status).toBe('answered');
		expect(get(notifications)[0]).toMatchObject({ status: 'answered', answer: 'lumen', answeredVia: 'chat' });
	});

	it('answers from the web outside the desktop app', async () => {
		expect(answerVia()).toBe('web');
		await answerItem('q:1', 'a');
		expect(answered[0].req).toEqual({ value: 'a', via: 'web' });
	});
});
