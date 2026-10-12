import { describe, expect, it } from 'vitest';
import { createTypedActivity, getActivityType } from './workflowTypes';

describe('expert node', () => {
	it('is its own type with the params the engine reads', () => {
		const def = getActivityType('expert');
		expect(def.type).toBe('expert');
		expect(def.parameters.map((p) => p.key)).toEqual(['expert', 'task', 'input', 'output', 'timeout']);
	});

	it('starts blank: the catalog blurb never becomes the task', () => {
		const act = createTypedActivity('activity-expert', { label: 'Expert', desc: 'Another employee does the step' });
		expect(act.type).toBe('expert');
		expect(act.intent).toBe('');
		expect(act.params).toEqual({});
	});
});
