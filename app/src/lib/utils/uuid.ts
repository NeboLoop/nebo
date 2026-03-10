/**
 * Generate a UUID v4
 * Uses crypto.randomUUID() if available, falls back to manual generation
 */
export function generateUUID(): string {
	// Use native if available (modern browsers with secure context)
	if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
		return crypto.randomUUID();
	}

	// Fallback: generate UUID v4 manually
	// This works in all browsers including older ones and non-secure contexts
	return 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, (c) => {
		const r = (Math.random() * 16) | 0;
		const v = c === 'x' ? r : (r & 0x3) | 0x8;
		return v.toString(16);
	});
}
