// A blocking ask: an employee's run is parked on the owner's answer. It must
// reach him wherever he is in the app — the chief of staff's chat, another
// employee's, a voice call — and he must be able to answer it without
// leaving that chat. So it is never put into the chat he is in (live
// 2026-10-02: Bookkeeper's workflow ask landed in Flip-Flap's chat and
// Flip-Flap answered it). Each new one raises a transient toast and the OS
// notification; a click on either opens its card over the current screen
// (<AskPopover/>), never a navigation. "See conversation" there is the only
// way it moves him. The answer goes through the ask's one answer path, and
// the first answer anywhere settles it everywhere (`asks_waiting`).

import { writable, get } from 'svelte/store';
import type { WaitingAsk } from '$lib/api/neboComponents';
import { waitingAsks, answerWaiting, askPath } from './waitingAsks';

/** The ask whose card is open over the current screen, by id. Set only by
 *  the owner's click on its toast or notification. */
export const openAskCard = writable<string | null>(null);

/** Blocking asks already told of, by id: each is told once. */
const told = new Set<string>();

/** The blocking asks in `asks` not told of yet; they are told of now. */
export function newBlockingAsks(asks: WaitingAsk[]): WaitingAsk[] {
  const fresh = asks.filter((a) => a.blocking && !told.has(a.id));
  for (const a of fresh) told.add(a.id);
  return fresh;
}

/** Whether `pathname` is the chat ask waits in: its card is already on that
 *  screen, so no toast for it (live 2026-10-08: the bar popped up under the
 *  card the owner was looking at). */
export function inAsksChat(ask: WaitingAsk, pathname: string): boolean {
  if (!ask.chatId) return false;
  const home = askPath(ask);
  return pathname === home || pathname.startsWith(home + '/');
}

/** What is already waiting when the app starts is its pinned bar's and its
 *  Inbox's: no burst of notices for it. */
export function seedBlockingAsks(asks: WaitingAsk[]): void {
  for (const a of asks) if (a.blocking) told.add(a.id);
}

/** Open the card for ask `id` over the current screen. */
export function openAsk(id: string): void {
  openAskCard.set(id);
}

export function closeAsk(): void {
  openAskCard.set(null);
}

/** Where "See conversation" goes: the conversation the ask waits in, or —
 *  for a permission ask no chat raised (a schedule, a workflow) — its item
 *  in the Inbox. */
export function askHome(ask: WaitingAsk): string {
  if (!ask.chatId && ask.kind === 'permission') {
    return `/inbox?m=${encodeURIComponent(`permission-ask:${ask.id}`)}`;
  }
  return askPath(ask);
}

/** Answer the waiting ask `id` with option `i`, from wherever the owner is
 *  (the card, a notification's action button). Nothing when it was settled
 *  meanwhile. */
export async function answerById(id: string, i: number): Promise<void> {
  const ask = get(waitingAsks).find((a) => a.id === id);
  if (!ask) return;
  await answerWaiting(ask, i);
}
