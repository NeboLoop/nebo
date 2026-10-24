// The ONE decision card's words and answers (DecisionCard): a question, a
// permission ask and an approval read the same way everywhere a decision is
// asked. A centered avatar or icon, one line with only its key words bold,
// the reason under it, the details folded behind "Show details" as label
// and value rows, and the answers as full-width stacked buttons in one
// order: Allow once (filled), Always allow (filled, with what "always"
// covers), Decline (outlined). A question keeps its own choices in the
// same style, the recommended one first and Other last.

import type { PermissionAskCard } from '$lib/api/neboComponents';
import type { Approval } from '$lib/stores/approvals';
import type { WizardOption } from './askWizard';

type Tr = (key: string, opts?: { values?: Record<string, string | number>; default?: string }) => string;

/** A piece of the card's line: its key words are bold. */
export interface Segment {
	text: string;
	bold: boolean;
}

/** One answer on the card, as a full-width button. `id` is what it sends. */
export interface DecisionChoice {
	id: string;
	label: string;
	/** One line under the label: what the choice means. */
	description?: string;
	tone: 'filled' | 'outlined';
	/** A small note under the button (what "Always allow" covers). */
	note?: string;
	/** Picked, on a card that picks before it sends. */
	on?: boolean;
	/** A small tag beside the label ("Recommended"). */
	badge?: string;
	/** Not available yet (a sign-in with fields left to fill). */
	disabled?: boolean;
	/** Working on it: a spinner beside the label. */
	working?: boolean;
}

/** One row under "Show details". */
export interface DecisionRow {
	label: string;
	value: string;
	/** A command or a path: shown as code. */
	code?: boolean;
}

const MARK = (i: number) => `\u0001${i}\u0002`;

/** Message `key` with its values bold: the line's key words. */
export function emphasized(tr: Tr, key: string, values: Record<string, string>): Segment[] {
	const names = Object.keys(values);
	const marked = tr(key, {
		values: Object.fromEntries(names.map((n, i) => [n, MARK(i)]))
	});
	const out: Segment[] = [];
	const re = /\u0001(\d+)\u0002/g;
	let last = 0;
	for (let m = re.exec(marked); m; m = re.exec(marked)) {
		if (m.index > last) out.push({ text: marked.slice(last, m.index), bold: false });
		out.push({ text: values[names[Number(m[1])]] ?? '', bold: true });
		last = m.index + m[0].length;
	}
	if (last < marked.length) out.push({ text: marked.slice(last), bold: false });
	return out.filter((s) => s.text !== '');
}

/** A permission ask's answers, in the one order. A critical target (no
 *  Always allow on the card) never offers it; a send check asks whether the
 *  send went out instead. */
export function permissionChoices(ask: PermissionAskCard, tr: Tr): DecisionChoice[] {
	if (ask.kind === 'send_check') {
		return [
			{ id: 'sent', label: tr('permissionAsk.sent'), tone: 'filled' },
			{ id: 'not_sent', label: tr('permissionAsk.notSent'), tone: 'outlined' }
		];
	}
	const out: DecisionChoice[] = [];
	if (ask.thisOnce)
		out.push({
			id: 'this_once',
			label: tr('decision.allowOnce'),
			tone: 'filled'
		});
	if (ask.allowAlways)
		out.push({
			id: 'allow_always',
			label: tr('decision.alwaysAllow'),
			tone: 'filled',
			note: ask.allowAlwaysCovers || undefined
		});
	out.push({ id: 'no', label: tr('decision.decline'), tone: 'outlined' });
	return out;
}

/** A permission ask's folded details: what the call would send or change,
 *  the command it would run, and its one-line risk. */
export function permissionRows(ask: PermissionAskCard, tr: Tr): DecisionRow[] {
	const rows: DecisionRow[] = ask.details.map((d) => ({
		label: tr(`permissionAsk.detail.${d.kind}`, { default: d.kind }),
		value: d.value,
		code: d.kind === 'command'
	}));
	if (ask.command && !ask.details.some((d) => d.kind === 'command')) {
		rows.push({
			label: tr('permissionAsk.detail.command'),
			value: ask.command,
			code: true
		});
	}
	if (ask.warningKey) rows.push({ label: tr('decision.risk'), value: tr(ask.warningKey) });
	return rows;
}

/** What about a permission ask needs the owner, in his language: the bot's
 *  key and values, its English line when it sends none (an older bot). */
export function permissionReason(ask: PermissionAskCard, tr: Tr): string {
	return ask.reasonKey ? tr(ask.reasonKey, { values: ask.reasonValues, default: ask.reason }) : ask.reason;
}

/** An approval's answers, in the one order. */
export function approvalChoices(tr: Tr): DecisionChoice[] {
	return [
		{ id: 'once', label: tr('decision.allowOnce'), tone: 'filled' },
		{ id: 'always', label: tr('decision.alwaysAllow'), tone: 'filled' },
		{ id: 'deny', label: tr('decision.decline'), tone: 'outlined' }
	];
}

/** The words for a gated call that carries no sentence of its own. */
const ACTION_KEYS: Record<string, string> = {
	shell_command: 'components.approvalModal.runShellCommand',
	file_write: 'components.approvalModal.writeFile',
	http_request: 'components.approvalModal.httpRequest'
};

/** What an approval would do, as the card's bold action line. */
export function approvalAction(approval: Approval, tr: Tr): string {
	if (approval.headline) return approval.headline;
	const key = ACTION_KEYS[approval.actionType];
	return key ? tr(key) : '';
}

/** An approval's folded details: its fact rows, and the call itself. */
export function approvalRows(approval: Approval, tr: Tr): DecisionRow[] {
	const rows: DecisionRow[] = (approval.detailRows ?? []).map((r) => ({
		label: r.label,
		value: r.value
	}));
	if (approval.actionDetail) {
		rows.push({
			label: tr('components.approvalModal.technicalDetails'),
			value: approval.actionDetail,
			code: true
		});
	}
	return rows;
}

/** A question's choices, the recommended one first, each once. */
export function questionOrder(options: WizardOption[]): WizardOption[] {
	return [...options.filter((o) => o.recommended), ...options.filter((o) => !o.recommended)];
}

/** The employee an approval's session is: `agent:<id>:…`. */
export function approvalAgentId(approval: Approval): string {
	return /^agent:([^:]+):/.exec(approval.sessionId)?.[1] ?? '';
}
