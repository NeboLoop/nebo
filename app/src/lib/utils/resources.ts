import { listMCPIntegrations, listExtensions, getActiveAgents } from '$lib/api/nebo';

export type Resource = {
	type: 'mcp' | 'skill' | 'agent' | 'cmd';
	id: string;
	name: string;
	status: 'ok' | 'warn';
};

export async function loadResources(currentAgentId?: string): Promise<Resource[]> {
	const [mcpRes, extRes, agentsRes] = await Promise.all([
		listMCPIntegrations().catch(() => ({ integrations: [] })),
		listExtensions().catch(() => ({ skills: [] })),
		getActiveAgents().catch(() => ({ agents: [] })),
	]);
	const mcps: Resource[] = (mcpRes.integrations || []).map((i: any) => ({
		type: 'mcp' as const,
		id: i.id,
		name: i.name,
		status: i.connectionStatus === 'connected' ? ('ok' as const) : ('warn' as const),
	}));
	const skillList = (extRes as any).extensions || (extRes as any).skills || [];
	const skills: Resource[] = skillList.map((s: any) => ({
		type: 'skill' as const,
		id: s.name,
		name: s.name,
		status: s.enabled ? ('ok' as const) : ('warn' as const),
	}));
	const agents: Resource[] = (agentsRes.agents || [])
		.filter((r: any) => r.agentId !== currentAgentId)
		.map((r: any) => ({
			type: 'agent' as const,
			id: r.agentId,
			name: r.name,
			status: 'ok' as const,
		}));
	const commands: Resource[] = [
		{ type: 'cmd', id: 'exit', name: 'exit', status: 'ok' },
		{ type: 'cmd', id: 'emit', name: 'emit', status: 'ok' },
	];
	// Order must match visual display: commands, mcps, skills, agents
	return [...commands, ...mcps, ...skills, ...agents];
}
