// The asks waiting on the owner: one step of an employee's work each, one
// card everywhere (the Inbox, the phone, the open chat). The REST list loads
// them once; `permission_ask` and `permission_ask_resolved` keep it current.
// The first answer anywhere wins, and the resolved event clears the rest.

import { writable } from 'svelte/store';
import type { PermissionAskCard } from '$lib/api/neboComponents';
import { askBandStatus, askNotificationId, setApprovalStatus } from './notifications';

/** A permission card's answers, or a held send's (did it go out?). */
export type AskAnswer = 'allow_always' | 'this_once' | 'no' | 'sent' | 'not_sent';

/** Open asks, oldest first. A chat shows the ones its own flow raised. */
export const openAsks = writable<PermissionAskCard[]>([]);

/** Asks settled while the app is open, as they were settled: a chat keeps
 *  each of its own in place as a one-line receipt. */
export const settledAsks = writable<PermissionAskCard[]>([]);

export async function loadOpenAsks(): Promise<void> {
  const { listPermissionAsks } = await import('$lib/api/nebo');
  const res = await listPermissionAsks();
  openAsks.set(res.asks ?? []);
}

/** What a chat shows: only the asks its own flow raised (`chatId`), never
 *  one from a schedule, a workflow, another employee's run or another chat
 *  (those live in the Inbox). Settled ones keep their place as a receipt;
 *  the open ones come last, at the bottom beside the composer. */
export function chatAsksOf(
  open: PermissionAskCard[],
  settled: PermissionAskCard[],
  chatId: string
): PermissionAskCard[] {
  if (!chatId) return [];
  const mine = (a: PermissionAskCard) => a.chatId === chatId;
  const openHere = open.filter(mine);
  return [...settled.filter((a) => mine(a) && !openHere.some((o) => o.id === a.id)), ...openHere];
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
  settledAsks.update((list) => [...list.filter((a) => a.id !== card.id), card]);
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
