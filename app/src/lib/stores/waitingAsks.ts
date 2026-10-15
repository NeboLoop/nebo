// Everything waiting on the owner's answer, from every employee: the
// questions a run is parked on and the permission asks. The pinned bar at the
// top of the chat reads it. `GET /asks` loads it once; `asks_waiting` carries
// the whole list again whenever it changes, so nothing polls.

import { writable } from 'svelte/store';
import type { WaitingAsk } from '$lib/api/neboComponents';
import { getWebSocketClient } from '$lib/websocket/client';
import { answerItem, waitingItemId } from '$lib/inbox/item';

/** Oldest first. */
export const waitingAsks = writable<WaitingAsk[]>([]);

export async function loadWaitingAsks(): Promise<WaitingAsk[]> {
  const { listWaitingAsks } = await import('$lib/api/nebo');
  const res = await listWaitingAsks();
  const asks = res.asks ?? [];
  waitingAsks.set(asks);
  return asks;
}

/** The whole list, as `asks_waiting` carries it. */
export function setWaitingAsks(data: { asks?: WaitingAsk[] } | null | undefined): void {
  waitingAsks.set(data?.asks ?? []);
}

/** Answer from the bar with option `i`: what a tap on that option sends.
 *  An ask that is an Inbox item (every permission ask, a relayed question)
 *  is answered on the Inbox's one answer path; a question in the owner's
 *  own chat is that chat's card and its reply goes on the socket. The
 *  server's `asks_waiting` then clears it. */
export async function answerWaiting(ask: WaitingAsk, i: number): Promise<void> {
  const value = ask.values[i] ?? ask.options[i];
  if (!value) return;
  const item = waitingItemId(ask);
  if (item) {
    await answerItem(item, value, 'chat');
    return;
  }
  getWebSocketClient().send('ask_response', { request_id: ask.id, value });
}

/** Where an ask waits: its conversation, or the employee when it has none. */
export function askPath(ask: WaitingAsk): string {
  return ask.chatId ? `/${ask.agentId}/threads/${encodeURIComponent(ask.chatId)}` : `/${ask.agentId}`;
}

/** The assistant's chat: the owner's front door. What waits on him from
 *  work he is not in (a colleague's hand-off, a schedule, a background run)
 *  is relayed there, and its notice opens there. The server's
 *  `owner_notify::link::assistant`. */
export const ASSISTANT_PATH = '/assistant';

/** Whether `pathname` is the assistant's chat. */
export function inAssistantChat(pathname: string): boolean {
  return pathname === ASSISTANT_PATH || pathname.startsWith(ASSISTANT_PATH + '/');
}

/** The asks relayed to the assistant's chat, oldest first: those raised by
 *  work the owner is not in. A card from a chat he is in stays there. */
export function relayedAsks(asks: WaitingAsk[]): WaitingAsk[] {
  return asks.filter((a) => a.relayed);
}
