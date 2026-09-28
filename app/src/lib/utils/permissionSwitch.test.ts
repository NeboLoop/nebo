import { describe, it, expect } from 'vitest';
import type { PermissionSwitch } from '$lib/api/nebo';
import { switchChange, inheritedNote } from './permissionSwitch';

const sw = (over: Partial<PermissionSwitch>): PermissionSwitch => ({
	id: 'capability:web',
	sentence: 'Look things up and open pages on the web',
	value: 'ask',
	inherited: false,
	canInherit: true,
	locked: false,
	...over
});

describe('switchChange', () => {
	it('sends the state tapped when it is not lit', () => {
		expect(switchChange(sw({ value: 'ask' }), 'allow')).toBe('allow');
		expect(switchChange(sw({ value: 'ask', inherited: true, canInherit: false }), 'deny')).toBe('deny');
	});

	it('clears the page’s own setting when its lit state is tapped', () => {
		expect(switchChange(sw({ value: 'allow' }), 'allow')).toBe('inherit');
	});

	it('does nothing for a lit own state with nothing to fall back to', () => {
		expect(switchChange(sw({ value: 'allow', canInherit: false }), 'allow')).toBeNull();
	});

	it('makes a lit inherited state the page’s own', () => {
		expect(switchChange(sw({ value: 'ask', inherited: true, inheritsFrom: 'company', canInherit: false }), 'ask')).toBe('ask');
	});

	it('sends nothing for a switch the company turns off', () => {
		expect(switchChange(sw({ value: 'deny', inherited: true, locked: true, canInherit: false }), 'allow')).toBeNull();
	});
});

describe('inheritedNote', () => {
	it('names where an inherited value comes from', () => {
		expect(inheritedNote(sw({}))).toBeNull();
		expect(inheritedNote(sw({ inherited: true, inheritsFrom: 'default' }))).toBe('permissions.viaDefault');
		expect(inheritedNote(sw({ inherited: true, inheritsFrom: 'company' }))).toBe('permissions.sameAsCompany');
		expect(inheritedNote(sw({ inherited: true, inheritsFrom: 'company', locked: true }))).toBe('permissions.lockedByCompany');
	});
});
