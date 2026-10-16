// A project's card in its thread: the lead's line handing work out reads
// "{Member}: {task}" with where it stands (handed out, working, done,
// couldn't finish, stopped) and when it counts as done; a hand-off line in
// an employee's chat stays the one-line "→ {Member}: {ask}" with its status.
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render } from 'svelte/server';
import { readable } from 'svelte/store';
import { addMessages, init } from 'svelte-i18n';
import en from '$lib/i18n/locales/en.json';

vi.mock('$app/stores', () => ({ page: readable({ url: new URL('http://nebo.test/teams/launch?project=ph') }) }));
vi.mock('$lib/websocket/client', () => ({
	getWebSocketClient: () => ({ on: () => () => {} })
}));

import type { HandoffView } from '$lib/api/neboComponents';
import { handoffs } from '$lib/stores/handoffs';
import HandoffLine from './HandoffLine.svelte';

beforeAll(() => {
	addMessages('en', en);
	init({ fallbackLocale: 'en', initialLocale: 'en' });
});

const card: HandoffView = {
	id: 'h-1',
	kind: 'assignment',
	fromAgentId: 'pm',
	fromName: 'Product Marketer',
	toAgentId: 'sw',
	toName: 'Script Writer',
	teamId: 'launch',
	senderSession: 'agent:pm:coworker:team:launch:project:ph',
	senderLink: '/teams/launch?project=ph',
	receiverSession: 'agent:sw:case:c-1',
	receiverLink: '/sw/cases/c-1',
	ask: 'hero 30s + cutdown 15s',
	status: 'running',
	result: '',
	error: '',
	createdAt: 1,
	projectId: 'ph',
};

const text = (html: string) => html.replace(/<!--[^>]*-->/g, '').replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').trim();
const renderCard = (status?: string, doneWhen = 'both scripts saved and on brand') => {
	handoffs.set(status ? { [card.id]: { ...card, status } } : {});
	return render(HandoffLine, {
		props: { card: true, to: 'Script Writer', ask: 'hero 30s + cutdown 15s', handoffId: card.id, doneWhen },
	}).body;
};

describe('HandoffLine as a card', () => {
	beforeEach(() => handoffs.set({}));

	it('names the member and the task, says where it stands, and when it is done', () => {
		const out = renderCard('running');
		expect(text(out)).toBe('Script Writer: hero 30s + cutdown 15s working Done when: both scripts saved and on brand');
		expect(out).toContain('handoff-card-state-working');
		expect(out).toContain('href="/sw/cases/c-1"');
	});

	it('says each state in the card words', () => {
		expect(text(renderCard('queued'))).toContain('handed out');
		expect(text(renderCard('done'))).toContain('done');
		expect(text(renderCard('failed'))).toContain("couldn't finish");
		expect(renderCard('failed')).toContain('handoff-card-state-notFinished');
		expect(text(renderCard('stopped'))).toContain('stopped');
	});

	it('shows no state before the hand-off is read, and no done-when the lead did not give', () => {
		const out = text(renderCard(undefined, ''));
		expect(out).toBe('Script Writer: hero 30s + cutdown 15s');
	});

	it('leaves the hand-off line in an employee chat as it was', () => {
		handoffs.set({ [card.id]: card });
		const out = render(HandoffLine, { props: { to: 'Script Writer', ask: 'hero 30s', handoffId: card.id } }).body;
		expect(out).toContain('handoff-line');
		expect(out).not.toContain('handoff-card');
		expect(text(out)).toBe('→ Script Writer: hero 30s + cutdown 15s ⎿ Working on it');
	});
});
