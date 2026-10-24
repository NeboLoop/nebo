// The one decision card's words and answers: the same order everywhere, the
// key words bold, the details as label and value rows.
import { describe, it, expect, beforeAll } from 'vitest';
import { addMessages, init, locale, t } from 'svelte-i18n';
import { get } from 'svelte/store';
import en from '$lib/i18n/locales/en.json';
import he from '$lib/i18n/locales/he.json';
import { approvalAction, approvalChoices, approvalReason, approvalRows, emphasized, permissionChoices, permissionRows, questionOrder, wantsLine } from './decision';

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
	action: '',
	actionValues: {},
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

	// The coordinator, 10-24: one sentence, only the key words bold,
	// "Bookkeeper wants to send a text to Dana", in the owner's language
	// by the server's key, its English when it sends none.
	it('says one sentence: who wants to do what, the key words bold', () => {
		const intent = { action: 'send a text to +13855550100', actionKey: 'decision.wants.textTo', actionValues: { phone: '+13855550100' } };
		expect(wantsLine(tr, 'Bookkeeper', intent)).toEqual([
			{ text: 'Bookkeeper', bold: true },
			{ text: ' wants to ', bold: false },
			{ text: 'send a text to +13855550100', bold: true }
		]);
		// The model's own line: its words, no key.
		expect(wantsLine(tr, 'Bookkeeper', { action: 'send invoice 1041 to ACME' })?.[2].text).toBe('send invoice 1041 to ACME');
		// No action: no line of its own.
		expect(wantsLine(tr, 'Bookkeeper', { action: '' })).toBeNull();
		locale.set('de');
		addMessages('de', { decision: { wantsTo: '{name} möchte {action}', wants: { textTo: 'eine SMS an {phone} senden' } } });
		expect(wantsLine(tr, 'Bookkeeper', intent)?.map((s) => s.text).join('')).toBe('Bookkeeper möchte eine SMS an +13855550100 senden');
		locale.set('en');
	});

	it('a suggested goal never offers Always allow, and says why it asks', () => {
		expect(approvalChoices(tr, false).map((c) => c.id)).toEqual(['once', 'deny']);
		const goal = { requestId: 'g', sessionId: 's', agent: 'Chief', actionType: 'goal', actionDetail: 'tests pass', reasonKey: 'permissionAsk.reason.agreesGoal', reason: 'x' };
		expect(approvalReason(goal, tr)).toBe("Work continues until a check confirms it's met.");
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
