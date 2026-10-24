// What takes the composer's place in the chat on screen: the questions and
// approvals waiting there, oldest first, one at a time. A question card
// (choices, or several questions), a permission ask, an approval. Cards done
// in the app (an install, a sign-in, a hire) and open questions stay in the
// conversation; the composer answers an open question.

import type { AskWidgetDef } from '$lib/components/chat/AskWidget.svelte';
import type { PermissionAskCard } from '$lib/api/neboComponents';
import type { Approval } from '$lib/stores/approvals';
import { takesOverComposer } from './ask';

export type ComposerAsk =
	| { kind: 'question'; id: string; prompt: string; widgets: AskWidgetDef[] }
	| { kind: 'permission'; id: string; ask: PermissionAskCard }
	| { kind: 'approval'; id: string; approval: Approval };

interface AskMsg {
	type: string;
	requestId?: string;
	prompt?: string;
	widgets?: AskWidgetDef[];
	response?: string;
	cancelled?: boolean;
}

/** The asks waiting in this chat that take the composer's place. A
 *  question waits only while its run is going (`running`); a permission ask
 *  or an approval waits until it is answered anywhere. */
export function composerAsksOf(
	messages: AskMsg[],
	permissionAsks: PermissionAskCard[],
	approvals: Approval[],
	running: boolean
): ComposerAsk[] {
	const questions: ComposerAsk[] = running
		? messages
				.filter((m) => m.type === 'ask' && m.requestId && m.response == null && !m.cancelled && takesOverComposer(m.widgets))
				.map((m) => ({ kind: 'question', id: m.requestId!, prompt: m.prompt ?? '', widgets: m.widgets! }))
		: [];
	return [
		...questions,
		...permissionAsks.filter((a) => a.status === 'open').map((ask): ComposerAsk => ({ kind: 'permission', id: ask.id, ask })),
		...approvals.filter((a) => !a.decision).map((approval): ComposerAsk => ({ kind: 'approval', id: approval.requestId, approval }))
	];
}

/** The asks still shown in place of the composer: all of them, unless the
 *  owner closed every one; a new one comes back up. */
export function shownComposerAsks(asks: ComposerAsk[], closed: ReadonlySet<string>): ComposerAsk[] {
	return asks.every((a) => closed.has(a.id)) ? [] : asks;
}
