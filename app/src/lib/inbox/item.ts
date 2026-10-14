// The ONE place the app decides what an Inbox item is and how it is
// answered. An item is the server's row (`Notification`): its `status`
// (`open` | `answered` | `archived` | `update`), its `options` and its
// `type` say everything; nothing here fetches a status of its own.
//
// Two kinds of item: a decision that needs the owner (`needsYou`), answered
// right on its card with its options, and an update (a title, a sentence, a
// link). Three are drawn specially: the assistant's morning brief (`brief`),
// its offer to handle a kind of item (`rule_offer`) and what it handled under
// a rule (`handled`). Every answer, from every card (the Inbox, the pinned
// bar, the assistant's chat card), goes through `answerItem`: the server's
// one answer endpoint, which hands it to the item's own path.

import type { ItemOption, WaitingAsk } from '$lib/api/neboComponents';
import { inDesktopApp } from '$lib/api/engineService';
import { upsertItem, type InboxItem } from '$lib/stores/notifications';

export type { InboxItem };

/** Where the owner answered: the surfaces the server records. */
export type AnswerVia = 'desktop' | 'web' | 'chat';

/** Where an item is shown in the Inbox. */
export type Section = 'brief' | 'needs_you' | 'updates' | 'history';

/** How a card is drawn. */
export type CardKind = 'brief' | 'offer' | 'handled' | 'needs_you' | 'update' | 'history';

/** The assistant: the owner's personal assistant, one roster entry. */
export const ASSISTANT_ID = 'assistant';

/** Whether an item's employee is the assistant (`primary` on older rows). */
export const isAssistant = (agentId?: string | null): boolean => agentId === ASSISTANT_ID || agentId === 'primary';

/** How many decisions show before "Show N more". */
export const TOP_NEEDS_YOU = 3;
/** How many employees' updates show before "<Name> and N more employees". */
export const TOP_GROUPS = 3;

const UNDO = 'undo';

export function sectionOf(n: InboxItem): Section {
	if (n.status === 'answered' || n.status === 'archived') return 'history';
	if (n.type === 'brief') return 'brief';
	return n.status === 'open' ? 'needs_you' : 'updates';
}

export function cardKindOf(n: InboxItem): CardKind {
	const section = sectionOf(n);
	if (section === 'brief' || section === 'history') return section;
	if (n.type === 'rule_offer') return 'offer';
	if (n.type === 'handled') return 'handled';
	return section === 'needs_you' ? 'needs_you' : 'update';
}

/** The answers on a decision's card (a handled item's Undo is its own button). */
export function answersOf(n: InboxItem): ItemOption[] {
	return (n.options ?? []).filter((o) => !(n.type === 'handled' && o.value === UNDO));
}

/** A handled item's Undo, when what was done can be undone. */
export function undoOf(n: InboxItem): ItemOption | null {
	return n.type === 'handled' ? (n.options ?? []).find((o) => o.value === UNDO) ?? null : null;
}

/** Items answered on their own screen (their link) have no answers on the card. */
export const answeredElsewhere = (n: InboxItem): boolean => sectionOf(n) === 'needs_you' && (n.options ?? []).length === 0;

/** Kinds whose answers are a fixed set: the owner's own words are not one. */
const FIXED_TYPES = new Set(['permission_ask', 'approval', 'rule_offer', 'handled']);
const FIXED_IDS = /^(permission-ask|approval|learn|artifact-update):/;

/** An employee's own question takes the owner's own words besides its options. */
export function takesOwnWords(n: InboxItem): boolean {
	return sectionOf(n) === 'needs_you' && (n.options ?? []).length > 0 && !FIXED_TYPES.has(n.type) && !FIXED_IDS.test(n.id);
}

/** The learned rule a handled item was done under (`handled:<ruleId>:<n>`). */
export function ruleIdOf(n: InboxItem): string | null {
	if (n.type !== 'handled') return null;
	const m = /^handled:(.+):[^:]+$/.exec(n.id);
	return m ? m[1] : null;
}

