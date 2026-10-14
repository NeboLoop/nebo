// The Inbox, as the server keeps it. Each item is the server's row, status
// and answers included: nothing here works out a status of its own. The
// first page of items not put away loads once, with every decision still
// open (however old); History loads when it is asked for. WS events keep it
// current: `notification` / `notification_created` add or restate an item,
// `notification_settled` moves a decision to History. The counts (the badge
// and the Inbox's greeting) are the server's.

import { writable, derived, get } from 'svelte/store';
import { logger } from '$lib/monitoring';
import type { Notification } from '$lib/api/neboComponents';

export type InboxItem = Notification;

/** Items not put away, newest first: decisions (open or answered) and updates. */
export const notifications = writable<InboxItem[]>([]);

/** Answered and put-away items, newest first, once History was opened. */
export const history = writable<InboxItem[]>([]);

/** What waits on the owner: decisions still open, updates not yet read. */
export const inboxCounts = writable<{ needsYou: number; unread: number }>({ needsYou: 0, unread: 0 });

/** The badge: both. */
export const unreadCount = derived(inboxCounts, ($c) => $c.needsYou + $c.unread);

/** True while the backend may have older pages beyond what's loaded. */
export const hasMore = writable(false);

const PAGE_SIZE = 50;
let loaded = false;

const newestFirst = (a: InboxItem, b: InboxItem) => b.createdAt - a.createdAt;

/** An item as an event carries it, with what an older payload leaves out. */
function itemOf(data: Partial<InboxItem> & { id: string; message?: string; link?: string }): InboxItem {
  const needsYou = !!data.needsYou;
  return {
    ...data,
    id: data.id,
    type: data.type || 'info',
    title: data.title ?? '',
    body: data.body ?? data.message ?? '',
    actionUrl: data.actionUrl ?? data.link,
    createdAt: data.createdAt ?? Math.floor(Date.now() / 1000),
    readAt: data.readAt ?? undefined,
    needsYou,
    options: data.options ?? [],
    status: data.status || (needsYou ? 'open' : 'update'),
  } as InboxItem;
}

function merge(list: InboxItem[], more: InboxItem[]): InboxItem[] {
  const byId = new Map(list.map((n) => [n.id, n]));
  for (const n of more) byId.set(n.id, n);
  return [...byId.values()].sort(newestFirst);
}

export async function refreshCounts(): Promise<void> {
  const api = await import('$lib/api/nebo');
  const c = await api.unreadCount();
  inboxCounts.set({ needsYou: c.needsYou ?? 0, unread: c.unread ?? 0 });
}

async function fetchPage(offset: number): Promise<void> {
  const { listNotifications } = await import('$lib/api/nebo');
  const page = (await listNotifications(PAGE_SIZE, offset)).notifications ?? [];
  hasMore.set(page.length === PAGE_SIZE);
  if (offset === 0) {
    // Every open decision, however far down the list it is.
    const open = (await listNotifications(200, 0, 'needs_you')).notifications ?? [];
    notifications.set(merge(page, open));
  } else {
    notifications.update((list) => merge(list, page));
  }
}

/** Load the first page (once; the badge needs it app-wide). */
export async function loadNotifications(): Promise<void> {
  if (loaded) return;
  loaded = true;
  try {
    await Promise.all([fetchPage(0), refreshCounts()]);
  } catch {
    logger.debug('Notifications API unavailable');
  }
}

/** Fetch the next page (infinite scroll). */
export async function loadMore(): Promise<void> {
  try {
    await fetchPage(get(notifications).length);
  } catch {
    logger.debug('Notifications API unavailable');
  }
}

/** Load History: what was answered and what was put away. */
export async function loadHistory(): Promise<void> {
  try {
    const { listNotifications } = await import('$lib/api/nebo');
    history.set(((await listNotifications(200, 0, 'history')).notifications ?? []).sort(newestFirst));
  } catch {
    logger.debug('Notifications API unavailable');
  }
}

/** Put the server's copy of an item in place (an answer's result). */
export function upsertItem(item: InboxItem): void {
  const archived = item.status === 'archived';
  notifications.update((list) => (archived ? list.filter((n) => n.id !== item.id) : merge(list, [item])));
  if (item.status === 'answered' || archived) history.update((list) => merge(list, [item]));
  void refreshCounts().catch(() => {});
}

/** A new or restated item from a WebSocket event. Upsert by id: a repeated
 *  broadcast (or WS + REST overlap) never makes a second row. */
export function pushNotification(data: Partial<InboxItem> & { id: string; message?: string; link?: string }): InboxItem {
  const item = itemOf(data);
  notifications.update((list) => merge(list, [item]));
  void refreshCounts().catch(() => {});
  return item;
}

/** A decision was answered somewhere (here, the mobile app, the assistant's
 *  chat): it moves to History with the answer and where it came from. */
export function settleNotification(data: { id: string; answer?: string; answeredVia?: string | null; status?: string }): void {
  const now = Math.floor(Date.now() / 1000);
  const settle = (n: InboxItem): InboxItem => ({
    ...n,
    status: data.status || 'answered',
    answer: data.answer ?? n.answer,
    answeredVia: data.answeredVia ?? undefined,
    answeredAt: n.answeredAt ?? now,
    readAt: n.readAt ?? now,
  });
  const found = get(notifications).find((n) => n.id === data.id);
  if (found) {
    const settled = settle(found);
    notifications.update((list) => list.map((n) => (n.id === data.id ? settled : n)));
    history.update((list) => merge(list, [settled]));
  }
  void refreshCounts().catch(() => {});
}

export function markAsRead(id: string) {
  const now = Math.floor(Date.now() / 1000);
  notifications.update((list) => list.map((n) => (n.id === id && !n.readAt ? { ...n, readAt: now } : n)));
  import('$lib/api/nebo')
    .then((api) => api.markRead(id))
    .then(() => refreshCounts())
    .catch(() => {});
}

export function markAllRead() {
  const now = Math.floor(Date.now() / 1000);
  notifications.update((list) => list.map((n) => (n.readAt ? n : { ...n, readAt: now })));
  import('$lib/api/nebo')
    .then((api) => api.markAllRead())
    .then(() => refreshCounts())
    .catch(() => {});
}

/** Put an item away: it moves to History. Nothing is deleted. */
export function removeNotification(id: string) {
  const found = get(notifications).find((n) => n.id === id);
  notifications.update((list) => list.filter((n) => n.id !== id));
  if (found) history.update((list) => merge(list, [{ ...found, status: 'archived', archivedAt: Math.floor(Date.now() / 1000) }]));
  import('$lib/api/nebo')
    .then((api) => api.deleteNotification(id))
    .then(() => refreshCounts())
    .catch(() => {});
}
