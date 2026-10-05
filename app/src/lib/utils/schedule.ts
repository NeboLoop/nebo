/**
 * Cron → plain English, for DISPLAY. One home for the translation so every
 * surface (flows list, chain trigger card, reminders) says "Every day at
 * 8:00 AM" instead of leaking `0 0 8 * * *` at the owner.
 *
 * Returns null for anything it can't express faithfully — callers show the
 * raw cron in mono then. Never guesses: a wrong English rendering of a
 * schedule is worse than an honest cron.
 *
 * Accepts 5/6/7 field crons (the server's normalize_cron emits 7:
 * sec min hour dom mon dow year).
 */

import { get } from 'svelte/store';
import { t } from 'svelte-i18n';

/** Cron day-of-week order (0 = Sunday), as the `weekdays` message keys. */
const DOW_KEYS = ['sun', 'mon', 'tue', 'wed', 'thu', 'fri', 'sat'];
const DOW_ALIAS: Record<string, string> = {
	SUN: '0', MON: '1', TUE: '2', WED: '3', THU: '4', FRI: '5', SAT: '6'
};

function normDow(f: string): string {
	let v = f.toUpperCase();
	for (const [k, n] of Object.entries(DOW_ALIAS)) v = v.replaceAll(k, n);
	return v.replace(/7/g, '0'); // both conventions for Sunday
}

function timeOf(minF: string, hourF: string): string | null {
	if (!/^\d{1,2}$/.test(minF) || !/^\d{1,2}$/.test(hourF)) return null;
	const m = +minF, h = +hourF;
	if (m > 59 || h > 23) return null;
	return clockTime(h, m);
}

/** "8:00 AM" — the locale's wording of a wall-clock time. Exported for the
 *  other time-of-day labels so there is one way to say a time. */
export function clockTime(h: number, m: number): string {
	const tr = get(t);
	return tr('schedule.clockTime', {
		values: {
			hour: h % 12 === 0 ? 12 : h % 12,
			minute: String(m).padStart(2, '0'),
			period: tr(h >= 12 ? 'schedule.pm' : 'schedule.am')
		}
	});
}

export function describeCron(cron: string): string | null {
	const tr = get(t);
	const fields = cron.trim().split(/\s+/);
	if (fields.length < 5 || fields.length > 7) return null;

	// Normalize to [sec, min, hour, dom, mon, dow]; a 7th field is the year.
	let sec = '0', min: string, hour: string, dom: string, mon: string, dow: string, year = '*';
	if (fields.length === 5) [min, hour, dom, mon, dow] = fields;
	else if (fields.length === 6) [sec, min, hour, dom, mon, dow] = fields;
	else [sec, min, hour, dom, mon, dow, year] = fields;

	if (sec !== '0' && sec !== '*') return null;
	dow = normDow(dow);

	// A pinned year is a one-shot timer — inexpressible as a recurrence.
	if (year !== '*') return null;
	if (mon !== '*') return null; // month-specific → raw cron is more honest

	// Interval forms: every N minutes / hours.
	if (/^\*\/(\d+)$/.test(min) && hour === '*' && dom === '*' && dow === '*') {
		const n = +min.match(/^\*\/(\d+)$/)![1];
		return tr('schedule.cronEveryMinutes', { values: { n } });
	}
	if (/^\*\/(\d+)$/.test(hour) && /^\d{1,2}$/.test(min) && dom === '*' && dow === '*') {
		const n = +hour.match(/^\*\/(\d+)$/)![1];
		if (+min === 0) return tr('schedule.cronEveryHours', { values: { n } });
		return tr('schedule.cronEveryHoursAt', { values: { n, minute: String(+min).padStart(2, '0') } });
	}

	// Fixed time-of-day forms.
	const time = timeOf(min, hour);
	if (!time) return null;

	if (dom === '*' && dow === '*') return tr('schedule.cronDailyAt', { values: { time } });
	if (dom === '*' && dow === '1-5') return tr('automations.weekdaysAt', { values: { time } });
	if (dom === '*' && (dow === '0,6' || dow === '6,0')) return tr('automations.weekendsAt', { values: { time } });
	if (dom === '*' && /^\d$/.test(dow) && DOW_KEYS[+dow]) {
		const days = tr('schedule.weekdayPlural', { values: { day: DOW_KEYS[+dow] } });
		return tr('schedule.cronDaysAt', { values: { days, time } });
	}
	if (dom === '*' && /^\d(,\d)+$/.test(dow)) {
		const keys = dow.split(',').map((d) => DOW_KEYS[+d]).filter(Boolean);
		if (keys.length !== dow.split(',').length) return null;
		const days = keys.map((k) => tr(`weekdays.${k}`)).join(tr('schedule.listSeparator'));
		return tr('schedule.cronDaysAt', { values: { days, time } });
	}
	if (dow === '*' && /^\d{1,2}$/.test(dom) && +dom >= 1 && +dom <= 31) {
		return tr('schedule.cronMonthlyAt', { values: { day: +dom, time } });
	}
	return null;
}

