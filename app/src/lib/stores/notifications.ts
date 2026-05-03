import { writable, derived } from 'svelte/store';
import { logger } from '$lib/monitoring';

export type NotificationType = 'agent' | 'system' | 'warning' | 'error';

export interface Notification {
  id: string;
  type: NotificationType;
  title: string;
  message: string;
  time: string;
  read: boolean;
  link?: string;
}

export const notifications = writable<Notification[]>([]);

export const unreadCount = derived(notifications, ($n) =>
  $n.filter(n => !n.read).length
);

let loaded = false;

/**
 * Load notifications from the backend API.
 * Falls back to initial hardcoded data if API is unreachable.
 */
export async function loadNotifications(): Promise<void> {
  if (loaded) return;
  try {
    const { listNotifications } = await import('$lib/api/nebo');
    const data = await listNotifications();
    const mapped: Notification[] = (data.notifications || []).map(n => ({
      id: n.id,
      type: (n.type as NotificationType) || 'system',
      title: n.title,
      message: n.body || '',
      time: formatRelativeTime(n.createdAt),
      read: !!n.readAt,
      link: n.actionUrl || undefined,
    }));
    notifications.set(mapped);
    loaded = true;
    logger.debug('Loaded notifications from API');
  } catch {
    logger.debug('Notifications API unavailable');
    loaded = true;
  }
}

export function markAsRead(id: string) {
  notifications.update(list =>
    list.map(n => n.id === id ? { ...n, read: true } : n)
  );
  // Fire-and-forget API call
  import('$lib/api/nebo').then(api => api.markNotificationRead(id)).catch(() => {});
}

export function markAllRead() {
  notifications.update(list =>
    list.map(n => ({ ...n, read: true }))
  );
  import('$lib/api/nebo').then(api => api.markAllNotificationsRead()).catch(() => {});
}

export function removeNotification(id: string) {
  notifications.update(list => list.filter(n => n.id !== id));
  import('$lib/api/nebo').then(api => api.deleteNotification(id)).catch(() => {});
}

function formatRelativeTime(isoDate: string): string {
  try {
    const date = new Date(isoDate);
    const now = Date.now();
    const diffMs = now - date.getTime();
    const diffMin = Math.floor(diffMs / 60_000);
    if (diffMin < 1) return 'just now';
    if (diffMin < 60) return `${diffMin} minute${diffMin > 1 ? 's' : ''} ago`;
    const diffHr = Math.floor(diffMin / 60);
    if (diffHr < 24) return `${diffHr} hour${diffHr > 1 ? 's' : ''} ago`;
    const diffDay = Math.floor(diffHr / 24);
    return `${diffDay} day${diffDay > 1 ? 's' : ''} ago`;
  } catch {
    return isoDate;
  }
}
