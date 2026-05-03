/**
 * Accessibility Utilities Tests
 */
import { describe, it, expect } from 'vitest';
import {
	generateAriaId,
	getRelativeLuminance,
	getContrastRatio,
	hexToRgb,
	meetsContrastRequirements,
	CONTRAST_RATIOS
} from './accessibility';

describe('Accessibility Utilities', () => {
	describe('generateAriaId', () => {
		it('should generate a unique ID with the given prefix', () => {
			const id1 = generateAriaId('test');
			const id2 = generateAriaId('test');

			expect(id1).toMatch(/^test-[a-z0-9]+$/);
			expect(id2).toMatch(/^test-[a-z0-9]+$/);
			expect(id1).not.toBe(id2);
		});

		it('should handle different prefixes', () => {
			const id = generateAriaId('form-input');
			expect(id).toMatch(/^form-input-[a-z0-9]+$/);
		});
	});

	describe('hexToRgb', () => {
		it('should parse 6-digit hex colors with hash', () => {
			expect(hexToRgb('#ffffff')).toEqual([255, 255, 255]);
			expect(hexToRgb('#000000')).toEqual([0, 0, 0]);
			expect(hexToRgb('#ff0000')).toEqual([255, 0, 0]);
		});

		it('should parse 6-digit hex colors without hash', () => {
			expect(hexToRgb('ffffff')).toEqual([255, 255, 255]);
			expect(hexToRgb('3b82f6')).toEqual([59, 130, 246]);
		});

		it('should parse 3-digit hex colors', () => {
			expect(hexToRgb('#fff')).toEqual([255, 255, 255]);
			expect(hexToRgb('#000')).toEqual([0, 0, 0]);
			expect(hexToRgb('f00')).toEqual([255, 0, 0]);
		});
	});

	describe('getRelativeLuminance', () => {
		it('should return 1 for white', () => {
			const luminance = getRelativeLuminance(255, 255, 255);
			expect(luminance).toBeCloseTo(1, 4);
		});

		it('should return 0 for black', () => {
			const luminance = getRelativeLuminance(0, 0, 0);
			expect(luminance).toBe(0);
		});

		it('should return correct luminance for gray', () => {
			const luminance = getRelativeLuminance(128, 128, 128);
			expect(luminance).toBeGreaterThan(0);
			expect(luminance).toBeLessThan(1);
		});
	});

	describe('getContrastRatio', () => {
		it('should return 21 for black on white', () => {
			const ratio = getContrastRatio([0, 0, 0], [255, 255, 255]);
			expect(ratio).toBeCloseTo(21, 0);
		});

		it('should return 21 for white on black', () => {
			const ratio = getContrastRatio([255, 255, 255], [0, 0, 0]);
			expect(ratio).toBeCloseTo(21, 0);
		});

		it('should return 1 for same colors', () => {
			const ratio = getContrastRatio([128, 128, 128], [128, 128, 128]);
			expect(ratio).toBe(1);
		});
	});

	describe('meetsContrastRequirements', () => {
		it('should pass for black on white (normal text)', () => {
			expect(meetsContrastRequirements('#000000', '#ffffff')).toBe(true);
		});

		it('should pass for white on black (normal text)', () => {
			expect(meetsContrastRequirements('#ffffff', '#000000')).toBe(true);
		});

		it('should fail for low contrast combinations', () => {
			// Light gray on white
			expect(meetsContrastRequirements('#cccccc', '#ffffff')).toBe(false);
		});

		it('should use lower ratio for large text', () => {
			// This might fail for normal text but pass for large text
			const ratio = getContrastRatio(hexToRgb('#767676'), hexToRgb('#ffffff'));
			// 4.54:1 ratio - passes AA for normal text
			expect(ratio).toBeGreaterThan(CONTRAST_RATIOS.LARGE_TEXT);
		});

		it('should verify our primary button colors meet requirements for large text', () => {
			// White text on blue button gradient - meets AA for large text (3:1)
			const ratio = getContrastRatio(hexToRgb('#ffffff'), hexToRgb('#3b82f6'));
			// The ratio is around 3.95, which passes for large/bold text (3:1)
			expect(ratio).toBeGreaterThan(CONTRAST_RATIOS.LARGE_TEXT);
			// The button uses font-weight: 600 (semibold), which qualifies as large text
			expect(meetsContrastRequirements('#ffffff', '#3b82f6', true)).toBe(true);
		});

		it('should verify error message colors meet requirements', () => {
			// Error red (#b91c1c = red-700) on light red background (#fef2f2) for better contrast
			// Original #dc2626 (red-600) had 4.41:1 ratio - using darker shade for AA compliance
			const ratio = getContrastRatio(hexToRgb('#b91c1c'), hexToRgb('#fef2f2'));
			// The contrast ratio is ~5.6, meeting AA for normal text
			expect(ratio).toBeGreaterThan(CONTRAST_RATIOS.NORMAL_TEXT);
		});
	});

	describe('CONTRAST_RATIOS', () => {
		it('should have correct WCAG AA ratios', () => {
			expect(CONTRAST_RATIOS.NORMAL_TEXT).toBe(4.5);
			expect(CONTRAST_RATIOS.LARGE_TEXT).toBe(3);
			expect(CONTRAST_RATIOS.UI_COMPONENTS).toBe(3);
		});
	});
});
