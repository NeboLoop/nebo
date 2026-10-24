// Ask cards stand off the chat while they wait, and fold to a receipt once
// answered.
//
// The owner: the question card was white on the white chat with a faint
// border ("make it a different color so it stands off the background"), and
// an answered card stayed a full card with the question and a blue pill.
// Every OPEN ask (a question, a permission ask, an approval) is now the one
// tinted decision card (`.ask-card decision-card`); an answered one
// collapses in place to a muted one-line receipt with no card chrome, and
// tapping it opens the question read-only (a native <details>, folded
// until tapped).
import { describe, it, expect, vi, beforeAll } from 'vitest';
import { render } from 'svelte/server';
import { addMessages, init } from 'svelte-i18n';
import en from '$lib/i18n/locales/en.json';
import { readFileSync } from 'node:fs';

vi.mock('$lib/api/nebo', () => ({
	authLoginAccount: vi.fn(),
	listPlugins: vi.fn(async () => ({ plugins: [] })),
	submitCode: vi.fn()
}));
vi.mock('$lib/websocket/client', () => ({
	getWebSocketClient: () => ({ on: () => () => {} })
}));
vi.mock('$lib/stores/permissionAsks', () => ({ answerAsk: vi.fn() }));
vi.mock('$lib/stores/approvals', () => ({ answerApproval: vi.fn() }));

import AskWidget, { SKIP_VALUE } from './AskWidget.svelte';
import ApprovalAskCard from './ApprovalAskCard.svelte';
import PermissionAskCard from '$lib/components/PermissionAskCard.svelte';

beforeAll(() => {
	addMessages('en', en);
	init({ fallbackLocale: 'en', initialLocale: 'en' });
});

const question = "What's the **most important** thing to fix this month?";
const widgets = [{ type: 'options' as const, options: ['Marketing', 'Sales', { label: 'Hiring', description: 'Two roles open' }] }];
const ask = (response?: string, extra: Record<string, unknown> = {}) =>
	render(AskWidget, { props: { requestId: 'r1', prompt: question, widgets, response, onSubmit: () => {}, ...extra } }).body;

/** The receipt's folded part, with Svelte's hydration comments dropped. */
const text = (html: string) => html.replace(/<!--[^>]*-->/g, '').replace(/<[^>]+>/g, '').replace(/\s+/g, ' ').trim();
const summary = (html: string) => text(html.match(/<summary[^>]*>([\s\S]*?)<\/summary>/)?.[1] ?? '');

const permission = {
	id: 'p1',
	kind: 'permission',
	agentId: 'a1',
	employee: 'Bookkeeper',
	sessionKey: 's',
	sentence: 'send the invoice to the client',
	what: 'Send the invoice to the client',
	why: '',
	details: [],
	reason: 'It goes outside the company.',
	reasonKey: '',
	reasonValues: {},
	allowAlways: true,
	thisOnce: true,
	status: 'open',
	chatId: 'c',
	blocking: true,
	createdAt: 0
};

