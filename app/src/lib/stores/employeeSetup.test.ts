import { describe, it, expect } from 'vitest';
import { applySetupEvent, type SetupProgress } from './employeeSetup';

describe('employeeSetup', () => {
	it('follows a cascade from start to complete on its employee', () => {
		let s: Record<string, SetupProgress> = {};
		s = applySetupEvent(s, 'dep_cascade_start', { agentId: 'ada', total: 2 });
		expect(s.ada).toEqual({ settled: 0, total: 2 });
		s = applySetupEvent(s, 'dep_installed', { agentId: 'ada' });
		s = applySetupEvent(s, 'dep_failed', { agentId: 'ada' });
		expect(s.ada).toEqual({ settled: 2, total: 2 });
		// A dependency's own dependency raises the total as it lands.
		s = applySetupEvent(s, 'dep_installed', { agentId: 'ada' });
		expect(s.ada).toEqual({ settled: 3, total: 3 });
		s = applySetupEvent(s, 'dep_cascade_complete', { agentId: 'ada' });
		expect(s.ada).toBeUndefined();
	});

	it('leaves alone a cascade that sets up no employee, and other employees', () => {
		const s = { bob: { settled: 1, total: 3 } };
		expect(applySetupEvent(s, 'dep_cascade_start', { total: 4 })).toBe(s);
		expect(applySetupEvent(s, 'dep_installed', { agentId: 'ada' })).toBe(s);
		expect(applySetupEvent(s, 'dep_cascade_complete', { agentId: 'ada' })).toBe(s);
	});
});
