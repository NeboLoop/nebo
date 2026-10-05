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

/** `overdue_days` / `finance.ap.invoice_mailbox` → "Overdue days" / "Invoice mailbox". */
function humanize(key: string): string {
	const last = key.split('.').pop() || key;
	const words = last.replace(/([a-z])([A-Z])/g, '$1 $2').replace(/[_-]+/g, ' ').trim().toLowerCase();
	return words.charAt(0).toUpperCase() + words.slice(1);
}

/** The short name shown as the field's label. A label written as a long
 *  question gives way to a name made from the field's key. */
export function shortLabel(field: AgentInputField): string {
	const label = (field.label || '').trim();
	if (label && !isQuestion(label)) return label;
	const fromKey = humanize(field.key || field.id || '');
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
const INFERRED_UNITS: [RegExp, string][] = [
	[/(%|\bpercent(age)?\b|\bpct\b)/, '%'],
	[/\bseconds?\b|_secs?\b|_seconds\b/, 'units.seconds'],
	[/\bminutes?\b|_mins?\b|_minutes\b/, 'units.minutes'],
	[/\bhours?\b|_hrs?\b|_hours\b/, 'units.hours'],
	[/\bdays?\b|_days\b/, 'units.days'],
	[/\bweeks?\b|_weeks\b/, 'units.weeks'],
	[/\bmonths?\b|_months\b/, 'units.months'],
	[/\byears?\b|_years\b/, 'units.years']
];

export type FieldUnit = { text: string } | { key: string } | null;

/** A number field's unit: the schema's `unit` when it declares one, else
 *  inferred from its key and question. Null when nothing names one. */
export function unitFor(field: AgentInputField): FieldUnit {
	if (controlFor(field) !== 'number') return null;
	if (field.unit && field.unit.trim()) return { text: field.unit.trim() };
	// A money amount is in a currency, never a time unit its question mentions.
	if (field.money) return null;
	const haystack = `${field.key || ''} ${field.label || ''} ${field.description || ''}`.toLowerCase();
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
		if (f.money || f.default === undefined || f.default === null || f.default === '') continue;
		out[f.key] = f.default;
	}
	return out;
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
			if (!Number.isFinite(n)) {
				errors[f.key] = { key: 'agentInputForm.numberError' };
			} else if (typeof f.min === 'number' && n < f.min) {
				errors[f.key] = { key: 'agentInputForm.minError', values: { min: f.min } };
			} else if (typeof f.max === 'number' && n > f.max) {
				errors[f.key] = { key: 'agentInputForm.maxError', values: { max: f.max } };
			}
		}
	}
	return errors;
}

/** One +/- press on a number field, kept inside its bounds. */
export function stepNumber(field: AgentInputField, current: unknown, direction: 1 | -1): number {
	const step = typeof field.step === 'number' && field.step > 0 ? field.step : 1;
	const base = typeof current === 'number' ? current : current == null || String(current).trim() === '' ? NaN : Number(current);
	let next = (Number.isFinite(base) ? base : (typeof field.min === 'number' ? field.min : 0)) + step * direction;
	if (typeof field.min === 'number') next = Math.max(field.min, next);
	if (typeof field.max === 'number') next = Math.min(field.max, next);
	// Keep decimal steps clean (0.1 + 0.2).
	return Math.round(next * 1e6) / 1e6;
}
