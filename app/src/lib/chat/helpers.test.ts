import { describe, expect, it } from 'vitest';
import { applyHelperEvent, type HelperLine } from './helpers';

describe('applyHelperEvent', () => {
	it('tracks a helper from start to finish', () => {
		let list: HelperLine[] = [];
		list = applyHelperEvent(list, 'subagent_start', { task_id: 'h-1', description: 'read the ledger' });
		expect(list).toEqual([{ taskId: 'h-1', description: 'read the ledger', activity: '' }]);
		list = applyHelperEvent(list, 'subagent_progress', { task_id: 'h-1', current_operation: 'reading march.csv' });
		expect(list[0].activity).toBe('reading march.csv');
		list = applyHelperEvent(list, 'subagent_complete', { task_id: 'h-1' });
		expect(list).toEqual([]);
	});

	it('keeps helpers apart and ignores strangers', () => {
		let list: HelperLine[] = [];
		list = applyHelperEvent(list, 'subagent_start', { task_id: 'a', description: 'one' });
		list = applyHelperEvent(list, 'subagent_start', { task_id: 'b', description: 'two' });
		list = applyHelperEvent(list, 'subagent_progress', { task_id: 'zzz', current_operation: 'x' });
		list = applyHelperEvent(list, 'subagent_progress', { task_id: 'b', current_operation: 'searching' });
		expect(list.map((h) => [h.taskId, h.activity])).toEqual([
			['a', ''],
			['b', 'searching'],
		]);
		expect(applyHelperEvent(list, 'subagent_start', {})).toBe(list);
	});

	it('a resumed helper keeps its description', () => {
		let list = applyHelperEvent([], 'subagent_start', { task_id: 'h-1', description: 'scan logs' });
		list = applyHelperEvent(list, 'subagent_start', { task_id: 'h-1' });
		expect(list).toEqual([{ taskId: 'h-1', description: 'scan logs', activity: '' }]);
	});
});
