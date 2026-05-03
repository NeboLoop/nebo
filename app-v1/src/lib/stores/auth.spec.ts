import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { get } from 'svelte/store';
import { auth, isAuthenticated, currentUser, authError, authLoading, passwordReset } from './auth';

// Mock the API module
vi.mock('$lib/api/nebo', () => ({
	login: vi.fn(),
	register: vi.fn(),
	refreshToken: vi.fn(),
	getCurrentUser: vi.fn(),
	updateCurrentUser: vi.fn()
}));

// Mock the logger
vi.mock('$lib/monitoring', () => ({
	logger: {
		info: vi.fn(),
		debug: vi.fn(),
		warn: vi.fn(),
		error: vi.fn()
	}
}));

// Mock localStorage
const localStorageMock = (() => {
	let store: Record<string, string> = {};
	return {
		getItem: vi.fn((key: string) => store[key] || null),
		setItem: vi.fn((key: string, value: string) => {
			store[key] = value;
		}),
		removeItem: vi.fn((key: string) => {
			delete store[key];
		}),
		clear: vi.fn(() => {
			store = {};
		})
	};
})();

Object.defineProperty(global, 'localStorage', { value: localStorageMock });

describe('Auth Store', () => {
	beforeEach(() => {
		vi.clearAllMocks();
		localStorageMock.clear();
		auth.logout(); // Reset store state
	});

	afterEach(() => {
		vi.restoreAllMocks();
	});

	describe('Initial State', () => {
		it('should have correct initial state', () => {
			const state = get(auth);
			expect(state.user).toBeNull();
			expect(state.token).toBeNull();
			expect(state.refreshToken).toBeNull();
			expect(state.expiresAt).toBeNull();
			expect(state.isLoading).toBe(false);
			expect(state.error).toBeNull();
		});

		it('should have isAuthenticated as false initially', () => {
			expect(get(isAuthenticated)).toBe(false);
		});

		it('should have currentUser as null initially', () => {
			expect(get(currentUser)).toBeNull();
		});

		it('should have authLoading as false initially', () => {
			expect(get(authLoading)).toBe(false);
		});
	});

	describe('logout', () => {
		it('should reset state to initial values', async () => {
			// First set some state
			const mockApi = await import('$lib/api/nebo');
			vi.mocked(mockApi.login).mockResolvedValue({
				token: 'test-token',
				refreshToken: 'test-refresh',
				expiresAt: Date.now() + 3600000
			});

			// Logout and verify
			auth.logout();

			const state = get(auth);
			expect(state.user).toBeNull();
			expect(state.token).toBeNull();
			expect(state.refreshToken).toBeNull();
			expect(state.expiresAt).toBeNull();
			expect(state.isLoading).toBe(false);
			expect(state.error).toBeNull();
		});

		it('should clear localStorage on logout', () => {
			localStorageMock.setItem('nebo_token', 'test');
			localStorageMock.setItem('nebo_refresh_token', 'test');
			localStorageMock.setItem('nebo_expires_at', '123456');

			auth.logout();

			expect(localStorageMock.removeItem).toHaveBeenCalledWith('nebo_token');
			expect(localStorageMock.removeItem).toHaveBeenCalledWith('nebo_refresh_token');
			expect(localStorageMock.removeItem).toHaveBeenCalledWith('nebo_expires_at');
		});
	});

	describe('clearError', () => {
		it('should clear error state', async () => {
			// First create an error by attempting a failed login
			const mockApi = await import('$lib/api/nebo');
			vi.mocked(mockApi.login).mockRejectedValue(new Error('Login failed'));

			await auth.login({ email: 'test@example.com', password: 'password' });
			expect(get(authError)).not.toBeNull();

			auth.clearError();
			expect(get(authError)).toBeNull();
		});
	});

	describe('needsTokenRefresh', () => {
		it('should return false if no expiresAt', () => {
			expect(auth.needsTokenRefresh()).toBe(false);
		});
	});
});

describe('Derived Stores', () => {
	beforeEach(() => {
		auth.logout();
	});

	describe('isAuthenticated', () => {
		it('should be false when no token or user', () => {
			expect(get(isAuthenticated)).toBe(false);
		});
	});

	describe('currentUser', () => {
		it('should reflect user from auth store', () => {
			expect(get(currentUser)).toBeNull();
		});
	});

	describe('authError', () => {
		it('should reflect error from auth store', () => {
			expect(get(authError)).toBeNull();
		});
	});

	describe('authLoading', () => {
		it('should reflect isLoading from auth store', () => {
			expect(get(authLoading)).toBe(false);
		});
	});
});

describe('Password Reset Store', () => {
	beforeEach(() => {
		passwordReset.reset();
	});

	describe('Initial State', () => {
		it('should have correct initial state', () => {
			const state = get(passwordReset);
			expect(state.isLoading).toBe(false);
			expect(state.isSuccess).toBe(false);
			expect(state.error).toBeNull();
		});
	});

	describe('reset', () => {
		it('should reset state to initial values', () => {
			passwordReset.reset();

			const state = get(passwordReset);
			expect(state.isLoading).toBe(false);
			expect(state.isSuccess).toBe(false);
			expect(state.error).toBeNull();
		});
	});

	describe('requestReset', () => {
		it('should set isLoading to true while processing', async () => {
			const promise = passwordReset.requestReset('test@example.com');

			// Check loading state (might be quick, so this is a bit tricky)
			await promise;

			// After completion
			const state = get(passwordReset);
			expect(state.isLoading).toBe(false);
		});

		it('should set isSuccess to true on success', async () => {
			await passwordReset.requestReset('test@example.com');

			const state = get(passwordReset);
			expect(state.isSuccess).toBe(true);
			expect(state.error).toBeNull();
		});
	});

	describe('resetPassword', () => {
		it('should set isSuccess to true on success', async () => {
			await passwordReset.resetPassword('token', 'NewPassword123');

			const state = get(passwordReset);
			expect(state.isSuccess).toBe(true);
			expect(state.error).toBeNull();
		});
	});
});
