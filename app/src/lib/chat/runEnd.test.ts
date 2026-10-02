import { describe, expect, it } from 'vitest';
import { endsRun } from './runEnd';

// The sidebar's working dot clears on the turn-end events only when they end
// the run (`+layout.svelte`), so an employee keeps showing as working while a
// message the owner sent meanwhile is taken into its running turn.
describe('run end', () => {
	it('a queued message completing does not end the run; every other turn end does', () => {
		expect(endsRun({ stop_reason: 'queued_into_running_turn' })).toBe(false);
		expect(endsRun({})).toBe(true);
		expect(endsRun({ stop_reason: 'max_steps' })).toBe(true);
		expect(endsRun(null)).toBe(true);
	});
});
