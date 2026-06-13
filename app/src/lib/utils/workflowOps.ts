// Architect edit operations — the structured contract the workflow-builder
// chat emits and the builder applies to its LOCAL DRAFT. Ops mirror the
// WorkflowConfig model 1:1 so everything the canvas can express is
// expressible here. Application is pure: callers own state/undo/persistence
// (the builder applies a batch as ONE undo snapshot; Save persists).

import type { WorkflowActivity, WorkflowConfig, WorkflowTrigger } from '$lib/types/agentPage';

export type WorkflowOp =
	| { op: 'create_workflow'; name: string; workflow?: Partial<WorkflowConfig> }
	| { op: 'delete_workflow'; workflow: string }
	| { op: 'rename_workflow'; from: string; to: string }
	| { op: 'set_trigger'; workflow: string; trigger: WorkflowTrigger }
	| { op: 'set_emit'; workflow: string; emit: string | null }
	| { op: 'set_description'; workflow: string; description: string }
	| {
			op: 'add_activity';
			workflow: string;
			activity: WorkflowActivity;
			after?: string | null;
			branchLabel?: string;
	  }
	| { op: 'update_activity'; workflow: string; id: string; set: Partial<WorkflowActivity> }
	| { op: 'remove_activity'; workflow: string; id: string }
	| { op: 'connect'; workflow: string; from: string; to: string; label?: string }
	| { op: 'disconnect'; workflow: string; from: string; to: string };

export interface ApplyOpsResult {
	workflows: Record<string, WorkflowConfig>;
	/** Human-readable description of each applied op (for the chat summary). */
	applied: string[];
	/** Ops that could not be applied, with the reason (also for the chat). */
	skipped: { op: string; reason: string }[];
}

const TRIGGER_NODE = '__trigger__';
const EMIT_NODE = '__emit__';

/**
 * Apply a batch of Architect ops to a workflow map. Pure — returns a new map;
 * invalid ops are skipped with a reason, valid ones still apply.
 */
export function applyOps(
	workflows: Record<string, WorkflowConfig>,
	ops: WorkflowOp[]
): ApplyOpsResult {
	const next: Record<string, WorkflowConfig> = JSON.parse(JSON.stringify(workflows));
	const applied: string[] = [];
	const skipped: { op: string; reason: string }[] = [];

	for (const op of ops) {
		const reason = applyOne(next, op);
		if (reason === null) {
			applied.push(describeOp(op));
		} else {
			skipped.push({ op: op.op, reason });
		}
	}

	return { workflows: next, applied, skipped };
}

