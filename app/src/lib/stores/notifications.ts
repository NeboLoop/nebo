import { writable, derived, get } from 'svelte/store';
import { logger } from '$lib/monitoring';
import { formatRelative } from '$lib/time';

export type NotificationType = 'agent' | 'system' | 'warning' | 'error';

export interface Notification {
  id: string;
  type: NotificationType;
  title: string;
  message: string;
  time: string;
  createdAt: number; // epoch ms — the Inbox groups by day with this
  read: boolean;
  link?: string;
  /** The AI employee that produced this notification (absent for system events). */
  agentId?: string;
}

export const notifications = writable<Notification[]>([]);

export const unreadCount = derived(notifications, ($n) =>
  $n.filter(n => !n.read).length
);

let loaded = false;
const PAGE_SIZE = 50;

/** True while the backend may have older pages beyond what's loaded. */
export const hasMore = writable(false);

async function fetchPage(offset: number): Promise<void> {
  const { listNotifications } = await import('$lib/api/nebo');
  const data = await listNotifications(PAGE_SIZE, offset);
  const mapped: Notification[] = (data.notifications || []).map(n => ({
    id: n.id,
    type: (n.type as NotificationType) || 'system',
    title: n.title,
    message: n.body || '',
    time: formatRelative(n.createdAt ? n.createdAt * 1000 : Date.now()),
    createdAt: n.createdAt ? n.createdAt * 1000 : Date.now(),
    read: !!n.readAt,
    link: n.actionUrl || undefined,
    agentId: n.agentId || undefined,
  }));
  hasMore.set(mapped.length === PAGE_SIZE);
  if (offset === 0) {
    notifications.set(mapped);
  } else {
    notifications.update(list => {
      const seen = new Set(list.map(n => n.id));
      return [...list, ...mapped.filter(m => !seen.has(m.id))];
    });
  }
}

/**
 * Load the first page of notifications from the backend API.
 */
export async function loadNotifications(): Promise<void> {
  if (loaded) return;
  loaded = true;
  try {
    await fetchPage(0);
    logger.debug('Loaded notifications from API');
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

/**
 * Push a new notification into the store from a WebSocket event payload.
 * No API call — pure push.
 */
export function pushNotification(data: {
  id: string;
  type?: string;
  title: string;
  body?: string;
  actionUrl?: string;
  readAt?: number | null;
  createdAt?: number;
  agentId?: string;
}): void {
  const notif: Notification = {
    id: data.id,
    type: (data.type as NotificationType) || 'system',
    title: data.title,
    message: data.body || '',
    time: formatRelative(data.createdAt ? data.createdAt * 1000 : Date.now()),
    createdAt: data.createdAt ? data.createdAt * 1000 : Date.now(),
    read: !!data.readAt,
    link: data.actionUrl || undefined,
    agentId: data.agentId || undefined,
  };
  // Upsert by id: repeated broadcasts (or WS + REST overlap) must never
  // produce duplicate rows — ids like artifact-update:plugin:gws:0.23.1 are
  // deliberately stable across re-emits.
  notifications.update(list => [notif, ...list.filter(x => x.id !== notif.id)]);
}

export function markAsRead(id: string) {
  notifications.update(list =>
    list.map(n => n.id === id ? { ...n, read: true } : n)
  );
  // Fire-and-forget API call
  import('$lib/api/nebo').then(api => api.markRead(id)).catch(() => {});
}

export function markAllRead() {
  notifications.update(list =>
    list.map(n => ({ ...n, read: true }))
  );
  import('$lib/api/nebo').then(api => api.markAllRead()).catch(() => {});
}

export function removeNotification(id: string) {
  notifications.update(list => list.filter(n => n.id !== id));
  import('$lib/api/nebo').then(api => api.deleteNotification(id)).catch(() => {});
}

