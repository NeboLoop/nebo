// How a setup question is shown: its short label, the one-line hint under it,
// the control that fits its type, its unit, its default and its validation.
// The ONE place these rules live; AgentInputForm renders from them and the
// employee's Configure summary reads the same labels and units.

import type { AgentInputField } from '$lib/types/agentPage';

export type InputControl = 'number' | 'toggle' | 'select' | 'textarea' | 'path' | 'file' | 'text';

/** A label longer than this reads as the question, not a name. */
const SHORT_LABEL_MAX = 32;

/** The control that fits a field's declared type. */
export function controlFor(field: AgentInputField): InputControl {
	const type = (field.type || 'text').toLowerCase();
	if (type === 'number' || type === 'integer' || type === 'float') return 'number';
	if (type === 'checkbox' || type === 'boolean' || type === 'bool' || type === 'toggle') return 'toggle';
	if (type === 'select' || type === 'radio' || type === 'enum') return 'select';
	if (type === 'textarea') return 'textarea';
	if (type === 'path') return 'path';
	if (type === 'file') return 'file';
	if (Array.isArray(field.options) && field.options.length > 0) return 'select';
	return 'text';
}

function isQuestion(label: string): boolean {
	return label.length > SHORT_LABEL_MAX || label.trim().endsWith('?');
}

/** Trailing words a number field's unit suffix already shows. A singular
 *  `day` is a day of the month or week, not a duration, so it stays. */
const UNIT_WORDS = new Set([
	'days', 'hours', 'hrs', 'minutes', 'mins', 'weeks', 'week', 'months', 'month', 'years', 'year',
	'pct', 'percent', '%', 'usd', 'dollars', 'cents', 'count'
]);

/** Words before which a time word names a rate or a reference ("per week",
 *  "day of the month"), never the value's unit. */
const NOT_A_UNIT_BEFORE = new Set(['per', 'each', 'every', 'a', 'of', 'the']);

/** Abbreviations a key uses, as the words a person reads. */
const ABBREVIATIONS: Record<string, string> = {
	qty: 'quantity',
	num: 'number',
	amt: 'amount',
	pct: 'percent',
	acct: 'account',
	txn: 'transaction',
	max: 'maximum',
	min: 'minimum'
};

/** A money question: declared, or a key counted in cents. Never defaulted,
 *  never given a time unit. */
export function isMoney(field: AgentInputField): boolean {
	if (field.money) return true;
	const words = keyWords(field.key || '');
	return words[words.length - 1] === 'cents';
}

/** Kept uppercase inside a sentence-case name. */
const ACRONYMS = new Set([
	'sla', 'api', 'url', 'id', 'sms', 'crm', 'kpi', 'vat', 'ein', 'ach', 'ar', 'ap', 'qbo',
	'csv', 'pdf', 'mrr', 'arr', 'roi', 'cpc', 'cpa', 'sku', 'pos', 'hr', 'pto', 'eta', 'po'
]);

/** A key's words: its last path segment, split on _ - and camelCase. */
function keyWords(key: string): string[] {
	const last = key.split('.').pop() || key;
	return last
		.replace(/([a-z])([A-Z])/g, '$1 $2')
		.split(/[\s_-]+/)
		.map((w) => w.toLowerCase())
		.filter(Boolean);
}

/** A key as plain words, sentence case. For a number field the trailing unit
 *  words go (the suffix shows them):
 *  `feed_stall_days` → "Feed stall", `max_txn_amt` → "Maximum transaction amount". */
export function humanizeKey(key: string, isNumber: boolean): string {
	const words = keyWords(key);
	if (isNumber) {
		while (
			words.length > 1 &&
			UNIT_WORDS.has(words[words.length - 1]) &&
			!NOT_A_UNIT_BEFORE.has(words[words.length - 2])
		) {
			words.pop();
		}
	}
	const text = words
		// "no" is "number" only at the end (`invoice_no`), never in `no_show_policy`.
		.map((w, i) => (w === 'no' && i === words.length - 1 && i > 0 ? 'number' : (ABBREVIATIONS[w] ?? w)))
		.map((w) => (ACRONYMS.has(w) ? w.toUpperCase() : w))
		.join(' ');
	return text.charAt(0).toUpperCase() + text.slice(1);
}

/** The short name shown as the field's label. A label written as a long
 *  question gives way to a name made from the field's key. */
export function shortLabel(field: AgentInputField): string {
	const label = (field.label || '').trim();
	if (label && !isQuestion(label)) return label;
	const fromKey = humanizeKey(field.key || field.id || '', controlFor(field) === 'number');
	return fromKey || label;
}

/** The one-line muted hint under the label: the full question when the
 *  label was too long to be one, else the field's description. */
export function hintFor(field: AgentInputField): string {
	const label = (field.label || '').trim();
	if (label && isQuestion(label) && shortLabel(field) !== label) return label;
	return (field.description || '').trim();
}

/** Everything the field says about itself, for the hint's tooltip. */
export function fullHelp(field: AgentInputField): string {
	const hint = hintFor(field);
	const desc = (field.description || '').trim();
	return desc && desc !== hint ? `${hint}\n${desc}` : hint;
}

/** Units inferred from a question when the schema names none. The value is
 *  an i18n key under agentInputForm.units, or a literal symbol. */
