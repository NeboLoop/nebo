import { writable, derived, get } from 'svelte/store';
import * as api from '$lib/api/gobot';
import type { User, LoginRequest, RegisterRequest, LoginResponse } from '$lib/api/gobotComponents';
import { logger } from '$lib/monitoring';

/**
 * Authentication state interface
 */
export interface AuthState {
	user: User | null;
	token: string | null;
	refreshToken: string | null;
	expiresAt: number | null;
	isLoading: boolean;
	error: string | null;
}

/**
 * Session expiry state interface
 */
export interface SessionExpiryState {
	showWarning: boolean;
	secondsRemaining: number;
}

/**
 * Password reset request state
 */
export interface PasswordResetState {
	isLoading: boolean;
	isSuccess: boolean;
	error: string | null;
}

// Token storage keys
const TOKEN_KEY = 'gobot_token';
const REFRESH_TOKEN_KEY = 'gobot_refresh_token';
const EXPIRES_AT_KEY = 'gobot_expires_at';
const USER_KEY = 'gobot_user';

/**
 * Get stored auth data from localStorage
 */
function getStoredAuth(): {
	token: string | null;
	refreshToken: string | null;
	expiresAt: number | null;
	user: User | null;
} {
	if (typeof window === 'undefined') {
		return { token: null, refreshToken: null, expiresAt: null, user: null };
	}

	try {
		const token = localStorage.getItem(TOKEN_KEY);
		const refreshToken = localStorage.getItem(REFRESH_TOKEN_KEY);
		const expiresAtStr = localStorage.getItem(EXPIRES_AT_KEY);
		const expiresAt = expiresAtStr ? parseInt(expiresAtStr, 10) : null;
		const userStr = localStorage.getItem(USER_KEY);
		const user = userStr ? (JSON.parse(userStr) as User) : null;

		return { token, refreshToken, expiresAt, user };
	} catch {
		logger.warn('Failed to read auth data from localStorage');
		return { token: null, refreshToken: null, expiresAt: null, user: null };
	}
}

/**
 * Store auth data in localStorage
 */
function storeTokens(token: string, refreshToken: string, expiresAt: number): void {
	if (typeof window === 'undefined') return;

	try {
		localStorage.setItem(TOKEN_KEY, token);
		localStorage.setItem(REFRESH_TOKEN_KEY, refreshToken);
		localStorage.setItem(EXPIRES_AT_KEY, expiresAt.toString());
	} catch {
		logger.warn('Failed to store auth tokens in localStorage');
	}
}

/**
 * Store user in localStorage
 */
function storeUser(user: User): void {
	if (typeof window === 'undefined') return;

	try {
		localStorage.setItem(USER_KEY, JSON.stringify(user));
	} catch {
		logger.warn('Failed to store user in localStorage');
	}
}

/**
 * Clear all auth data from localStorage
 */
function clearStoredAuth(): void {
	if (typeof window === 'undefined') return;

	try {
		localStorage.removeItem(TOKEN_KEY);
		localStorage.removeItem(REFRESH_TOKEN_KEY);
		localStorage.removeItem(EXPIRES_AT_KEY);
		localStorage.removeItem(USER_KEY);
	} catch {
		logger.warn('Failed to clear auth data from localStorage');
	}
}

/**
 * Initial auth state
 */
const initialState: AuthState = {
	user: null,
	token: null,
	refreshToken: null,
	expiresAt: null,
	isLoading: false,
	error: null
};

/**
 * Create the auth store
 */
