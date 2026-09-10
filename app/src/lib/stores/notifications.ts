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

// ── Approvals ───────────────────────────────────────────────────────────
// A pending decision is a task, not mail: reading it does not settle it.
// The inbox pins `wf-approval:<run>`, `learn:<pending>` and
// `artifact-update:<type>:<artifact>:<version>` rows in an approval band
// while they are pending, but the sidebar badge counted only unread rows,
// so five open approvals the owner had looked at showed as a badge of 1
// (Danny, 2026-09-09). The statuses live here so the badge and the band
// read the same map, and a decision made in the band updates both.
export type ApprovalRef = { kind: 'workflow' | 'learning' | 'update'; id: string };

export const approvalRef = (id: string): ApprovalRef | null =>
  id.startsWith('wf-approval:')
    ? { kind: 'workflow', id: id.slice('wf-approval:'.length) }
    : id.startsWith('learn:')
      ? { kind: 'learning', id: id.slice('learn:'.length) }
      : id.startsWith('artifact-update:')
        ? { kind: 'update', id: id.split(':')[2] ?? '' }
        : null;

/** notification id → 'pending' | 'approved' | 'denied' | 'applied' | ... */
export const approvalStatuses = writable<Record<string, string>>({});
const statusFetched = new Set<string>();

export function setApprovalStatus(id: string, status: string) {
  statusFetched.add(id);
  approvalStatuses.update(m => ({ ...m, [id]: status }));
}

/** Fetch the status of every approval-shaped notification not yet known. */
export async function ensureApprovalStatuses(): Promise<void> {
  const list = get(notifications);
  const api = await import('$lib/api/nebo');
  let updates: Promise<Awaited<ReturnType<typeof api.listUpdates>>> | null = null;
  await Promise.all(list.map(async (n) => {
    const ref = approvalRef(n.id);
    if (!ref || statusFetched.has(n.id)) return;
    statusFetched.add(n.id);
    try {
      let status: string;
      if (ref.kind === 'workflow') {
        status = ((await api.getWorkflowApprovalStatus(ref.id)) as { status?: string }).status ?? 'unknown';
      } else if (ref.kind === 'learning') {
        status = ((await api.getLearning(ref.id)) as { status?: string }).status ?? 'unknown';
      } else {
        updates ??= api.listUpdates();
        const u = ((await updates).updates ?? []).find(x => x.artifactId === ref.id);
        status = u?.updateAvailable ? 'pending' : 'applied';
      }
      approvalStatuses.update(m => ({ ...m, [n.id]: status }));
    } catch {
      statusFetched.delete(n.id);
    }
  }));
}

/** What needs the owner: unread rows, plus approvals still pending however
 *  many times they were read. This is the sidebar badge. */
export const unreadCount = derived([notifications, approvalStatuses], ([$n, $s]) =>
  $n.filter(n => !n.read || (approvalRef(n.id) !== null && $s[n.id] === 'pending')).length
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
  void ensureApprovalStatuses();
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

