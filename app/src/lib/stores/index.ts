// Auth store and related exports
export {
	auth,
	isAuthenticated,
	currentUser,
	authError,
	authLoading,
	passwordReset,
	sessionExpiry,
	showSessionWarning,
	sessionSecondsRemaining,
	type AuthState,
	type PasswordResetState,
	type SessionExpiryState
} from './auth';

// Notification store and related exports
export {
	notification,
	notifications,
	unreadNotificationCount,
	hasUnreadNotifications,
	notificationLoading,
	type NotificationState
} from './notification';

// WebSocket: Use getWebSocketClient() from '$lib/websocket/client' instead
// The existing WebSocket client is connected at layout level

// Setup wizard store (Svelte 5 runes)
export { setup, setupState, type SetupState } from './setup.svelte';