/** The staged proposal behind a `learn:` item (a proposal or a rule offer). */
export function learningIdOf(n: InboxItem): string | null {
	return n.id.startsWith('learn:') ? n.id.slice('learn:'.length) : null;
}

/** An approved proposal (a learned skill or a tuning) can still be undone
 *  from History: its restore point is written back the way it was applied. */
export function canRevertLearning(n: InboxItem): boolean {
	return n.type !== 'rule_offer' && learningIdOf(n) !== null && n.status === 'answered' && n.answer === 'Approved';
}

/** The Inbox item a waiting ask is: every permission ask has one; a question
 *  has one only when it was relayed (a question in the owner's own chat is
 *  that chat's card). */
export function waitingItemId(ask: WaitingAsk): string | null {
	if (ask.kind === 'permission') return `permission-ask:${ask.id}`;
	return ask.relayed ? `question-ask:${ask.id}` : null;
}

/** Decisions, newest first; the top few show, the rest behind "Show N more". */
export function topNeedsYou(items: InboxItem[], expanded = false, top = TOP_NEEDS_YOU): { shown: InboxItem[]; more: number } {
	const open = items.filter((n) => sectionOf(n) === 'needs_you').sort(newestFirst);
	return expanded ? { shown: open, more: 0 } : { shown: open.slice(0, top), more: Math.max(0, open.length - top) };
}

export interface UpdateGroup {
	agentId: string;
	items: InboxItem[];
	unread: number;
	/** Of these, how many the assistant handled under a rule. */
	handled: number;
}

/** Updates grouped by employee: each group newest first, the group with the
 *  newest update first. Items with no employee group together under ''. */
export function groupUpdates(items: InboxItem[]): UpdateGroup[] {
	const by = new Map<string, InboxItem[]>();
	for (const n of items) {
		if (sectionOf(n) !== 'updates') continue;
		const key = isAssistant(n.agentId) ? ASSISTANT_ID : n.agentId ?? '';
		by.set(key, [...(by.get(key) ?? []), n]);
	}
	return [...by.entries()]
		.map(([agentId, list]) => {
			const sorted = list.sort(newestFirst);
			return {
				agentId,
				items: sorted,
				unread: sorted.filter((n) => !n.readAt).length,
				handled: sorted.filter((n) => n.type === 'handled').length
			};
		})
		.sort((a, b) => b.items[0].createdAt - a.items[0].createdAt);
}

/** The first groups, and the tail collapsed behind "<Name> and N more employees". */
export function splitGroups(groups: UpdateGroup[], expanded = false, top = TOP_GROUPS): { shown: UpdateGroup[]; rest: UpdateGroup[] } {
	return expanded || groups.length <= top + 1
		? { shown: groups, rest: [] }
		: { shown: groups.slice(0, top), rest: groups.slice(top) };
}

const newestFirst = (a: InboxItem, b: InboxItem) => b.createdAt - a.createdAt;

/** The surface this app answers from. */
export function answerVia(): AnswerVia {
	return inDesktopApp() ? 'desktop' : 'web';
}

/** Answer a decision: the one answer path. The item comes back as it stands
 *  (settled by this answer, or by an earlier one from anywhere) and replaces
 *  the Inbox's copy. */
export async function answerItem(id: string, value: string, via: AnswerVia = answerVia()): Promise<InboxItem> {
	const { answerNotification } = await import('$lib/api/nebo');
	const res = await answerNotification(id, { value, via });
	upsertItem(res.notification);
	return res.notification;
}

/** The words for where an owner said yes ("on the mobile app"), an i18n key. */
export function surfaceKey(via?: string | null): string | null {
	switch (via) {
		case 'mobile':
			return 'inbox.viaMobile';
		case 'desktop':
			return 'inbox.viaDesktop';
		case 'web':
			return 'inbox.viaWeb';
		case 'chat':
			return 'inbox.viaChat';
		default:
			return null;
	}
}