/** Apply one op in place. Returns null on success, or a skip reason. */
function applyOne(workflows: Record<string, WorkflowConfig>, op: WorkflowOp): string | null {
	switch (op.op) {
		case 'create_workflow': {
			if (!op.name?.trim()) return 'create_workflow requires a name';
			if (workflows[op.name]) return `workflow "${op.name}" already exists`;
			workflows[op.name] = {
				trigger: { type: 'manual' },
				description: '',
				isActive: true,
				activities: [],
				...op.workflow,
			};
			return null;
		}
		case 'delete_workflow': {
			if (!workflows[op.workflow]) return `unknown workflow "${op.workflow}"`;
			delete workflows[op.workflow];
			return null;
		}
		case 'rename_workflow': {
			if (!workflows[op.from]) return `unknown workflow "${op.from}"`;
			if (!op.to?.trim()) return 'rename_workflow requires a target name';
			if (workflows[op.to]) return `workflow "${op.to}" already exists`;
			workflows[op.to] = workflows[op.from];
			delete workflows[op.from];
			return null;
		}
		case 'set_trigger': {
			const wf = workflows[op.workflow];
			if (!wf) return `unknown workflow "${op.workflow}"`;
			if (!op.trigger?.type) return 'set_trigger requires trigger.type';
			wf.trigger = op.trigger;
			return null;
		}
		case 'set_emit': {
			const wf = workflows[op.workflow];
			if (!wf) return `unknown workflow "${op.workflow}"`;
			wf.emit = op.emit || undefined;
			return null;
		}
		case 'set_description': {
			const wf = workflows[op.workflow];
			if (!wf) return `unknown workflow "${op.workflow}"`;
			wf.description = op.description ?? '';
			return null;
		}
		case 'add_activity': {
			const wf = workflows[op.workflow];
			if (!wf) return `unknown workflow "${op.workflow}"`;
			const activity = op.activity;
			if (!activity?.id?.trim()) return 'add_activity requires activity.id';
			const activities = wf.activities ?? [];
			if (activities.some((a) => a.id === activity.id)) {
				return `activity id "${activity.id}" already exists`;
			}
			if (op.after && op.after !== TRIGGER_NODE && !activities.some((a) => a.id === op.after)) {
				return `unknown anchor activity "${op.after}"`;
			}

			// Insert at the right array position.
			const insertIdx =
				op.after == null
					? activities.length
					: op.after === TRIGGER_NODE
						? 0
						: activities.findIndex((a) => a.id === op.after) + 1;
			activities.splice(insertIdx, 0, activity);
			wf.activities = activities;

			// Wire connections when the workflow uses an explicit graph.
			if (wf.connections && op.after != null) {
				const wanted = op.branchLabel ?? undefined;
				const existing = wf.connections.find(
					(c) => c.from === op.after && (c.label ?? undefined) === wanted
				);
				if (existing) {
					// Splice into the chain: after -> new -> old target.
					const oldTarget = existing.to;
					existing.to = activity.id;
					wf.connections.push({ from: activity.id, to: oldTarget });
				} else {
					wf.connections.push({
						from: op.after,
						to: activity.id,
						...(op.branchLabel ? { label: op.branchLabel } : {}),
					});
				}
			}
			return null;
		}
		case 'update_activity': {
			const wf = workflows[op.workflow];
			if (!wf) return `unknown workflow "${op.workflow}"`;
			const activity = wf.activities?.find((a) => a.id === op.id);
			if (!activity) return `unknown activity "${op.id}"`;
			if (!op.set || typeof op.set !== 'object') return 'update_activity requires set';

			const newId = op.set.id;
			if (newId && newId !== op.id) {
				if (wf.activities?.some((a) => a.id === newId)) {
					return `activity id "${newId}" already exists`;
				}
				// Keep the graph attached through the rename.
				for (const conn of wf.connections ?? []) {
					if (conn.from === op.id) conn.from = newId;
					if (conn.to === op.id) conn.to = newId;
				}
				for (const other of wf.activities ?? []) {
					for (const branch of other.branches ?? []) {
						if (branch.nextId === op.id) branch.nextId = newId;
					}
				}
			}
			Object.assign(activity, op.set);
			return null;
		}
		case 'remove_activity': {
			const wf = workflows[op.workflow];
			if (!wf) return `unknown workflow "${op.workflow}"`;
			const activities = wf.activities ?? [];
			if (!activities.some((a) => a.id === op.id)) return `unknown activity "${op.id}"`;
			wf.activities = activities.filter((a) => a.id !== op.id);

			if (wf.connections) {
				const incoming = wf.connections.filter((c) => c.to === op.id);
				const outgoing = wf.connections.filter((c) => c.from === op.id);
				wf.connections = wf.connections.filter((c) => c.from !== op.id && c.to !== op.id);
				// Bridge parents to children, preserving the incoming branch
				// label and skipping duplicates/self-loops.
				for (const inc of incoming) {
					for (const out of outgoing) {
						if (inc.from === out.to) continue;
						const exists = wf.connections.some(
							(c) =>
								c.from === inc.from &&
								c.to === out.to &&
								(c.label ?? undefined) === (inc.label ?? undefined)
						);
						if (!exists) {
							wf.connections.push({
								from: inc.from,
								to: out.to,
								...(inc.label ? { label: inc.label } : {}),
							});
						}
					}
				}
			}
			return null;
		}
		case 'connect': {
			const wf = workflows[op.workflow];
			if (!wf) return `unknown workflow "${op.workflow}"`;
			if (op.to === TRIGGER_NODE) return 'connections cannot target __trigger__';
			if (op.from === EMIT_NODE) return 'connections cannot originate from __emit__';
			const ids = new Set((wf.activities ?? []).map((a) => a.id));
			if (op.from !== TRIGGER_NODE && !ids.has(op.from)) {
				return `unknown activity "${op.from}"`;
			}
			if (op.to !== EMIT_NODE && !ids.has(op.to)) return `unknown activity "${op.to}"`;
			wf.connections = wf.connections ?? [];
			const dup = wf.connections.some(
				(c) =>
					c.from === op.from && c.to === op.to && (c.label ?? undefined) === (op.label ?? undefined)
			);
			if (dup) return `connection ${op.from} -> ${op.to} already exists`;
			wf.connections.push({
				from: op.from,
				to: op.to,
				...(op.label ? { label: op.label } : {}),
			});
			return null;
		}
		case 'disconnect': {
			const wf = workflows[op.workflow];
			if (!wf) return `unknown workflow "${op.workflow}"`;
			const before = wf.connections?.length ?? 0;
			wf.connections = (wf.connections ?? []).filter(
				(c) => !(c.from === op.from && c.to === op.to)
			);
			if ((wf.connections?.length ?? 0) === before) {
				return `no connection ${op.from} -> ${op.to}`;
			}
			return null;
		}
		default:
			return `unknown op "${(op as { op?: string }).op}"`;
	}
}