// Matched against the key (its _ and - read as spaces) and the question.
// "per week", "each month", "day of the month" name a rate or a reference,
// not the unit of the value, so a time word after per/each/every/a/of/the is skipped.
const NOT_A_UNIT = '(?<!\\b(?:per|each|every|a|of|the) )';
const timeUnit = (words: string) => new RegExp(`${NOT_A_UNIT}\\b(?:${words})\\b`);
const INFERRED_UNITS: [RegExp, string][] = [
	[/%|\bpercent(age)?\b|\bpct\b/, '%'],
	[timeUnit('seconds?|secs'), 'units.seconds'],
	[timeUnit('minutes?|mins'), 'units.minutes'],
	[timeUnit('hours?|hrs'), 'units.hours'],
	// Only the plural is a duration: "close day" is a day of the month.
	[timeUnit('days'), 'units.days'],
	[timeUnit('weeks?'), 'units.weeks'],
	[timeUnit('months?'), 'units.months'],
	[timeUnit('years?'), 'units.years']
];

export type FieldUnit = { text: string } | { key: string } | null;

/** A number field's unit: the schema's `unit` when it declares one, else
 *  inferred from its key and question. Null when nothing names one. */
export function unitFor(field: AgentInputField): FieldUnit {
	if (controlFor(field) !== 'number') return null;
	if (field.unit && field.unit.trim()) return { text: field.unit.trim() };
	// A money amount is in a currency, never a time unit its question mentions.
	if (isMoney(field)) return null;
	const haystack = `${keyWords(field.key || '').join(' ')} ${field.label || ''} ${field.description || ''}`.toLowerCase();
	for (const [re, unit] of INFERRED_UNITS) {
		if (re.test(haystack)) return unit.startsWith('units.') ? { key: unit } : { text: unit };
	}
	return null;
}

/** The value a form opens with: what is saved, else the field's default
 *  (a money question never has one), else nothing. */
export function withDefaults(
	fields: AgentInputField[],
	saved: Record<string, unknown>
): Record<string, unknown> {
	const out: Record<string, unknown> = { ...saved };
	for (const f of fields) {
		const v = out[f.key];
		if (v !== undefined && v !== null && v !== '') continue;
		if (isMoney(f) || f.default === undefined || f.default === null || f.default === '') continue;
		out[f.key] = f.default;
	}
	return out;
}

/** Units whose values are never negative; % also tops out at 100. */
const NON_NEGATIVE_UNITS = new Set(['units.days', 'units.hours', 'units.minutes', '%']);
const NON_NEGATIVE_TEXT = new Set(['day', 'days', 'hour', 'hours', 'hr', 'hrs', 'minute', 'minutes', 'min', 'mins', 'count', '%', 'percent', 'pct']);

/** `close_day`, `due_day`, `day_of_month`: a day of the month (not of the week). */
function isDayOfMonth(words: string[]): boolean {
	if (words.includes('week') || words.includes('weekday')) return false;
	return words[words.length - 1] === 'day' || (words.includes('day') && words.includes('month'));
}

/** A number field's bounds: what the schema declares, else sensible ones —
 *  a count of days, hours, minutes or things starts at 0, and a % runs 0–100. */
export function boundsFor(field: AgentInputField): { min?: number; max?: number } {
	if (controlFor(field) !== 'number') return {};
	const unit = unitFor(field);
	const name = unit === null ? '' : 'key' in unit ? unit.key : unit.text.toLowerCase();
	const isPercent = name === '%' || name === 'percent' || name === 'pct';
	const words = keyWords(field.key || '');
	const isCount = words.includes('count');
	if (unit === null && isDayOfMonth(words)) {
		return {
			min: typeof field.min === 'number' ? field.min : 1,
			max: typeof field.max === 'number' ? field.max : 31
		};
	}
	const nonNegative = isCount || NON_NEGATIVE_UNITS.has(name) || NON_NEGATIVE_TEXT.has(name);
	return {
		min: typeof field.min === 'number' ? field.min : nonNegative ? 0 : undefined,
		max: typeof field.max === 'number' ? field.max : isPercent ? 100 : undefined
	};
}

export type FieldError = { key: string; values?: Record<string, number> };

function isEmpty(v: unknown): boolean {
	return v === undefined || v === null || (typeof v === 'string' && v.trim() === '');
}

/** The errors to show under each field: a missing required answer, a number
 *  that is not one, or one outside its bounds. Keys are agentInputForm.* i18n. */
export function validateInputs(
	fields: AgentInputField[],
	values: Record<string, unknown>
): Record<string, FieldError> {
	const errors: Record<string, FieldError> = {};
	for (const f of fields) {
		const v = values[f.key];
		const control = controlFor(f);
		if (control === 'toggle') continue;
		if (isEmpty(v)) {
			if (f.required) errors[f.key] = { key: 'agentInputForm.requiredError' };
			continue;
		}
		if (control === 'number') {
			const n = typeof v === 'number' ? v : Number(String(v).trim());
			const { min, max } = boundsFor(f);
			if (!Number.isFinite(n)) {
				errors[f.key] = { key: 'agentInputForm.numberError' };
			} else if (typeof min === 'number' && n < min) {
				errors[f.key] = { key: 'agentInputForm.minError', values: { min } };
			} else if (typeof max === 'number' && n > max) {
				errors[f.key] = { key: 'agentInputForm.maxError', values: { max } };
			}
		}
	}
	return errors;
}

/** One +/- press on a number field, kept inside its bounds. */
export function stepNumber(field: AgentInputField, current: unknown, direction: 1 | -1): number {
	const step = typeof field.step === 'number' && field.step > 0 ? field.step : 1;
	const base = typeof current === 'number' ? current : current == null || String(current).trim() === '' ? NaN : Number(current);
	const { min, max } = boundsFor(field);
	let next = (Number.isFinite(base) ? base : (typeof min === 'number' ? min : 0)) + step * direction;
	if (typeof min === 'number') next = Math.max(min, next);
	if (typeof max === 'number') next = Math.min(max, next);
	// Keep decimal steps clean (0.1 + 0.2).
	return Math.round(next * 1e6) / 1e6;
}
