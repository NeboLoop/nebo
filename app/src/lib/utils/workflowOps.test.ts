import { describe, expect, it } from 'vitest';
import type { WorkflowConfig } from '$lib/types/agentPage';
import { applyOps, extractOpsBlock } from './workflowOps';

function base(): Record<string, WorkflowConfig> {
	return {
		wf: {
			trigger: { type: 'manual' },
			description: 'test',
			isActive: true,
			activities: [
				{ id: 'a', type: 'custom', intent: 'do a' },
				{ id: 'b', type: 'custom', intent: 'do b' },
			],
			connections: [
				{ from: '__trigger__', to: 'a' },
				{ from: 'a', to: 'b' },
				{ from: 'b', to: '__emit__' },
			],
		},
	};
}

describe('applyOps', () => {
	it('adds an activity and splices the chain', () => {
		const result = applyOps(base(), [
			{
				op: 'add_activity',
				workflow: 'wf',
				activity: { id: 'mid', type: 'notify', intent: 'ping' },
				after: 'a',
			},
		]);
		expect(result.applied).toHaveLength(1);
		const wf = result.workflows.wf;
		expect(wf.activities?.map((a) => a.id)).toEqual(['a', 'mid', 'b']);
		// a -> mid -> b (spliced, not forked)
		expect(wf.connections).toContainEqual({ from: 'a', to: 'mid' });
		expect(wf.connections).toContainEqual({ from: 'mid', to: 'b' });
		expect(wf.connections?.some((c) => c.from === 'a' && c.to === 'b')).toBe(false);
	});

	it('removes an activity and bridges with label preservation', () => {
		const workflows = base();
		workflows.wf.connections = [
			{ from: '__trigger__', to: 'a' },
			{ from: 'a', to: 'b', label: 'True' },
			{ from: 'b', to: '__emit__' },
		];
		const result = applyOps(workflows, [{ op: 'remove_activity', workflow: 'wf', id: 'b' }]);
		expect(result.applied).toHaveLength(1);
		const wf = result.workflows.wf;
		expect(wf.activities?.map((a) => a.id)).toEqual(['a']);
		// Bridge a -> __emit__ carries the incoming label.
		expect(wf.connections).toContainEqual({ from: 'a', to: '__emit__', label: 'True' });
		expect(wf.connections?.some((c) => c.from === 'b' || c.to === 'b')).toBe(false);
	});

	it('renames an activity id and rewrites connections', () => {
		const result = applyOps(base(), [
			{ op: 'update_activity', workflow: 'wf', id: 'a', set: { id: 'a2', intent: 'renamed' } },
		]);
		const wf = result.workflows.wf;
		expect(wf.activities?.[0].id).toBe('a2');
		expect(wf.activities?.[0].intent).toBe('renamed');
		expect(wf.connections).toContainEqual({ from: '__trigger__', to: 'a2' });
		expect(wf.connections).toContainEqual({ from: 'a2', to: 'b' });
	});

	it('validates connect endpoints and rejects duplicates/illegal direction', () => {
		const result = applyOps(base(), [
			{ op: 'connect', workflow: 'wf', from: 'a', to: 'ghost' },
			{ op: 'connect', workflow: 'wf', from: 'a', to: 'b' }, // duplicate
			{ op: 'connect', workflow: 'wf', from: '__emit__', to: 'a' },
			{ op: 'connect', workflow: 'wf', from: 'b', to: '__trigger__' },
		]);
		expect(result.applied).toHaveLength(0);
		expect(result.skipped).toHaveLength(4);
	});

	it('skips invalid ops but applies valid ones in the same batch', () => {
		const result = applyOps(base(), [
			{ op: 'set_description', workflow: 'wf', description: 'new desc' },
			{ op: 'remove_activity', workflow: 'wf', id: 'ghost' },
			{ op: 'set_emit', workflow: 'wf', emit: 'wf.done' },
		]);
		expect(result.applied).toHaveLength(2);
		expect(result.skipped).toHaveLength(1);
		expect(result.workflows.wf.description).toBe('new desc');
		expect(result.workflows.wf.emit).toBe('wf.done');
	});

	it('creates, renames and deletes workflows', () => {
		const result = applyOps(base(), [
			{ op: 'create_workflow', name: 'wf2', workflow: { description: 'second' } },
			{ op: 'rename_workflow', from: 'wf2', to: 'wf3' },
			{ op: 'delete_workflow', workflow: 'wf' },
		]);
		expect(result.applied).toHaveLength(3);
		expect(Object.keys(result.workflows)).toEqual(['wf3']);
		expect(result.workflows.wf3.description).toBe('second');
		expect(result.workflows.wf3.trigger?.type).toBe('manual');
	});

	it('does not mutate the input map', () => {
		const input = base();
		applyOps(input, [{ op: 'delete_workflow', workflow: 'wf' }]);
		expect(input.wf).toBeDefined();
	});
});

describe('extractOpsBlock', () => {
	it('parses a fenced ops block and strips it from display text', () => {
		const text =
			'Adding the step now.\n\n```workflow-ops\n' +
			JSON.stringify({
				ops: [{ op: 'set_description', workflow: 'wf', description: 'x' }],
			}) +
			'\n```\n\nDone!';
		const extracted = extractOpsBlock(text);
		expect(extracted).not.toBeNull();
		expect(extracted?.ops).toHaveLength(1);
		expect(extracted?.cleaned).toBe('Adding the step now.\n\n\n\nDone!'.trim());
		expect(extracted?.cleaned).not.toContain('workflow-ops');
	});

	it('accepts a bare array and rejects malformed blocks', () => {
		const bare = '```workflow-ops\n[{"op":"delete_workflow","workflow":"wf"}]\n```';
		expect(extractOpsBlock(bare)?.ops).toHaveLength(1);
		expect(extractOpsBlock('no block here')).toBeNull();
		expect(extractOpsBlock('```workflow-ops\nnot json\n```')).toBeNull();
		expect(extractOpsBlock('```workflow-ops\n{"ops":[]}\n```')).toBeNull();
	});
});