function createAuthStore() {
	const { subscribe, set, update } = writable<AuthState>(initialState);

	// Initialize store with stored auth data on client
	if (typeof window !== 'undefined') {
		const storedAuth = getStoredAuth();
		// Only load if we have a token - don't clear anything during initialization
		// This prevents HMR from logging users out
		if (storedAuth.token) {
			// Check if token is expired
			const isExpired = storedAuth.expiresAt && storedAuth.expiresAt < Date.now();

			if (!isExpired) {
				update((state) => ({
					...state,
					token: storedAuth.token,
					refreshToken: storedAuth.refreshToken,
					expiresAt: storedAuth.expiresAt,
					user: storedAuth.user
				}));
			}
			// If expired, don't clear - let the user stay on the page
			// They'll get redirected when they try to make an API call
		}
		// IMPORTANT: Never clear auth data during store initialization
		// Only clear on explicit logout
	}

	return {
		subscribe,

		/**
		 * Login with email and password
		 */
		async login(credentials: LoginRequest): Promise<boolean> {
			update((state) => ({ ...state, isLoading: true, error: null }));

			try {
				const response: LoginResponse = await api.login(credentials);

				storeTokens(response.token, response.refreshToken, response.expiresAt);

				update((state) => ({
					...state,
					token: response.token,
					refreshToken: response.refreshToken,
					expiresAt: response.expiresAt,
					isLoading: false,
					error: null
				}));

				// Fetch user profile after login
				await this.fetchCurrentUser();

				logger.info('User logged in successfully');
				return true;
			} catch (error) {
				const errorMessage =
					error instanceof Error ? error.message : 'Login failed. Please try again.';
				update((state) => ({
					...state,
					isLoading: false,
					error: errorMessage
				}));
				logger.error('Login failed', error);
				return false;
			}
		},

		/**
		 * Register a new user
		 */
		async register(userData: RegisterRequest): Promise<{ success: boolean }> {
			update((state) => ({ ...state, isLoading: true, error: null }));

			try {
				const response: LoginResponse = await api.register(userData);

				storeTokens(response.token, response.refreshToken, response.expiresAt);

				update((state) => ({
					...state,
					token: response.token,
					refreshToken: response.refreshToken,
					expiresAt: response.expiresAt,
					isLoading: false,
					error: null
				}));

				// Fetch user profile after registration
				await this.fetchCurrentUser();

				logger.info('User registered successfully');
				return { success: true };
			} catch (error) {
				const errorMessage =
					error instanceof Error ? error.message : 'Registration failed. Please try again.';
				update((state) => ({
					...state,
					isLoading: false,
					error: errorMessage
				}));
				logger.error('Registration failed', error);
				return { success: false };
			}
		},

		/**
		 * Fetch current user profile
		 */
		async fetchCurrentUser(): Promise<void> {
			// Check both store state and localStorage for token
			const state = get({ subscribe });
			const storedToken = typeof window !== 'undefined' ? localStorage.getItem(TOKEN_KEY) : null;
			const token = state.token || storedToken;

			if (!token) return;

			// If store doesn't have token but localStorage does, update store
			if (!state.token && storedToken) {
				const storedAuth = getStoredAuth();
				update((s) => ({
					...s,
					token: storedAuth.token,
					refreshToken: storedAuth.refreshToken,
					expiresAt: storedAuth.expiresAt,
					user: storedAuth.user
				}));
			}

			try {
				const response = await api.getCurrentUser();
				update((s) => ({ ...s, user: response.user }));
				storeUser(response.user);
				logger.debug('Fetched current user profile');
			} catch (error) {
				logger.error('Failed to fetch current user', error);
				// If fetching user fails, might mean token is invalid
				// Don't logout immediately, let token refresh handle it
			}
		},

		/**
		 * Update current user profile
		 */
		async updateProfile(data: { name?: string }): Promise<boolean> {
			update((state) => ({ ...state, isLoading: true, error: null }));

			try {
				const response = await api.updateCurrentUser(data);
				update((state) => ({
					...state,
					user: response.user,
					isLoading: false,
					error: null
				}));
				storeUser(response.user);
				logger.info('User profile updated successfully');
				return true;
			} catch (error) {
				const errorMessage = error instanceof Error ? error.message : 'Failed to update profile.';
				update((state) => ({
					...state,
					isLoading: false,
					error: errorMessage
				}));
				logger.error('Profile update failed', error);
				return false;
			}
		},

		/**
		 * Refresh authentication token
		 */
		async refreshAuthToken(): Promise<boolean> {
			const state = get({ subscribe });
			if (!state.refreshToken) return false;

			try {
				const response = await api.refreshToken({ refreshToken: state.refreshToken });

				// Keep the same refresh token, update access token
				storeTokens(response.token, state.refreshToken, response.expiresAt);

				update((s) => ({
					...s,
					token: response.token,
					expiresAt: response.expiresAt
				}));

				logger.debug('Auth token refreshed');
				return true;
			} catch (error: any) {
				logger.error('Token refresh failed', error);

				// Only logout if the server explicitly rejected the token (401/403)
				// Don't logout on network errors (backend might just be restarting)
				const status = error?.response?.status;
				if (status === 401 || status === 403) {
					this.logout();
				}

				return false;
			}
		},

		/**
		 * Logout the user
		 */
		logout(): void {
			clearStoredAuth();
			set(initialState);
			logger.info('User logged out');
		},

		/**
		 * Clear any error messages
		 */
		clearError(): void {
			update((state) => ({ ...state, error: null }));
		},

		/**
		 * Check if token needs refresh (within 5 minutes of expiry)
		 */
		needsTokenRefresh(): boolean {
			const state = get({ subscribe });
			if (!state.expiresAt) return false;
			const fiveMinutes = 5 * 60 * 1000;
			return state.expiresAt - Date.now() < fiveMinutes;
		},

		/**
		 * Set tokens from OAuth callback (used by /auth/callback page)
		 */
		async setOAuthTokens(token: string, refreshToken: string, expiresAt: number): Promise<boolean> {
			try {
				storeTokens(token, refreshToken, expiresAt);

				update((state) => ({
					...state,
					token,
					refreshToken,
					expiresAt,
					isLoading: false,
					error: null
				}));

				// Fetch user profile
				await this.fetchCurrentUser();

				logger.info('OAuth login successful');
				return true;
			} catch (error) {
				logger.error('OAuth token setup failed', error);
				return false;
			}
		}
	};
}

