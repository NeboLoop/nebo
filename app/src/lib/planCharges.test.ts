import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import type { PlanChargeLine } from '$lib/api/neboComponents';
import { chargeText, displayNumber, formatCents, formatChargeDate, hasMore, sourceKey } from './planCharges';

const line = (over: Partial<PlanChargeLine>): PlanChargeLine => ({
	id: '1', kind: 'memory', detail: '', quantity: 1, amountCents: 100, source: 'plan', refId: '', label: '', at: '2026-10-05T10:00:00Z',
	...over
});

const en = JSON.parse(readFileSync(join(__dirname, 'i18n/locales/en.json'), 'utf8'));
const has = (key: string) => key.split('.').reduce<unknown>((o, k) => (o as Record<string, unknown>)?.[k], en) !== undefined;

describe('Charged to your plan', () => {
	it('names every kind the hub sends, with its number, minutes, count or computer', () => {
		expect(chargeText(line({ kind: 'phone_number', detail: '+15551234567' }))).toEqual({
			key: 'settingsUsage.charges.phoneNumber', values: { number: '(555) 123-4567' }
		});
		expect(chargeText(line({ kind: 'calls', quantity: 42 })).values).toEqual({ minutes: 42 });
		expect(chargeText(line({ kind: 'texts', quantity: 120 })).values).toEqual({ count: 120 });
		expect(chargeText(line({ kind: 'cloud_computer', detail: 'Ada' })).values).toEqual({ name: 'Ada' });
		for (const kind of ['phone_number', 'calls', 'texts', 'texting_registration', 'texting_monthly', 'cloud_computer', 'memory', 'media']) {
			const t = chargeText(line({ kind }));
			expect(t.raw).toBeUndefined();
			expect(has(t.key), t.key).toBe(true);
		}
	});

	it('shows the hub’s own words for a kind it does not know', () => {
		expect(chargeText(line({ kind: 'storage', label: 'Storage · 10 GB' }))).toEqual({ key: '', raw: 'Storage · 10 GB' });
	});

	it('says where the money came from', () => {
		expect(sourceKey(line({ source: 'plan' }))).toBe('settingsUsage.charges.fromPlan');
		expect(sourceKey(line({ source: 'credit' }))).toBe('settingsUsage.charges.fromCredit');
		expect(has('settingsUsage.charges.fromPlan') && has('settingsUsage.charges.fromCredit')).toBe(true);
	});

	it('formats dollars and dates, and knows when there is more', () => {
		expect(formatCents(12345, 'en-US')).toBe('$123.45');
		expect(formatCents(84, 'de-DE')).toContain('0,84');
		expect(formatChargeDate('2026-10-05T10:00:00Z', 'en-US')).toBe('Oct 5');
		expect(formatChargeDate('nope', 'en-US')).toBe('');
		expect(displayNumber('+442071234567')).toBe('+442071234567');
		expect(hasMore(50, 51)).toBe(true);
		expect(hasMore(51, 51)).toBe(false);
	});
});
