import { describe, expect, it } from 'vitest';
import { applyHelperEvent, type HelperLine } from './helpers';

describe('applyHelperEvent', () => {
	it('tracks a helper from start to finish', () => {
		let list: HelperLine[] = [];
		list = applyHelperEvent(list, 'subagent_start', { task_id: 'h-1', title: 'read the ledger' });
		expect(list).toEqual([{ taskId: 'h-1', title: 'read the ledger', activity: '' }]);
		const call = { kind: 'read', params: { path: 'march.csv' } };
		list = applyHelperEvent(list, 'subagent_progress', { task_id: 'h-1', call });
		expect(list[0]).toEqual({ taskId: 'h-1', title: 'read the ledger', activity: '', call });
		// Background work says its own line, and it replaces the call.
		list = applyHelperEvent(list, 'subagent_progress', { task_id: 'h-1', current_operation: 'Searching 4 angles' });
		expect(list[0]).toEqual({ taskId: 'h-1', title: 'read the ledger', activity: 'Searching 4 angles' });
		// A sibling starting retitles it at once; a step carries the title
		// as it stands.
		list = applyHelperEvent(list, 'subagent_titles', { titles: [{ task_id: 'h-1', title: 'read the ledger — …March' }, { task_id: 'h-9', title: 'x' }, { task_id: 'h-1', title: '' }] });
		expect(list).toEqual([{ taskId: 'h-1', title: 'read the ledger — …March', activity: 'Searching 4 angles' }]);
		list = applyHelperEvent(list, 'subagent_progress', { task_id: 'h-1', title: 'read the ledger', current_operation: 'Reading 4 sources' });
		expect(list[0]).toEqual({ taskId: 'h-1', title: 'read the ledger', activity: 'Reading 4 sources' });
		list = applyHelperEvent(list, 'subagent_complete', { task_id: 'h-1' });
		expect(list).toEqual([]);
	});

	it('keeps helpers apart and ignores strangers', () => {
		let list: HelperLine[] = [];
		list = applyHelperEvent(list, 'subagent_start', { task_id: 'a', title: 'one' });
		list = applyHelperEvent(list, 'subagent_start', { task_id: 'b', title: 'two' });
		list = applyHelperEvent(list, 'subagent_progress', { task_id: 'zzz', current_operation: 'x' });
		list = applyHelperEvent(list, 'subagent_progress', { task_id: 'b', current_operation: 'searching' });
		expect(list.map((h) => [h.taskId, h.activity])).toEqual([
			['a', ''],
			['b', 'searching'],
		]);
		expect(applyHelperEvent(list, 'subagent_start', {})).toBe(list);
	});

	it('a resumed helper keeps its title', () => {
		let list = applyHelperEvent([], 'subagent_start', { task_id: 'h-1', title: 'scan logs' });
		list = applyHelperEvent(list, 'subagent_start', { task_id: 'h-1' });
		expect(list).toEqual([{ taskId: 'h-1', title: 'scan logs', activity: '' }]);
	});
});