describe('ask cards', () => {
	it("a question of the code check's is worded by its keys, its choices too", () => {
		const keyed = [{
			type: 'options' as const,
			options: ['Keep going', 'Stop here'],
			questionKey: 'verify.ask.reviewFailed',
			questionValues: { project: '/work/shop', count: 2, tries: 3 },
			optionKeys: ['verify.ask.keepGoing', 'verify.ask.stopHere']
		}];
		const html = render(AskWidget, { props: { requestId: 'r2', prompt: 'FALLBACK', widgets: keyed, onSubmit: () => {} } }).body;
		expect(text(html)).toContain('The review of /work/shop could not finish: 2 reviewers failed 3 times. Run them again?');
		expect(html).not.toContain('FALLBACK');
		expect(text(html)).toContain('Keep going');
	});


	it('an open question is a tinted ask card, not a receipt', () => {
		const html = ask(undefined, { disabled: false });
		expect(html).toContain('class="ask-card decision-card"');
		expect(html).not.toContain('ask-receipt');
		expect(html).not.toContain('bg-base-200 px-4');
	});

	it('an answered question is a one-line receipt: Asked · question → answer, no card', () => {
		const html = ask('Marketing');
		expect(html).not.toContain('ask-card');
		expect(html).toMatch(/<details class="ask-receipt"(?![^>]*\sopen)/);
		expect(summary(html)).toBe("Asked · What's the most important thing to fix this month? → Marketing");
	});

	it('tapping the receipt shows the question and its options read-only, the chosen one marked', () => {
		const html = ask('Marketing');
		const body = html.slice(html.indexOf('</summary>'));
		expect(body).toContain('<strong>most important</strong>');
		expect(body).toMatch(/ask-receipt-option ask-receipt-chosen[^>]*>[\s\S]*?Marketing/);
		expect(body).not.toMatch(/ask-receipt-chosen[^>]*>[\s\S]{0,200}?Sales/);
		expect(body).toContain('Hiring');
		expect(body).not.toMatch(/<button|<input/);
	});

	it('a typed "Other" answer is shown and marked', () => {
		const html = ask('Run a spring promo');
		expect(summary(html)).toContain('→ Run a spring promo');
		expect(html).toMatch(/ask-receipt-chosen[^>]*>[\s\S]*?Run a spring promo/);
	});

	it('a long list asked as a select is a dropdown of the same options; the receipt is the same', () => {
		const select = [{ type: 'options' as const, style: 'select' as const, options: ['Acme', 'Globex', 'Initech', 'Hooli', 'Stark', 'Wayne'] }];
		const open = render(AskWidget, { props: { requestId: 'r1', prompt: 'Which brand?', widgets: select, onSubmit: () => {} } }).body;
		expect(open).toContain('ask-select');
		expect(open).toMatch(/<option[^>]*value=""[^>]*>Choose one…<\/option>/);
		expect(open).toMatch(/<option[^>]*value="Hooli"/);
		expect(open).not.toContain('class="decision-choice');
		const done = render(AskWidget, { props: { requestId: 'r1', prompt: 'Which brand?', widgets: select, response: 'Hooli', onSubmit: () => {} } }).body;
		expect(summary(done)).toBe('Asked · Which brand? → Hooli');
		// Without `style`, the same options are buttons, as before.
		expect(ask()).toContain('class="decision-choice');
		expect(ask()).not.toContain('ask-select');
	});

	// 1b (owner, 10-24): one decision card everywhere. A question's choices
	// are the card's full-width stacked buttons: a bold label with its
	// one-line description, the recommended (first) one filled, the rest
	// outlined, Other last. A single pick on a card of one question answers
	// it at once, so there is no Submit.
	it('an option is a full-width button, a bold label with its description, the first filled, Other last', () => {
		const html = ask();
		const labels = [...html.matchAll(/<span class="decision-choice-label"[^>]*>([\s\S]*?)<\/span>/g)].map((m) => text(m[1]));
		expect(labels).toEqual(['Marketing', 'Sales', 'Hiring', 'Other']);
		expect(html).toMatch(/<span class="decision-choice-label"[^>]*>[\s\S]*?Hiring[\s\S]*?<span class="decision-choice-description"[^>]*>Two roles open<\/span>/);
		const tones = [...html.matchAll(/class="decision-choice ([^"]*)"/g)].map((m) => m[1].trim());
		expect(tones).toEqual(['decision-choice-filled', 'decision-choice-outlined', 'decision-choice-outlined', 'decision-choice-outlined']);
		expect(html).not.toMatch(/>Submit<\/button>/);
		// The recommended one comes first, with its tag.
		const recommended = [{ type: 'options' as const, options: ['Sales', { label: 'Hiring', recommended: true }] }];
		const rec = render(AskWidget, { props: { requestId: 'r1', prompt: question, widgets: recommended, onSubmit: () => {} } }).body;
		const recLabels = [...rec.matchAll(/<span class="decision-choice-label"[^>]*>([\s\S]*?)<\/span>/g)].map((m) => text(m[1]));
		expect(recLabels[0]).toContain('Hiring');
		expect(rec).toContain('decision-choice-badge');
		// Several picks: toggles, then Submit.
		const multi = [{ type: 'options' as const, multiSelect: true, options: ['Sales', 'Hiring'] }];
		const many = render(AskWidget, { props: { requestId: 'r1', prompt: question, widgets: multi, onSubmit: () => {} } }).body;
		expect(many).toMatch(/role="checkbox"/);
		expect(many).toMatch(/>Submit<\/button>/);
		// The styles are in app.css: stacked full-width buttons, the label
		// bold, the description smaller, a picked state.
		const css = readFileSync('src/app.css', 'utf8');
		const rule = (sel: string) => css.match(new RegExp(`\\n${sel.replace(/[.]/g, '\\.')} \\{([^}]*)\\}`))?.[1] ?? '';
		expect(rule('.decision-choices')).toContain('flex-col');
		expect(rule('.decision-choice')).toContain('w-full');
		expect(rule('.decision-choice-filled')).toContain('btn-primary');
		expect(rule('.decision-choice-label')).toContain('font-semibold');
		expect(rule('.decision-choice-description')).toContain('text-xs');
		expect(rule('.decision-choice-on')).toContain('--color-primary');
	});

	it('an open question has its asker on the card, and Stop alone in the corner when its work runs', () => {
		const html = render(AskWidget, { props: { requestId: 'r1', prompt: question, widgets, onSubmit: () => {}, asker: { id: 'a1', name: 'Bookkeeper' }, onstop: () => {} } }).body;
		expect(html).toMatch(/class="decision-head"[\s\S]*?<svg|class="decision-head"[\s\S]*?Bookkeeper/);
		expect(html).toMatch(/class="decision-stop"[^>]*>Stop<\/button>/);
		expect(html.indexOf('decision-stop')).toBeLessThan(html.indexOf('decision-choices'));
		expect(ask()).not.toContain('decision-stop');
	});

	it('an install or a sign-in stays in the chat on the same card: its icon, its name bold, the button, Skip', () => {
		const install = [{ type: 'install_plugin' as const, name: 'Shopify', code: 'PLUG-1', description: 'Your store, orders and products' }];
		const html = render(AskWidget, { props: { requestId: 'i1', prompt: 'Install Shopify?', widgets: install, onSubmit: () => {} } }).body;
		expect(html).toContain('ask-card decision-card');
		expect(html).toContain('decision-icon');
		expect(html.replace(/<!--[^>]*-->/g, '')).toMatch(/<b>Shopify<\/b>/);
		const labels = [...html.matchAll(/<span class="decision-choice-label"[^>]*>([\s\S]*?)<\/span>/g)].map((m) => text(m[1]));
		expect(labels).toEqual(['Install', 'Skip']);
		const connect = [{ type: 'connect_account' as const, plugin: 'gmail', label: 'Gmail', agentId: 'a1' }];
		const signIn = render(AskWidget, { props: { requestId: 'c1', prompt: 'Sign in', widgets: connect, onSubmit: () => {} } }).body;
		expect(signIn).toContain('decision-icon');
		expect([...signIn.matchAll(/<span class="decision-choice-label"[^>]*>([\s\S]*?)<\/span>/g)].map((m) => text(m[1]))).toEqual(['Sign in', 'Skip']);
	});

	it('several questions are one card of steps by their headers, the first open, Next to go on', () => {
		const steps = [
			{ type: 'options' as const, header: 'Length', question: 'How long?', options: [{ label: '30 seconds', description: 'Tight' }, '60 seconds'] },
			{ type: 'options' as const, header: 'Music', question: 'Which music?', options: ['Upbeat', 'Calm'], multiSelect: true },
			{ type: 'options' as const, header: 'Title', question: 'What title?' }
		];
		const html = render(AskWidget, { props: { requestId: 'w1', prompt: 'How long?', widgets: steps, onSubmit: () => {} } }).body;
		expect(text(html)).toContain('LengthMusicTitle');
		expect(html).toMatch(/class="ask-step ask-step-current"[^>]*>Length/);
		expect(text(html)).toContain('How long?');
		expect(text(html)).not.toContain('Which music?');
		expect(html).toMatch(/>Next<\/button>/);
		expect(html).not.toMatch(/>Back<\/button>/);
		// Answered: one record, each answer under its header.
		const done = render(AskWidget, { props: { requestId: 'w1', prompt: 'How long?', widgets: steps, response: JSON.stringify(['60 seconds', 'Upbeat, Calm', 'Spring launch']), onSubmit: () => {} } }).body;
		expect(summary(done)).toBe('Asked · Length, Music, Title → 60 seconds · Upbeat, Calm · Spring launch');
		expect(done).not.toContain('ask-card');
	});

	it('skipped and cancelled questions read as such', () => {
		expect(summary(ask(SKIP_VALUE))).toBe("Skipped · What's the most important thing to fix this month?");
		expect(summary(ask(undefined, { cancelled: true }))).toBe("Cancelled · What's the most important thing to fix this month?");
	});

	it('an open permission ask is the same tinted card; settled, the same receipt', () => {
		const open = render(PermissionAskCard, { props: { ask: permission, via: 'chat' } }).body;
		expect(open).toContain('ask-card decision-card');
		expect(open).not.toContain('ask-receipt');
		// One order everywhere: Allow once, Always allow, Decline.
		const labels = [...open.matchAll(/<span class="decision-choice-label"[^>]*>([\s\S]*?)<\/span>/g)].map((m) => text(m[1]));
		expect(labels).toEqual(['Allow once', 'Always allow', 'Decline']);
		const tones = [...open.matchAll(/class="decision-choice ([^"]*)"/g)].map((m) => m[1].trim());
		expect(tones).toEqual(['decision-choice-filled', 'decision-choice-filled', 'decision-choice-outlined']);
		// Its employee's name is the bold word of the line.
		expect(open.replace(/<!--[^>]*-->/g, '')).toMatch(/<b>Bookkeeper<\/b> wants your OK/);
		// A critical target never offers Always allow.
		const critical = render(PermissionAskCard, { props: { ask: { ...permission, allowAlways: false }, via: 'chat' } }).body;
		expect([...critical.matchAll(/<span class="decision-choice-label"[^>]*>([\s\S]*?)<\/span>/g)].map((m) => text(m[1]))).toEqual(['Allow once', 'Decline']);

		for (const [status, answer, lead] of [
			['allowed', 'allow_always', 'Allowed · always'],
			['allowed', 'this_once', 'Allowed · this once'],
			['declined', 'no', 'Declined']
		]) {
			const html = render(PermissionAskCard, { props: { ask: { ...permission, status, answer }, via: 'chat' } }).body;
			expect(html).not.toContain('ask-card');
			expect(summary(html)).toBe(`${lead} · Send the invoice to the client`);
			expect(html.slice(html.indexOf('</summary>'))).not.toContain('<button');
		}
	});

	// Live 2026-10-15: the card read "Bookkeeper wants your OK · Using message.
	// It would act in your apps as you." with no reason. It says what, why,
	// what about it needs the owner, previews what would go out under "Show
	// details", and says what Allow always covers; never a tool's name.
	it('a permission ask says what, why, the details and what Allow always covers', () => {
		const card = {
			...permission,
			sentence: 'texting +13855550100',
			what: 'Texting +13855550100',
			why: 'Workflow "Month-end close", activity "Queue review": Find the questions waiting on you.',
			statedReason: 'Dana asked for the total by text.',
			details: [
				{ kind: 'to', value: '+13855550100' },
				{ kind: 'body', value: 'Your invoice is ready.\nTotal: $420' }
			],
			reason: 'It would use your Messages app as you.',
			allowAlwaysCovers: 'Always let Bookkeeper send texts'
		};
		const html = render(PermissionAskCard, { props: { ask: card, via: 'chat' } }).body;
		const shown = text(html);
		expect(shown).toContain('Bookkeeper wants your OK');
		expect(shown).toContain('Texting +13855550100');
		expect(shown).toContain('Why: Workflow "Month-end close", activity "Queue review"');
		expect(shown).toContain('Bookkeeper says: Dana asked for the total by text.');
		expect(shown).toContain('It would use your Messages app as you.');
		expect(summary(html)).toBe('Show details');
		expect(shown).toContain('To +13855550100');
		expect(shown).toContain('Message Your invoice is ready.');
		expect(shown).toContain('Always let Bookkeeper send texts');
		// What "always" covers is the note right under Always allow.
		expect(html).toMatch(/Always allow[\s\S]*?<\/button>\s*(<!--[^>]*-->\s*)*<p class="decision-choice-note"[^>]*>Always let Bookkeeper send texts<\/p>/);
		expect(shown).not.toMatch(/\bmessage\b|Using/);

		// Nothing to say leaves the lines out: no empty "Why", no details fold.
		const plain = text(render(PermissionAskCard, { props: { ask: permission, via: 'chat' } }).body);
		expect(plain).toContain('Send the invoice to the client');
		expect(plain).not.toMatch(/Why:|Show details/);
	});

	// The owner reads why it asked in his own language: the bot sends the
	// line's key and values, and the card words it from the translations;
	// a bot that sends none still shows its English line.
	it('a permission ask words why it asked from the translations', () => {
		const keyed = {
			...permission,
			reason: 'English from the bot',
			reasonKey: 'permissionAsk.reason.reachesApp',
			reasonValues: { app: 'Messages' }
		};
		const shown = text(render(PermissionAskCard, { props: { ask: keyed, via: 'chat' } }).body);
		expect(shown).toContain('It would use your Messages app as you.');
		expect(shown).not.toContain('English from the bot');

		const removal = {
			...permission,
			reasonKey: 'permissionAsk.reason.removesFolders',
			warningKey: 'permissionAsk.warning.recursivelyForceRemovesFiles'
		};
		// The reason sits under the header; the command's one-line risk is a
		// row in the folded details.
		const removed = render(PermissionAskCard, { props: { ask: { ...removal, command: 'rm -rf ~/old' }, via: 'chat' } }).body;
		expect(removed).toMatch(/class="decision-reason"[^>]*>Removing a folder outside its work needs your OK\.<\/p>/);
		const rows = text(removed.slice(removed.indexOf('decision-rows')));
		expect(rows).toContain('Command rm -rf ~/old');
		expect(rows).toContain('Risk It may remove whole folders of files, without asking.');

		const unknown = { ...permission, reason: 'Its own words.', reasonKey: 'permissionAsk.reason.noSuchCase' };
		expect(text(render(PermissionAskCard, { props: { ask: unknown, via: 'chat' } }).body)).toContain('Its own words.');
	});

	it('an approval matches: tinted while open, a receipt once decided', () => {
		const approval = { requestId: 'g1', sessionId: 's', agent: 'Bookkeeper', actionType: 'suggest_goal', actionDetail: '', headline: 'Agree on this goal?' };
		const open = render(ApprovalAskCard, { props: { approval } }).body;
		expect(open).toContain('ask-card decision-card');
		expect([...open.matchAll(/<span class="decision-choice-label"[^>]*>([\s\S]*?)<\/span>/g)].map((m) => text(m[1]))).toEqual(['Allow once', 'Always allow', 'Decline']);
		expect(open).toMatch(/class="decision-action"[^>]*>Agree on this goal\?<\/p>/);
		const decided = render(ApprovalAskCard, { props: { approval: { ...approval, decision: 'once' } } }).body;
		expect(decided).not.toContain('ask-card');
		expect(summary(decided)).toBe('Allowed · this once · Agree on this goal?');
	});

	it('the tint is the theme primary, never red or amber, and a quieter receipt has no chrome', () => {
		const css = readFileSync('src/app.css', 'utf8');
		const rule = (sel: string) => css.match(new RegExp(`(?:^|\\n)${sel.replace(/[.[\]"=]/g, '\\$&')}\\s*\\{([^}]*)\\}`))?.[1] ?? '';
		const card = rule('.ask-card');
		expect(card).toContain('color-mix(in srgb, var(--color-primary) 7%, var(--color-base-100))');
		expect(card).not.toMatch(/error|warning|#/);
		expect(rule('.ask-receipt')).not.toMatch(/bg-|border|shadow/);
	});
});

// 2026-10-19: "Want this waiting for you every morning?" sat under a
// collapsed tool row holding the brief it meant. What the owner decides on
// is on the card, above the question; the question stays the question.
describe('an ask carries what it is about', () => {
	const brief = 'Two meetings today. **Dana** is waiting on the lease.';
	const withContext = [{ type: 'options' as const, context: brief, options: ['Every weekday at 7:30 AM', 'Not now'] }];
	const card = (response?: string) =>
		render(AskWidget, { props: { requestId: 'r2', prompt: 'Want this waiting for you every morning?', widgets: withContext, response, onSubmit: () => {} } }).body;

	it('shows the brief on the open card, above the question', () => {
		const html = card();
		expect(html).toContain('ask-context');
		expect(html).toContain('<strong>Dana</strong>');
		expect(html.indexOf('ask-context')).toBeLessThan(html.indexOf('Want this waiting'));
		expect(text(html).split('Want this waiting for you every morning?').length - 1).toBe(1);
	});

	it('keeps it in the answered receipt, folded with the question', () => {
		const html = card('Not now');
		expect(summary(html)).toContain('Want this waiting for you every morning?');
		expect(summary(html)).not.toContain('Dana');
		expect(html).toContain('<strong>Dana</strong>');
	});

	it('shows no context box for a question that stands alone', () => {
		expect(ask()).not.toContain('ask-context');
	});
});
