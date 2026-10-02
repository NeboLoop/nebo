// Ask cards stand off the chat while they wait, and fold to a receipt once
// answered.
//
// The owner: the question card was white on the white chat with a faint
// border ("make it a different color so it stands off the background"), and
// an answered card stayed a full card with the question and a blue pill.
// Every OPEN ask (a question, a permission ask, an approval) is now one
// tinted `.ask-card`; an answered one collapses in place to a muted
// one-line receipt with no card chrome, and tapping it opens the question
// read-only (a native <details>, folded until tapped).
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
	reason: 'It goes outside the company.',
	allowAlways: true,
	thisOnce: true,
	status: 'open',
	chatId: 'c',
	blocking: true,
	createdAt: 0
};

describe('ask cards', () => {
	it('an open question is a tinted ask card, not a receipt', () => {
		const html = ask(undefined, { disabled: false });
		expect(html).toContain('class="ask-card"');
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

	it('skipped and cancelled questions read as such', () => {
		expect(summary(ask(SKIP_VALUE))).toBe("Skipped · What's the most important thing to fix this month?");
		expect(summary(ask(undefined, { cancelled: true }))).toBe("Cancelled · What's the most important thing to fix this month?");
	});

	it('an open permission ask is the same tinted card; settled, the same receipt', () => {
		const open = render(PermissionAskCard, { props: { ask: permission, via: 'chat' } }).body;
		expect(open).toContain('ask-card permission-ask-card');
		expect(open).not.toContain('ask-receipt');

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

	it('an approval matches: tinted while open, a receipt once decided', () => {
		const approval = { requestId: 'g1', sessionId: 's', agent: 'Bookkeeper', actionType: 'suggest_goal', actionDetail: '', headline: 'Agree on this goal?' };
		expect(render(ApprovalAskCard, { props: { approval } }).body).toContain('ask-card permission-ask-card');
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
