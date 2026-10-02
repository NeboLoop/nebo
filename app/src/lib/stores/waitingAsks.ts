// Everything waiting on the owner's answer, from every employee: the
// questions a run is parked on and the permission asks. The pinned bar at the
// top of the chat reads it. `GET /asks` loads it once; `asks_waiting` carries
// the whole list again whenever it changes, so nothing polls.

import { writable } from 'svelte/store';
import type { WaitingAsk } from '$lib/api/neboComponents';
import { getWebSocketClient } from '$lib/websocket/client';
import { answerAsk, type AskAnswer } from './permissionAsks';

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

/** Answer from the bar with option `i`: what a tap on that option sends,
 *  through the ask's own answer path (a question's reply on the socket, a
 *  permission ask's answer). The server's `asks_waiting` then clears it. */
export async function answerWaiting(ask: WaitingAsk, i: number): Promise<void> {
  const value = ask.values[i] ?? ask.options[i];
  if (!value) return;
  if (ask.kind === 'permission') {
    await answerAsk(ask.id, value as AskAnswer, 'chat');
    return;
  }
  getWebSocketClient().send('ask_response', { request_id: ask.id, value });
}

/** Where an ask waits: its conversation, or the employee when it has none. */
export function askPath(ask: WaitingAsk): string {
  return ask.chatId ? `/${ask.agentId}/threads/${encodeURIComponent(ask.chatId)}` : `/${ask.agentId}`;
}
