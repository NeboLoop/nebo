// "Charged to your plan": what the plan's pool (or purchased credit) paid for
// this month, in dollars. Only AI work is a percentage of the plan; it is
// never a line here, and the plan's internal rate is never shown.
import type { PlanChargeLine } from '$lib/api/neboComponents';

/** The i18n key and values naming a line, by its kind. */
export interface ChargeText {
	key: string;
	values?: Record<string, string | number>;
	/** No words for this kind: show the hub's English label as is. */
	raw?: string;
}

/** +15551234567 → (555) 123-4567; any other number stays as the hub sent it. */
export function displayNumber(e164: string): string {
	const m = /^\+1(\d{3})(\d{3})(\d{4})$/.exec(e164.trim());
	return m ? `(${m[1]}) ${m[2]}-${m[3]}` : e164.trim();
}

export function chargeText(line: PlanChargeLine): ChargeText {
	const k = 'settingsUsage.charges.';
	const detail = (line.detail ?? '').trim();
	switch (line.kind) {
		case 'phone_number':
			return detail ? { key: k + 'phoneNumber', values: { number: displayNumber(detail) } } : { key: k + 'phoneNumberPlain' };
		case 'calls':
			return { key: k + 'calls', values: { minutes: line.quantity } };
		case 'texts':
			return { key: k + 'texts', values: { count: line.quantity } };
		case 'texting_registration':
			return { key: k + 'textingRegistration' };
		case 'texting_monthly':
			return { key: k + 'textingMonthly' };
		case 'cloud_computer':
			return detail ? { key: k + 'cloudComputerNamed', values: { name: detail } } : { key: k + 'cloudComputer' };
		case 'memory':
			return { key: k + 'memory' };
		case 'media':
			return { key: k + 'media' };
		default:
			return { key: '', raw: line.label || line.kind };
	}
}

/** Where the money came from: the plan's pool, or credit the customer bought. */
export function sourceKey(line: PlanChargeLine): string {
	return line.source === 'credit' ? 'settingsUsage.charges.fromCredit' : 'settingsUsage.charges.fromPlan';
}

/** Cents as dollars, in the reader's locale. */
export function formatCents(cents: number, locale?: string | null): string {
	return new Intl.NumberFormat(locale || undefined, { style: 'currency', currency: 'USD' }).format((cents || 0) / 100);
}

/** A line's date, short, in the reader's locale. */
export function formatChargeDate(iso: string, locale?: string | null): string {
	const d = new Date(iso);
	if (Number.isNaN(d.getTime())) return '';
	return d.toLocaleDateString(locale || undefined, { month: 'short', day: 'numeric' });
}

/** Another page is there to load. */
export function hasMore(shown: number, count: number): boolean {
	return shown < count;
}
