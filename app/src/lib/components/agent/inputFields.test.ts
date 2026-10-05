import { describe, expect, it } from 'vitest';
import type { AgentInputField } from '$lib/types/agentPage';
import {
	controlFor,
	shortLabel,
	hintFor,
	unitFor,
	withDefaults,
	validateInputs,
	stepNumber
} from './inputFields';

const f = (over: Partial<AgentInputField>): AgentInputField => ({ key: 'k', label: '', type: 'text', ...over });

describe('setup questions', () => {
	it('picks the control for the type', () => {
		expect(controlFor(f({ type: 'number' }))).toBe('number');
		expect(controlFor(f({ type: 'checkbox' }))).toBe('toggle');
		expect(controlFor(f({ type: 'radio' }))).toBe('select');
		expect(controlFor(f({ type: 'select' }))).toBe('select');
		expect(controlFor(f({ type: 'text', options: [{ value: 'a', label: 'A' }] }))).toBe('select');
		expect(controlFor(f({ type: 'textarea' }))).toBe('textarea');
		expect(controlFor(f({ type: 'text' }))).toBe('text');
	});

	it('shortens a long question to a label and keeps the question as the hint', () => {
		const q = f({ key: 'overdue_days', label: 'How many days past due before I start chasing an invoice?' });
		expect(shortLabel(q)).toBe('Overdue days');
		expect(hintFor(q)).toBe('How many days past due before I start chasing an invoice?');
		const short = f({ key: 'mailbox', label: 'Invoice mailbox', description: 'Where bills arrive' });
		expect(shortLabel(short)).toBe('Invoice mailbox');
		expect(hintFor(short)).toBe('Where bills arrive');
	});

	it('uses the schema unit, else infers one', () => {
		expect(unitFor(f({ type: 'number', unit: 'invoices' }))).toEqual({ text: 'invoices' });
		expect(unitFor(f({ type: 'number', key: 'overdue_days' }))).toEqual({ key: 'units.days' });
		expect(unitFor(f({ type: 'number', label: 'What percent of the budget?' }))).toEqual({ text: '%' });
		expect(unitFor(f({ type: 'number', label: 'How many?' }))).toBeNull();
		expect(unitFor(f({ type: 'text', label: 'How many days?' }))).toBeNull();
		expect(unitFor(f({ type: 'number', money: true, label: 'Most I may spend each month?' }))).toBeNull();
	});

	it('pre-fills defaults, never for money', () => {
		const fields = [
			f({ key: 'a', default: 7 }),
			f({ key: 'b', default: 100, money: true }),
			f({ key: 'c', default: 'x' })
		];
		expect(withDefaults(fields, { c: 'saved' })).toEqual({ a: 7, c: 'saved' });
	});

	it('validates required answers and number bounds', () => {
		const fields = [
			f({ key: 'name', required: true }),
			f({ key: 'days', type: 'number', min: 1, max: 30 }),
			f({ key: 'note' })
		];
		expect(validateInputs(fields, { name: ' ', days: 'abc' })).toEqual({
			name: { key: 'agentInputForm.requiredError' },
			days: { key: 'agentInputForm.numberError' }
		});
		expect(validateInputs(fields, { name: 'x', days: 40 })).toEqual({
			days: { key: 'agentInputForm.maxError', values: { max: 30 } }
		});
		expect(validateInputs(fields, { name: 'x', days: 5 })).toEqual({});
	});

	it('steps a number inside its bounds', () => {
		const days = f({ type: 'number', min: 1, max: 3 });
		expect(stepNumber(days, 3, 1)).toBe(3);
		expect(stepNumber(days, 1, -1)).toBe(1);
		expect(stepNumber(days, '', 1)).toBe(2);
		expect(stepNumber(f({ type: 'number', step: 0.1 }), 0.2, 1)).toBe(0.3);
	});
});
