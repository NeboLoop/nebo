/**
 * Accessibility Utilities
 *
 * This module provides utility functions for accessibility testing and
 * common accessibility patterns used throughout the application.
 */

/**
 * Generates a unique ID for ARIA relationships.
 * Use this to create consistent IDs for aria-labelledby, aria-describedby, etc.
 */
export function generateAriaId(prefix: string): string {
	return `${prefix}-${Math.random().toString(36).substr(2, 9)}`;
}

/**
 * Checks if an element should be visible to screen readers only.
 * Returns CSS classes for visually hidden but accessible content.
 */
export const VISUALLY_HIDDEN_STYLES = {
	position: 'absolute',
	width: '1px',
	height: '1px',
	padding: '0',
	margin: '-1px',
	overflow: 'hidden',
	clip: 'rect(0, 0, 0, 0)',
	whiteSpace: 'nowrap',
	border: '0'
} as const;

/**
 * Creates an announcement for screen readers using a live region.
 * @param message - The message to announce
 * @param priority - 'polite' for non-urgent announcements, 'assertive' for urgent
 */
export function announceToScreenReader(
	message: string,
	priority: 'polite' | 'assertive' = 'polite'
): void {
	if (typeof document === 'undefined') return;

	// Find or create the live region container
	let liveRegion = document.getElementById('sr-announcements');

	if (!liveRegion) {
		liveRegion = document.createElement('div');
		liveRegion.id = 'sr-announcements';
		liveRegion.setAttribute('aria-live', priority);
		liveRegion.setAttribute('aria-atomic', 'true');
		liveRegion.setAttribute('role', 'status');
		Object.assign(liveRegion.style, VISUALLY_HIDDEN_STYLES);
		document.body.appendChild(liveRegion);
	}

	// Clear and set the message (the brief delay ensures screen readers pick up the change)
	liveRegion.textContent = '';
	setTimeout(() => {
		if (liveRegion) {
			liveRegion.textContent = message;
		}
	}, 100);
}

/**
 * Traps focus within a container element.
 * Useful for modals, dialogs, and other focus-trapped contexts.
 * @param container - The container element to trap focus within
 * @returns A cleanup function to remove the trap
 */
export function trapFocus(container: HTMLElement): () => void {
	const focusableElements = container.querySelectorAll<HTMLElement>(
		'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])'
	);

	const firstFocusable = focusableElements[0];
	const lastFocusable = focusableElements[focusableElements.length - 1];

	function handleKeyDown(e: KeyboardEvent) {
		if (e.key !== 'Tab') return;

		if (e.shiftKey) {
			if (document.activeElement === firstFocusable) {
				e.preventDefault();
				lastFocusable?.focus();
			}
		} else {
			if (document.activeElement === lastFocusable) {
				e.preventDefault();
				firstFocusable?.focus();
			}
		}
	}

	container.addEventListener('keydown', handleKeyDown);

	// Focus the first focusable element
	firstFocusable?.focus();

	return () => {
		container.removeEventListener('keydown', handleKeyDown);
	};
}

/**
 * WCAG 2.1 AA minimum contrast ratios
 */
export const CONTRAST_RATIOS = {
	/** Minimum for normal text */
	NORMAL_TEXT: 4.5,
	/** Minimum for large text (18pt or 14pt bold) */
	LARGE_TEXT: 3,
	/** Minimum for UI components and graphical objects */
	UI_COMPONENTS: 3
} as const;

/**
 * Calculates the relative luminance of a color.
 * @param r - Red value (0-255)
 * @param g - Green value (0-255)
 * @param b - Blue value (0-255)
 * @returns Relative luminance value
 */
export function getRelativeLuminance(r: number, g: number, b: number): number {
	const [rs, gs, bs] = [r, g, b].map((c) => {
		const sRGB = c / 255;
		return sRGB <= 0.03928 ? sRGB / 12.92 : Math.pow((sRGB + 0.055) / 1.055, 2.4);
	});
	return 0.2126 * rs + 0.7152 * gs + 0.0722 * bs;
}

/**
 * Calculates the contrast ratio between two colors.
 * @param color1 - First color as [r, g, b] array
 * @param color2 - Second color as [r, g, b] array
 * @returns Contrast ratio (1 to 21)
 */
export function getContrastRatio(
	color1: [number, number, number],
	color2: [number, number, number]
): number {
	const lum1 = getRelativeLuminance(...color1);
	const lum2 = getRelativeLuminance(...color2);
	const lighter = Math.max(lum1, lum2);
	const darker = Math.min(lum1, lum2);
	return (lighter + 0.05) / (darker + 0.05);
}

