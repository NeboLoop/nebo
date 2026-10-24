// The one decision card's words and answers: the same order everywhere, the
// key words bold, the details as label and value rows.
import { describe, it, expect, beforeAll } from 'vitest';
import { addMessages, init, locale, t } from 'svelte-i18n';
import { get } from 'svelte/store';
import en from '$lib/i18n/locales/en.json';
import he from '$lib/i18n/locales/he.json';
import { approvalAction, approvalChoices, approvalRows, emphasized, permissionChoices, permissionRows, questionOrder } from './decision';

beforeAll(() => {
	addMessages('en', en);
	addMessages('he', he);
	init({ fallbackLocale: 'en', initialLocale: 'en' });
});

const tr = (key: string, opts?: Record<string, unknown>) => get(t)(key, opts as never);

const ask = {
	id: 'p1',
	kind: 'permission',
	agentId: 'a1',
	employee: 'Bookkeeper',
	sessionKey: 's',
	sentence: 'searching your texts',
	what: 'Searching your texts',
	why: '',
	details: [{ kind: 'search', value: 'invoice' }],
	reason: 'r',
	reasonKey: '',
	reasonValues: {},
	command: 'grep -r invoice ~/Messages',
	warningKey: 'permissionAsk.warning.forceRemovesFiles',
	allowAlways: true,
	allowAlwaysCovers: 'Always let Bookkeeper search your texts',
	thisOnce: true,
	status: 'open',
	chatId: 'c',
	blocking: true,
	createdAt: 0
};

describe('the decision card', () => {
	it('bolds only the key words of its line', () => {
		expect(emphasized(tr, 'permissionAsk.title', { name: 'Bookkeeper' })).toEqual([
			{ text: 'Bookkeeper', bold: true },
			{ text: ' wants your OK', bold: false }
		]);
	});

	it('a permission ask answers Allow once, Always allow with what it covers, Decline', () => {
		const choices = permissionChoices(ask, tr);
		expect(choices.map((c) => [c.id, c.label, c.tone])).toEqual([
			['this_once', 'Allow once', 'filled'],
			['allow_always', 'Always allow', 'filled'],
			['no', 'Decline', 'outlined']
		]);
		expect(choices[1].note).toBe('Always let Bookkeeper search your texts');
		// A critical target never offers Always allow.
		expect(permissionChoices({ ...ask, allowAlways: false }, tr).map((c) => c.id)).toEqual(['this_once', 'no']);
		// A send check asks whether it went out.
		expect(permissionChoices({ ...ask, kind: 'send_check' }, tr).map((c) => c.id)).toEqual(['sent', 'not_sent']);
	});

	it("folds a permission ask's details, its command and its risk into rows", () => {
		expect(permissionRows(ask, tr)).toEqual([
			{ label: 'Looking for', value: 'invoice', code: false },
			{ label: 'Command', value: 'grep -r invoice ~/Messages', code: true },
			{ label: 'Risk', value: 'It may remove files without asking.' }
		]);
	});

	it('an approval answers in the same order and folds the call into its rows', () => {
		expect(approvalChoices(tr).map((c) => [c.id, c.label, c.tone])).toEqual([
			['once', 'Allow once', 'filled'],
			['always', 'Always allow', 'filled'],
			['deny', 'Decline', 'outlined']
		]);
		const shell = {
			requestId: 'r',
			sessionId: 'agent:a1:main',
			agent: 'Bookkeeper',
			actionType: 'shell_command',
			actionDetail: 'ls ~'
		};
		expect(approvalAction(shell, tr)).toBe('Run Shell Command');
		expect(approvalRows(shell, tr)).toEqual([{ label: 'Technical details', value: 'ls ~', code: true }]);
		expect(approvalAction({ ...shell, headline: 'Send the invoice to Dana' }, tr)).toBe('Send the invoice to Dana');
	});

	it('puts the recommended choice first', () => {
		expect(questionOrder([{ label: 'A' }, { label: 'B', recommended: true }]).map((o) => o.label)).toEqual(['B', 'A']);
	});

	it('reads in a right-to-left language too', () => {
		locale.set('he');
		const choices = permissionChoices(ask, tr);
		expect(choices.map((c) => c.label)).toEqual([he.decision.allowOnce, he.decision.alwaysAllow, he.decision.decline]);
		expect(emphasized(tr, 'permissionAsk.title', { name: 'Bookkeeper' }).filter((s) => s.bold)).toEqual([{ text: 'Bookkeeper', bold: true }]);
		locale.set('en');
	});
});