// Export the auth store singleton
export const auth = createAuthStore();

// Derived stores for convenience
// Only check token for authentication - user profile might fail to load if backend is temporarily down
// This prevents logout when backend restarts
export const isAuthenticated = derived(
	auth,
	($auth) => !!$auth.token && (!$auth.expiresAt || $auth.expiresAt > Date.now())
);
export const currentUser = derived(auth, ($auth) => $auth.user);
export const authError = derived(auth, ($auth) => $auth.error);
export const authLoading = derived(auth, ($auth) => $auth.isLoading);

/**
 * Password reset store (separate from main auth for cleaner state management)
 */
function createPasswordResetStore() {
	const { subscribe, set, update } = writable<PasswordResetState>({
		isLoading: false,
		isSuccess: false,
		error: null
	});

	return {
		subscribe,

		/**
		 * Request password reset email
		 */
		async requestReset(email: string): Promise<boolean> {
			update((state) => ({ ...state, isLoading: true, error: null, isSuccess: false }));

			try {
				await api.forgotPassword({ email });

				update((state) => ({
					...state,
					isLoading: false,
					isSuccess: true,
					error: null
				}));

				logger.info('Password reset requested', { email });
				return true;
			} catch (error) {
				const errorMessage = error instanceof Error ? error.message : 'Failed to send reset email.';
				update((state) => ({
					...state,
					isLoading: false,
					isSuccess: false,
					error: errorMessage
				}));
				logger.error('Password reset request failed', error);
				return false;
			}
		},

		/**
		 * Reset password with token
		 */
		async resetPassword(token: string, newPassword: string): Promise<boolean> {
			update((state) => ({ ...state, isLoading: true, error: null, isSuccess: false }));

			try {
				await api.resetPassword({ token, newPassword });

				update((state) => ({
					...state,
					isLoading: false,
					isSuccess: true,
					error: null
				}));

				logger.info('Password reset successfully');
				return true;
			} catch (error) {
				const errorMessage = error instanceof Error ? error.message : 'Failed to reset password.';
				update((state) => ({
					...state,
					isLoading: false,
					isSuccess: false,
					error: errorMessage
				}));
				logger.error('Password reset failed', error);
				return false;
			}
		},

		/**
		 * Reset the store state
		 */
		reset(): void {
			set({
				isLoading: false,
				isSuccess: false,
				error: null
			});
		}
	};
}

export const passwordReset = createPasswordResetStore();

/**
 * Session expiry monitoring
 * Shows a warning modal before the session expires, giving users a chance to continue
 */
