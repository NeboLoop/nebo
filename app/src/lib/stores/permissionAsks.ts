// The asks waiting on the owner: one step of an employee's work each, one
// card everywhere (the Inbox, the phone, the open chat). The REST list loads
// them once; `permission_ask` and `permission_ask_resolved` keep it current.
// The first answer anywhere wins, and the resolved event clears the rest.

import { writable } from 'svelte/store';
import type { PermissionAskCard } from '$lib/api/neboComponents';
import { askBandStatus, askNotificationId, setApprovalStatus } from './notifications';

export type AskAnswer = 'allow_always' | 'this_once' | 'no';

/** Open asks, oldest first. The chat shows its own session's. */
export const openAsks = writable<PermissionAskCard[]>([]);

export async function loadOpenAsks(): Promise<void> {
  const { listPermissionAsks } = await import('$lib/api/nebo');
  const res = await listPermissionAsks();
  openAsks.set(res.asks ?? []);
}

/** A new ask was raised. */
export function askRaised(card: PermissionAskCard): void {
  openAsks.update((list) => (list.some((a) => a.id === card.id) ? list : [...list, card]));
  setApprovalStatus(askNotificationId(card.id), 'pending');
}

/** An ask was answered (here or anywhere else), or withdrawn because the
 *  work it was for ended. */
export function askSettled(card: PermissionAskCard): void {
  openAsks.update((list) => list.filter((a) => a.id !== card.id));
  setApprovalStatus(askNotificationId(card.id), askBandStatus(card.status));
}

/** Answer an ask. Returns the card as it was settled: when someone else
 *  answered first, theirs. */
export async function answerAsk(id: string, answer: AskAnswer, via: 'chat' | 'inbox'): Promise<PermissionAskCard> {
  const { answerPermissionAsk } = await import('$lib/api/nebo');
  const card = await answerPermissionAsk(id, { answer, via });
  askSettled(card);
  return card;
}
