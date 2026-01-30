// Re-export all generated API functions
export * from './nebo';

// Import all functions and components
import * as api from './nebo';

// Export api object containing all API methods
export const nebo = api;

// API Configuration - base URL is loaded from browser origin
export const API_CONFIG = {
	get baseURL() {
		// In browser, use the current origin; in SSR, default to localhost
		if (typeof window !== 'undefined') {
			return window.location.origin;
		}
		return 'http://localhost:8847';
	}
};

// Re-export types
export type * from './neboComponents';
