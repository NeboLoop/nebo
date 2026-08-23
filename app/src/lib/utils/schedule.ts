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

const DOW_NAMES = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];
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
	const ampm = h >= 12 ? 'PM' : 'AM';
	const h12 = h % 12 === 0 ? 12 : h % 12;
	return `${h12}:${String(m).padStart(2, '0')} ${ampm}`;
}

export function describeCron(cron: string): string | null {
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
		return n === 1 ? 'Every minute' : `Every ${n} minutes`;
	}
	if (/^\*\/(\d+)$/.test(hour) && /^\d{1,2}$/.test(min) && dom === '*' && dow === '*') {
		const n = +hour.match(/^\*\/(\d+)$/)![1];
		const at = +min === 0 ? '' : ` at :${String(+min).padStart(2, '0')}`;
		return n === 1 ? `Every hour${at}` : `Every ${n} hours${at}`;
	}

	// Fixed time-of-day forms.
	const time = timeOf(min, hour);
	if (!time) return null;

	if (dom === '*' && dow === '*') return `Every day at ${time}`;
	if (dom === '*' && dow === '1-5') return `Weekdays at ${time}`;
	if (dom === '*' && (dow === '0,6' || dow === '6,0')) return `Weekends at ${time}`;
	if (dom === '*' && /^\d$/.test(dow)) return `${DOW_NAMES[+dow]}s at ${time}`;
	if (dom === '*' && /^\d(,\d)+$/.test(dow)) {
		const days = dow.split(',').map((d) => DOW_NAMES[+d]?.slice(0, 3)).filter(Boolean);
		if (days.length !== dow.split(',').length) return null;
		return `${days.join(', ')} at ${time}`;
	}
	if (dow === '*' && /^\d{1,2}$/.test(dom) && +dom >= 1 && +dom <= 31) {
		const d = +dom;
		const suffix = d % 10 === 1 && d !== 11 ? 'st' : d % 10 === 2 && d !== 12 ? 'nd' : d % 10 === 3 && d !== 13 ? 'rd' : 'th';
		return `Monthly on the ${d}${suffix} at ${time}`;
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
