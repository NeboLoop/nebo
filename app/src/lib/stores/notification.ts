import { writable, derived, get } from 'svelte/store';
import * as api from '$lib/api/nebo';
import type { Notification } from '$lib/api/neboComponents';
import { logger } from '$lib/monitoring';

/**
 * Notification state interface
 */
export interface NotificationState {
	notifications: Notification[];
	unreadCount: number;
	isLoading: boolean;
	error: string | null;
}

const initialState: NotificationState = {
	notifications: [],
	unreadCount: 0,
	isLoading: false,
	error: null
};

// Polling interval for unread count (30 seconds)
const POLL_INTERVAL = 30000;
let pollTimer: ReturnType<typeof setInterval> | null = null;

function createNotificationStore() {
	const { subscribe, set, update } = writable<NotificationState>(initialState);

	return {
		subscribe,

		/**
		 * Load notifications
		 */
		async loadNotifications(options?: { unread?: boolean; page?: number }): Promise<void> {
			update((state) => ({ ...state, isLoading: true, error: null }));

			try {
				const response = await api.listNotifications({
					unread: options?.unread,
					page: options?.page,
					pageSize: 20
				});

				update((state) => ({
					...state,
					notifications: response.notifications,
					unreadCount: response.unreadCount,
					isLoading: false
				}));

				logger.debug('Notifications loaded', { count: response.notifications.length });
			} catch (error) {
				const errorMessage =
					error instanceof Error ? error.message : 'Failed to load notifications';
				update((state) => ({ ...state, isLoading: false, error: errorMessage }));
				logger.error('Failed to load notifications', error);
			}
		},

		/**
		 * Fetch unread count only (for polling)
		 */
		async fetchUnreadCount(): Promise<number> {
			try {
				const response = await api.getUnreadCount();
				update((state) => ({ ...state, unreadCount: response.count }));
				return response.count;
			} catch (error) {
				logger.error('Failed to fetch unread count', error);
				return get({ subscribe }).unreadCount;
			}
		},

		/**
		 * Mark a notification as read
		 */
		async markAsRead(id: string): Promise<boolean> {
			try {
				await api.markNotificationRead(id);
				update((state) => ({
					...state,
					notifications: state.notifications.map((n) =>
						n.id === id ? { ...n, readAt: new Date().toISOString() } : n
					),
					unreadCount: Math.max(0, state.unreadCount - 1)
				}));
				return true;
			} catch (error) {
				logger.error('Failed to mark notification as read', error);
				return false;
			}
		},

		/**
		 * Mark all notifications as read
		 */
		async markAllAsRead(): Promise<boolean> {
			try {
				await api.markAllNotificationsRead();
				update((state) => ({
					...state,
					notifications: state.notifications.map((n) => ({
						...n,
						readAt: n.readAt || new Date().toISOString()
					})),
					unreadCount: 0
				}));
				return true;
			} catch (error) {
				logger.error('Failed to mark all notifications as read', error);
				return false;
			}
		},

		/**
		 * Delete a notification
		 */
		async deleteNotification(id: string): Promise<boolean> {
			try {
				await api.deleteNotification(id);
				update((state) => {
					const notification = state.notifications.find((n) => n.id === id);
					const wasUnread = notification && !notification.readAt;
					return {
						...state,
						notifications: state.notifications.filter((n) => n.id !== id),
						unreadCount: wasUnread ? Math.max(0, state.unreadCount - 1) : state.unreadCount
					};
				});
				return true;
			} catch (error) {
				logger.error('Failed to delete notification', error);
				return false;
			}
		},

		/**
		 * Start polling for unread count
		 */
		startPolling(): void {
			if (typeof window === 'undefined') return;
			this.stopPolling();

			// Fetch immediately
			this.fetchUnreadCount();

			// Then poll periodically
			pollTimer = setInterval(() => {
				this.fetchUnreadCount();
			}, POLL_INTERVAL);

			logger.debug('Notification polling started');
		},

		/**
		 * Stop polling
		 */
		stopPolling(): void {
			if (pollTimer) {
				clearInterval(pollTimer);
				pollTimer = null;
				logger.debug('Notification polling stopped');
			}
		},

		/**
		 * Add a notification locally (for real-time updates)
		 */
		addNotification(notification: Notification): void {
			update((state) => ({
				...state,
				notifications: [notification, ...state.notifications],
				unreadCount: state.unreadCount + 1
			}));
		},

		/**
		 * Reset store
		 */
		reset(): void {
			this.stopPolling();
			set(initialState);
		}
	};
}

export const notification = createNotificationStore();

// Derived stores
export const notifications = derived(notification, ($n) => $n.notifications);
export const unreadNotificationCount = derived(notification, ($n) => $n.unreadCount);
export const hasUnreadNotifications = derived(notification, ($n) => $n.unreadCount > 0);
export const notificationLoading = derived(notification, ($n) => $n.isLoading);
