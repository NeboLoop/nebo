// A rule says when the owner said yes on his own clock then (the offset the
// server recorded with it), whatever zone this browser is in; the ceiling
// reads and writes in dollars.
import { describe, it, expect } from 'vitest';
import { formatMoment, cents, dollars } from './rules';

describe('rules', () => {
	it("words the consent moment on the owner's clock, with its zone", () => {
		const at = Date.parse('2026-10-14T15:02:00Z') / 1000;
		expect(formatMoment({ at, timeZone: 'America/Denver', zoneAbbr: 'MDT', offsetSeconds: -21600 }, 'en-US')).toBe('Wed, Oct 14, 2026, 9:02 AM MDT');
	});

	it('reads the ceiling in dollars; empty is no ceiling', () => {
		expect(cents('$500')).toBe(50000);
		expect(cents('1,250.50')).toBe(125050);
		expect(cents('')).toBeNull();
		expect(cents('abc')).toBeNull();
		expect(dollars(50000)).toBe('500');
		expect(dollars(null)).toBe('');
	});
});
