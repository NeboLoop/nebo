import { describe, expect, it } from 'vitest';
import type { AgentInputField } from '$lib/types/agentPage';
import {
	controlFor,
	shortLabel,
	hintFor,
	unitFor,
	isMoney,
	boundsFor,
	humanizeKey,
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

	// Real keys from the bundled employees' manifests (nebo-employees).
	it.each([
		['feed_stall_days', true, 'Feed stall'],
		['variance_threshold_pct', true, 'Variance threshold'],
		['training_expiry_warning_days', true, 'Training expiry warning'],
		['escalation_after_hours', true, 'Escalation after'],
		['material_change_pct', true, 'Material change'],
		['contact_sla_minutes', true, 'Contact SLA'],
		['buffer_minutes', true, 'Buffer'],
		['deposit_percent', true, 'Deposit'],
		['dunning_max_attempts', true, 'Dunning maximum attempts'],
		['exception_max_months', true, 'Exception maximum'],
		['abuse_count_threshold', true, 'Abuse count threshold'],
		['adhoc_promote_count', true, 'Adhoc promote'],
		['close_day', true, 'Close day'],
		['due_day', true, 'Due day'],
		['run_day', false, 'Run day'],
		['application_cutoff_day', true, 'Application cutoff day'],
		['day_of_month', true, 'Day of month'],
		['weekday', false, 'Weekday'],
		['first_response_sla_hours', true, 'First response SLA'],
		['api_base_url', false, 'API base URL'],
		['ap_mailbox', false, 'AP mailbox'],
		['qbo_company_id', false, 'QBO company ID'],
		['pto_accrual_days', true, 'PTO accrual'],
		['mrr_alert_pct', true, 'MRR alert'],
		['sku_reorder_qty', true, 'SKU reorder quantity'],
		['minimum_cash_cents', true, 'Minimum cash'],
		['review_threshold_cents', true, 'Review threshold'],
		['no_show_policy', false, 'No show policy'],
		['no_show_wait_minutes', true, 'No show wait'],
		['invoice_no', false, 'Invoice number'],
		['po_no', true, 'Po number'],
		['pct_complete_alert', true, 'Percent complete alert'],
		['discount_pct_cap', true, 'Discount percent cap'],
		['benchmark_refresh_years', true, 'Benchmark refresh'],
		['reassess_months_high', true, 'Reassess months high'],
		['count_capacity_per_week', true, 'Count capacity per week'],
		['retention_year', true, 'Retention'],
		['forecast_horizon_weeks', true, 'Forecast horizon'],
		['business_hours', false, 'Business hours'],
		['counsel_email', false, 'Counsel email'],
		['finance.ap.invoice_mailbox', false, 'Invoice mailbox'],
		['min_order_qty', true, 'Minimum order quantity'],
		['max_txn_amt_usd', true, 'Maximum transaction amount'],
		['acct_num', false, 'Account number'],
		['days', true, 'Days']
	])('names %s as plain words', (key, isNumber, want) => {
		expect(humanizeKey(key, isNumber)).toBe(want);
	});

	it('gives time, count and % numbers sensible bounds when none are declared', () => {
		expect(boundsFor(f({ type: 'number', key: 'feed_stall_days' }))).toEqual({ min: 0, max: undefined });
		expect(boundsFor(f({ type: 'number', key: 'escalation_after_hours' }))).toEqual({ min: 0, max: undefined });
		expect(boundsFor(f({ type: 'number', key: 'buffer_minutes' }))).toEqual({ min: 0, max: undefined });
		expect(boundsFor(f({ type: 'number', key: 'abuse_count_threshold' }))).toEqual({ min: 0, max: undefined });
		expect(boundsFor(f({ type: 'number', key: 'variance_threshold_pct' }))).toEqual({ min: 0, max: 100 });
		expect(boundsFor(f({ type: 'number', key: 'fee', unit: '%' }))).toEqual({ min: 0, max: 100 });
		expect(boundsFor(f({ type: 'number', key: 'temperature_offset' }))).toEqual({ min: undefined, max: undefined });
		expect(boundsFor(f({ type: 'number', key: 'lead_days', min: 2, max: 9 }))).toEqual({ min: 2, max: 9 });
	});

	it('never steps a time, count or % below 0, nor a % above 100', () => {
		expect(stepNumber(f({ type: 'number', key: 'feed_stall_days' }), 0, -1)).toBe(0);
		expect(stepNumber(f({ type: 'number', key: 'retry_count' }), 0, -1)).toBe(0);
		expect(stepNumber(f({ type: 'number', key: 'variance_threshold_pct' }), 100, 1)).toBe(100);
		expect(validateInputs([f({ type: 'number', key: 'feed_stall_days' })], { feed_stall_days: -3 })).toEqual({
			feed_stall_days: { key: 'agentInputForm.minError', values: { min: 0 } }
		});
		expect(validateInputs([f({ type: 'number', key: 'variance_threshold_pct' })], { variance_threshold_pct: 140 })).toEqual({
			variance_threshold_pct: { key: 'agentInputForm.maxError', values: { max: 100 } }
		});
	});

	it('treats a singular day as a day of the month, not a duration', () => {
		expect(unitFor(f({ type: 'number', key: 'close_day' }))).toBeNull();
		expect(unitFor(f({ type: 'number', key: 'due_day', label: 'Which day of the month are bills due?' }))).toBeNull();
		expect(unitFor(f({ type: 'number', key: 'feed_stall_days' }))).toEqual({ key: 'units.days' });
		expect(unitFor(f({ type: 'number', key: 'close_day', unit: 'days' }))).toEqual({ text: 'days' });
		expect(boundsFor(f({ type: 'number', key: 'close_day' }))).toEqual({ min: 1, max: 31 });
		expect(boundsFor(f({ type: 'number', key: 'day_of_month' }))).toEqual({ min: 1, max: 31 });
		expect(boundsFor(f({ type: 'number', key: 'run_day', max: 28 }))).toEqual({ min: 1, max: 28 });
		expect(boundsFor(f({ type: 'number', key: 'day_of_week' }))).toEqual({ min: undefined, max: undefined });
		expect(stepNumber(f({ type: 'number', key: 'close_day' }), 31, 1)).toBe(31);
		expect(stepNumber(f({ type: 'number', key: 'close_day' }), '', 1)).toBe(2);
	});

	it('shows weeks, months and years as the unit, never a rate', () => {
		expect(unitFor(f({ type: 'number', key: 'forecast_horizon_weeks' }))).toEqual({ key: 'units.weeks' });
		expect(unitFor(f({ type: 'number', key: 'exception_max_months' }))).toEqual({ key: 'units.months' });
		expect(unitFor(f({ type: 'number', key: 'benchmark_refresh_years' }))).toEqual({ key: 'units.years' });
		expect(unitFor(f({ type: 'number', key: 'count_capacity_per_week' }))).toBeNull();
	});

	it('treats a cents key as money: no unit, no default', () => {
		const cents = f({ type: 'number', key: 'review_threshold_cents', default: 50000 });
		expect(isMoney(cents)).toBe(true);
		expect(unitFor(cents)).toBeNull();
		expect(withDefaults([cents], {})).toEqual({});
		expect(isMoney(f({ type: 'number', key: 'cents_per_mile' }))).toBe(false);
	});
});
