import type { LinkedAgentEntry, LinkedComputerEntry } from '$lib/api/neboComponents';

// The runtimes that are coding agents: they run their own tools on their
// computer, under the permission mode the employee is hired with.
export const CODING = new Set(['claude-code', 'codex', 'gemini', 'opencode', 'acp']);

// Whether anything on the list is a coding agent to hire, which is hired
// with a permission mode.
export function anyCoding(computers: LinkedComputerEntry[]): boolean {
	return computers.some((c) => c.new.length > 0 || c.agents.some((a) => !a.hired && CODING.has(a.runtime)));
}

// The create-agent `linked` body for a row: the bot the row says its hire
// goes to, and for a coding agent the permission mode picked.
export function linkedHire(agent: LinkedAgentEntry, mode: string) {
	return { botId: agent.botId, agentId: agent.id, ...(CODING.has(agent.runtime) ? { permissionMode: mode } : {}) };
}