const SESSION_WARNING_SECONDS = 120; // Show warning 2 minutes before expiry
const SESSION_CHECK_INTERVAL = 10000; // Check every 10 seconds

let sessionMonitorInterval: ReturnType<typeof setInterval> | null = null;
let countdownInterval: ReturnType<typeof setInterval> | null = null;

function createSessionExpiryStore() {
	const { subscribe, set, update } = writable<SessionExpiryState>({
		showWarning: false,
		secondsRemaining: 0
	});

	function startCountdown(secondsUntilExpiry: number) {
		// Clear any existing countdown
		if (countdownInterval) {
			clearInterval(countdownInterval);
		}

		update((state) => ({
			...state,
			showWarning: true,
			secondsRemaining: Math.max(0, Math.floor(secondsUntilExpiry))
		}));

		countdownInterval = setInterval(() => {
			update((state) => {
				const newSeconds = state.secondsRemaining - 1;
				if (newSeconds <= 0) {
					// Time's up - auto logout
					if (countdownInterval) {
						clearInterval(countdownInterval);
						countdownInterval = null;
					}
					auth.logout();
					// Redirect to login
					if (typeof window !== 'undefined') {
						window.location.href = '/auth/login?expired=true';
					}
					return { showWarning: false, secondsRemaining: 0 };
				}
				return { ...state, secondsRemaining: newSeconds };
			});
		}, 1000);
	}

	function stopCountdown() {
		if (countdownInterval) {
			clearInterval(countdownInterval);
			countdownInterval = null;
		}
		set({ showWarning: false, secondsRemaining: 0 });
	}

	return {
		subscribe,

		/**
		 * Start monitoring the session for expiry
		 * Call this when the app initializes with an authenticated user
		 */
		startMonitoring(): void {
			if (typeof window === 'undefined') return;

			// Clear any existing monitor
			this.stopMonitoring();

			const checkSession = () => {
				const authState = get(auth);
				if (!authState.token || !authState.expiresAt) {
					return;
				}

				const now = Date.now();
				const expiresAt = authState.expiresAt;
				const secondsUntilExpiry = Math.floor((expiresAt - now) / 1000);

				// If already expired, logout
				if (secondsUntilExpiry <= 0) {
					auth.logout();
					if (typeof window !== 'undefined') {
						window.location.href = '/auth/login?expired=true';
					}
					return;
				}

				// If within warning window, show the modal
				const currentState = get({ subscribe });
				if (secondsUntilExpiry <= SESSION_WARNING_SECONDS && !currentState.showWarning) {
					logger.info('Session expiring soon, showing warning modal');
					startCountdown(secondsUntilExpiry);
				}
			};

			// Check immediately
			checkSession();

			// Then check periodically
			sessionMonitorInterval = setInterval(checkSession, SESSION_CHECK_INTERVAL);
			logger.debug('Session monitor started');
		},

		/**
		 * Stop monitoring the session
		 * Call this on logout
		 */
		stopMonitoring(): void {
			if (sessionMonitorInterval) {
				clearInterval(sessionMonitorInterval);
				sessionMonitorInterval = null;
			}
			stopCountdown();
			logger.debug('Session monitor stopped');
		},

		/**
		 * Continue the session by refreshing the token
		 * Call this when user clicks "Continue Session"
		 */
		async continueSession(): Promise<boolean> {
			stopCountdown();
			const success = await auth.refreshAuthToken();
			if (success) {
				logger.info('Session continued successfully');
				// Restart monitoring with new expiry
				this.startMonitoring();
			} else {
				logger.warn('Failed to continue session, logging out');
				auth.logout();
				if (typeof window !== 'undefined') {
					window.location.href = '/auth/login?expired=true';
				}
			}
			return success;
		},

		/**
		 * Dismiss the warning (user chose to logout)
		 */
		dismiss(): void {
			stopCountdown();
		}
	};
}

export const sessionExpiry = createSessionExpiryStore();

// Derived store for easy access to warning state
export const showSessionWarning = derived(sessionExpiry, ($se) => $se.showWarning);
export const sessionSecondsRemaining = derived(sessionExpiry, ($se) => $se.secondsRemaining);