/**
 * Parses a hex color to RGB array.
 * @param hex - Hex color string (with or without #)
 * @returns RGB array [r, g, b]
 */
export function hexToRgb(hex: string): [number, number, number] {
	const cleanHex = hex.replace('#', '');
	const fullHex =
		cleanHex.length === 3
			? cleanHex
					.split('')
					.map((c) => c + c)
					.join('')
			: cleanHex;

	const r = parseInt(fullHex.substring(0, 2), 16);
	const g = parseInt(fullHex.substring(2, 4), 16);
	const b = parseInt(fullHex.substring(4, 6), 16);

	return [r, g, b];
}

/**
 * Checks if a color combination meets WCAG AA contrast requirements.
 * @param foreground - Foreground color as hex string
 * @param background - Background color as hex string
 * @param isLargeText - Whether the text is large (18pt or 14pt bold)
 * @returns Whether the combination meets AA requirements
 */
export function meetsContrastRequirements(
	foreground: string,
	background: string,
	isLargeText = false
): boolean {
	const ratio = getContrastRatio(hexToRgb(foreground), hexToRgb(background));
	const minRatio = isLargeText ? CONTRAST_RATIOS.LARGE_TEXT : CONTRAST_RATIOS.NORMAL_TEXT;
	return ratio >= minRatio;
}

/**
 * Keyboard navigation helpers for common patterns.
 */
export const KeyboardHandlers = {
	/**
	 * Creates a handler for arrow key navigation within a group of elements.
	 * @param elements - Array of elements to navigate between
	 * @param options - Configuration options
	 */
	createArrowNavigation: (
		elements: HTMLElement[],
		options: {
			orientation?: 'horizontal' | 'vertical' | 'both';
			loop?: boolean;
		} = {}
	) => {
		const { orientation = 'horizontal', loop = true } = options;

		return (event: KeyboardEvent) => {
			const currentIndex = elements.findIndex((el) => el === document.activeElement);
			if (currentIndex === -1) return;

			let nextIndex = currentIndex;
			const isHorizontal = orientation === 'horizontal' || orientation === 'both';
			const isVertical = orientation === 'vertical' || orientation === 'both';

			if (
				(event.key === 'ArrowRight' && isHorizontal) ||
				(event.key === 'ArrowDown' && isVertical)
			) {
				event.preventDefault();
				nextIndex = currentIndex + 1;
				if (nextIndex >= elements.length) {
					nextIndex = loop ? 0 : elements.length - 1;
				}
			} else if (
				(event.key === 'ArrowLeft' && isHorizontal) ||
				(event.key === 'ArrowUp' && isVertical)
			) {
				event.preventDefault();
				nextIndex = currentIndex - 1;
				if (nextIndex < 0) {
					nextIndex = loop ? elements.length - 1 : 0;
				}
			} else if (event.key === 'Home') {
				event.preventDefault();
				nextIndex = 0;
			} else if (event.key === 'End') {
				event.preventDefault();
				nextIndex = elements.length - 1;
			}

			elements[nextIndex]?.focus();
		};
	}
};

/**
 * Type definitions for accessibility-related props
 */
export interface A11yLabelProps {
	/** The accessible name for the element */
	'aria-label'?: string;
	/** Reference to the element that labels this element */
	'aria-labelledby'?: string;
	/** Reference to elements that describe this element */
	'aria-describedby'?: string;
}

export interface A11yStateProps {
	/** Whether the element is expanded (for expandable widgets) */
	'aria-expanded'?: boolean;
	/** Whether the element is selected */
	'aria-selected'?: boolean;
	/** Whether the element is pressed (for toggle buttons) */
	'aria-pressed'?: boolean | 'mixed';
	/** Whether the element is disabled */
	'aria-disabled'?: boolean;
	/** Whether the element is invalid */
	'aria-invalid'?: boolean;
	/** Whether the element is required */
	'aria-required'?: boolean;
	/** Whether the element is busy loading */
	'aria-busy'?: boolean;
}

export interface A11yLiveRegionProps {
	/** The live region behavior */
	'aria-live'?: 'off' | 'polite' | 'assertive';
	/** Whether the whole region should be announced */
	'aria-atomic'?: boolean;
	/** Which changes should be announced */
	'aria-relevant'?: 'additions' | 'removals' | 'text' | 'all';
}