/** Best display form for a schedule: English when faithful, the cron itself
 *  otherwise. `looksCron` lets callers decide monospace styling. */
export function describeSchedule(raw: string | undefined | null): { text: string; isCron: boolean } {
	const s = (raw ?? '').trim();
	if (!s) return { text: '', isCron: false };
	const asCron = describeCron(s);
	if (asCron) return { text: asCron, isCron: false };
	// Already human (came from the server's own describer)?
	const cronish = /^[\d*,/\-A-Za-z]+(\s+[\d*,/\-A-Za-z]+){4,6}$/.test(s) && /[*\/]/.test(s);
	return { text: s, isCron: cronish };
}

/**
 * The simple schedule shapes the inline editor can hold. parseSimple returns
 * null for anything else — those schedules render read-only rather than being
 * rewritten into something they never said.
 */
export type SimpleSchedule =
	| { kind: 'daily'; hour: number; minute: number }
	| { kind: 'weekdays'; hour: number; minute: number }
	| { kind: 'weekends'; hour: number; minute: number }
	| { kind: 'weekly'; dow: number; hour: number; minute: number }
	| { kind: 'hours'; n: number }
	| { kind: 'minutes'; n: number };

export function parseSimple(cron: string): SimpleSchedule | null {
	const f = cron.trim().split(/\s+/);
	if (f.length < 5 || f.length > 7) return null;
	let sec = '0', min: string, hour: string, dom: string, mon: string, dow: string, year = '*';
	if (f.length === 5) [min, hour, dom, mon, dow] = f;
	else if (f.length === 6) [sec, min, hour, dom, mon, dow] = f;
	else [sec, min, hour, dom, mon, dow, year] = f;
	if ((sec !== '0' && sec !== '*') || mon !== '*' || year !== '*' || dom !== '*') return null;
	// Named DOW (Mon-Fri) is what the canvas emits — normalize like describeCron.
	dow = normDow(dow);

	let m: RegExpMatchArray | null;
	if ((m = min.match(/^\*\/(\d+)$/)) && hour === '*' && dow === '*') return { kind: 'minutes', n: +m[1] };
	if ((m = hour.match(/^\*\/(\d+)$/)) && min === '0' && dow === '*') return { kind: 'hours', n: +m[1] };
	if (!/^\d{1,2}$/.test(min) || !/^\d{1,2}$/.test(hour)) return null;
	const base = { hour: +hour, minute: +min };
	if (dow === '*') return { kind: 'daily', ...base };
	if (dow === '1-5') return { kind: 'weekdays', ...base };
	if (dow === '0,6' || dow === '6,0') return { kind: 'weekends', ...base };
	if (/^\d$/.test(dow)) return { kind: 'weekly', dow: +dow, ...base };
	return null;
}

/** Emits the event tool's 6-field form (sec min hour dom mon dow). */
export function buildSimple(s: SimpleSchedule): string {
	switch (s.kind) {
		case 'minutes':
			return `0 */${s.n} * * * *`;
		case 'hours':
			return `0 0 */${s.n} * * *`;
		case 'weekly':
			return `0 ${s.minute} ${s.hour} * * ${s.dow}`;
		case 'weekdays':
			return `0 ${s.minute} ${s.hour} * * 1-5`;
		case 'weekends':
			return `0 ${s.minute} ${s.hour} * * 0,6`;
		default:
			return `0 ${s.minute} ${s.hour} * * *`;
	}
}
