import { describe, it, expect } from 'vitest';
import { describeCron, describeSchedule, parseSimple, buildSimple, type SimpleSchedule } from './schedule';

// The losslessness contract: everything the simple editor can EMIT must parse
// back to the same shape, and everything it can't hold must return null —
// never a silent rewrite. (Shell plan slice 13; a live agent once destroyed
// its own binding fumbling raw cron, so this boundary is load-bearing.)

const SHAPES: SimpleSchedule[] = [
	{ kind: 'daily', hour: 8, minute: 0 },
	{ kind: 'daily', hour: 23, minute: 45 },
	{ kind: 'weekdays', hour: 7, minute: 0 },
	{ kind: 'weekends', hour: 10, minute: 30 },
	{ kind: 'weekly', dow: 0, hour: 9, minute: 15 },
	{ kind: 'weekly', dow: 6, hour: 18, minute: 0 },
	{ kind: 'minutes', n: 30 },
	{ kind: 'minutes', n: 1 },
	{ kind: 'hours', n: 4 }
];

describe('buildSimple ↔ parseSimple round-trip', () => {
	for (const shape of SHAPES) {
		it(`round-trips ${JSON.stringify(shape)}`, () => {
			expect(parseSimple(buildSimple(shape))).toEqual(shape);
		});
	}

	it('round-trips through a second build (idempotent)', () => {
		for (const shape of SHAPES) {
			const cron = buildSimple(shape);
			expect(buildSimple(parseSimple(cron)!)).toBe(cron);
		}
	});
});

describe('parseSimple accepts the field-count and naming variants in the wild', () => {
	it('5-field legacy crons', () => {
		expect(parseSimple('0 7 * * *')).toEqual({ kind: 'daily', hour: 7, minute: 0 });
		expect(parseSimple('30 9 * * 1-5')).toEqual({ kind: 'weekdays', hour: 9, minute: 30 });
	});
	it('named DOW as the canvas emits it', () => {
		expect(parseSimple('0 0 7 * * Mon-Fri')).toEqual({ kind: 'weekdays', hour: 7, minute: 0 });
		expect(parseSimple('0 0 9 * * SAT,SUN')).toEqual(
			// SAT,SUN normalizes to 6,0
			{ kind: 'weekends', hour: 9, minute: 0 }
		);
	});
	it('7 for Sunday (both conventions)', () => {
		expect(parseSimple('0 15 9 * * 7')).toEqual({ kind: 'weekly', dow: 0, hour: 9, minute: 15 });
	});
});

describe('parseSimple refuses what the simple shapes cannot hold', () => {
	const inexpressible = [
		'54 48 18 2 8 * 2026', // year-pinned one-shot (the real leaked cron)
		'0 0 7 1 * *', // day-of-month
		'0 0 7 * 3 *', // month-specific
		'0 0/15 9-17 * * 1-5', // ranges + step combos
		'0 0 7 * * 1,3,5', // DOW list
		'garbage',
		''
	];
	for (const cron of inexpressible) {
		it(`returns null for "${cron}"`, () => {
			expect(parseSimple(cron)).toBeNull();
		});
	}
});

describe('describeSchedule never lies', () => {
	it('renders plain English for simple shapes', () => {
		expect(describeCron('0 0 8 * * *')).toBe('Every day at 8:00 AM');
		expect(describeSchedule('0 0 7 * * 1-5').isCron).toBe(false);
	});
	it('falls back to honest raw cron for one-shots and complex shapes', () => {
		const oneShot = describeSchedule('54 48 18 2 8 * 2026');
		expect(oneShot.isCron).toBe(true);
		expect(oneShot.text).toBe('54 48 18 2 8 * 2026');
	});
});
