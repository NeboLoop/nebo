// Pure mapping between the agent-workflow CRUD API wire shapes and the
// frontend WorkflowConfig model. No state lives here — the [agentId] layout
// owns the workflow state and calls these to (de)serialize it.

import type { AgentWorkflowEntry } from '$lib/api/neboComponents';
import type { WorkflowConfig, WorkflowTrigger } from '$lib/types/agentPage';

/**
 * Map the backend workflow payload (map keyed by binding name) into
 * WorkflowConfig, preserving every trigger field (cron, interval, window,
 * sources, plugin, ...) so an edit→save round-trip never strips trigger
 * configuration.
 */
export function mapWorkflows(
	wfData: Record<string, AgentWorkflowEntry> | undefined | null
): Record<string, WorkflowConfig> | null {
	if (!wfData || typeof wfData !== 'object' || Array.isArray(wfData)) return null;
	const wfMap: Record<string, WorkflowConfig> = {};
	for (const [name, wf] of Object.entries(wfData)) {
		if (!name) continue;
		const trigger = wf.trigger ?? { type: 'manual' };
		wfMap[name] = {
			trigger: {
				...trigger,
				type: trigger.type || 'manual',
				event: trigger.event ?? trigger.sources?.join(', '),
			},
			schedule: trigger.schedule || trigger.cron,
			activities: Array.isArray(wf.activities)
				? (wf.activities as WorkflowConfig['activities'])
				: [],
			connections: Array.isArray(wf.connections)
				? (wf.connections as WorkflowConfig['connections'])
				: undefined,
			isActive: wf.isActive !== false,
			description: typeof wf.description === 'string' ? wf.description : undefined,
			lastFired: typeof wf.lastFired === 'string' ? wf.lastFired : undefined,
			emit: typeof wf.emit === 'string' ? wf.emit : undefined,
		};
	}
	return wfMap;
}

/**
 * Build the triggerType/triggerConfig payload the workflow CRUD API expects
 * from a WorkflowConfig trigger.
 */
export function triggerPayload(wf: WorkflowConfig): {
	triggerType: string;
	triggerConfig: Record<string, unknown>;
} {
	const t: WorkflowTrigger = wf.trigger ?? { type: 'manual' };
	switch (t.type) {
		case 'schedule':
			return {
				triggerType: 'schedule',
				triggerConfig: {
					cron: t.cron || t.schedule || wf.schedule || '',
					schedule: t.schedule || wf.schedule || undefined,
				},
			};
		case 'heartbeat': {
			const cfg: Record<string, unknown> = { interval: t.interval || '30m' };
			if (t.window?.start && t.window?.end) cfg.window = `${t.window.start}-${t.window.end}`;
			return { triggerType: 'heartbeat', triggerConfig: cfg };
		}
		case 'event': {
			const sources =
				t.sources ?? (t.event ? t.event.split(',').map((s) => s.trim()).filter(Boolean) : []);
			return { triggerType: 'event', triggerConfig: { sources } };
		}
		case 'watch':
		case 'folder': {
			const { type, ...rest } = t;
			return { triggerType: type, triggerConfig: rest };
		}
		default:
			return { triggerType: 'manual', triggerConfig: {} };
	}
}

/**
 * Persist a workflow map through the binding CRUD API by diffing against the
 * previously-loaded state: create new bindings, update changed ones, delete
 * removed ones. Returns the freshly mapped server state (or null if the
 * post-save refresh failed).
 */
export async function saveWorkflows(
	agentId: string,
	prev: Record<string, WorkflowConfig>,
	next: Record<string, WorkflowConfig>
): Promise<Record<string, WorkflowConfig> | null> {
	const api = await import('$lib/api/nebo');
	for (const name of Object.keys(prev)) {
		if (!(name in next)) await api.deleteAgentWorkflow(agentId, name);
	}
	for (const [name, wf] of Object.entries(next)) {
		const { triggerType, triggerConfig } = triggerPayload(wf);
		const payload: Record<string, unknown> = {
			triggerType,
			triggerConfig,
			description: wf.description ?? '',
			activities: wf.activities ?? [],
			connections: wf.connections ?? [],
			emit: wf.emit ?? null,
		};
		if (!(name in prev)) {
			await api.createAgentWorkflow(agentId, { bindingName: name, ...payload });
		} else if (JSON.stringify(prev[name]) !== JSON.stringify(wf)) {
			await api.updateAgentWorkflow(agentId, name, payload);
		}
		// isActive changes go through their canonical endpoint. `prev` is
		// server state, so one toggle moves the server to the draft's value.
		const wasActive = name in prev ? prev[name].isActive !== false : true;
		if ((wf.isActive !== false) !== wasActive) {
			await api.toggleAgentWorkflow(agentId, name);
		}
	}
	const resp = await api.listAgentWorkflows(agentId).catch(() => null);
	return mapWorkflows(resp?.workflows);
}