function describeOp(op: WorkflowOp): string {
	switch (op.op) {
		case 'create_workflow':
			return `created workflow "${op.name}"`;
		case 'delete_workflow':
			return `deleted workflow "${op.workflow}"`;
		case 'rename_workflow':
			return `renamed "${op.from}" to "${op.to}"`;
		case 'set_trigger':
			return `set ${op.workflow} trigger to ${op.trigger.type}`;
		case 'set_emit':
			return op.emit ? `set ${op.workflow} emit to ${op.emit}` : `cleared ${op.workflow} emit`;
		case 'set_description':
			return `updated ${op.workflow} description`;
		case 'add_activity':
			return `added activity "${op.activity.id}" to ${op.workflow}`;
		case 'update_activity':
			return `updated activity "${op.id}" in ${op.workflow}`;
		case 'remove_activity':
			return `removed activity "${op.id}" from ${op.workflow}`;
		case 'connect':
			return `connected ${op.from} -> ${op.to}${op.label ? ` (${op.label})` : ''} in ${op.workflow}`;
		case 'disconnect':
			return `disconnected ${op.from} -> ${op.to} in ${op.workflow}`;
	}
}

/** Fenced-block language tag the Architect uses to carry ops in a reply. */
export const WORKFLOW_OPS_FENCE = 'workflow-ops';

/**
 * Extract a ```workflow-ops fenced block from assistant text. Returns the
 * parsed ops and the text with the block removed (for display), or null when
 * no valid block is present.
 */
export function extractOpsBlock(text: string): { ops: WorkflowOp[]; cleaned: string } | null {
	const fence = new RegExp('```' + WORKFLOW_OPS_FENCE + '\\s*\\n([\\s\\S]*?)```', 'm');
	const match = text.match(fence);
	if (!match) return null;
	try {
		const parsed = JSON.parse(match[1]);
		const ops = Array.isArray(parsed) ? parsed : parsed?.ops;
		if (!Array.isArray(ops) || ops.length === 0) return null;
		return { ops: ops as WorkflowOp[], cleaned: text.replace(fence, '').trim() };
	} catch {
		return null;
	}
}
