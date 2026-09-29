import type { LinkedAgentEntry, LinkedComputerEntry } from '$lib/api/neboComponents';

// The runtimes that are coding agents: they run their own tools on their
// computer, under the permission mode the employee is hired with.
export const CODING = new Set(['claude-code', 'codex', 'gemini', 'opencode', 'acp']);

// `company`: the employee follows the company's mode, now and when it
// changes; any other is its own.
export type LinkedMode = 'company' | 'automatic' | 'ask' | 'plan' | 'full_access';

// Whether anything on the list is a coding agent to hire, which is hired
// with a permission mode.
export function anyCoding(computers: LinkedComputerEntry[]): boolean {
	return computers.some((c) => c.agents.some((a) => !a.hired && CODING.has(a.runtime)));
}

// The create-agent `linked` body for an entry: the bot the entry says its
// hire goes to, and for a coding agent the permission mode picked, unless
// it follows the company's.
export function linkedHire(agent: LinkedAgentEntry, mode: LinkedMode) {
	const own = CODING.has(agent.runtime) && mode !== 'company';
	return { botId: agent.botId, agentId: agent.id, ...(own ? { permissionMode: mode } : {}) };
}

// How often the hire list looks again while it is open: an app installed
// or linked meanwhile shows up without reopening it.
export const RELIST_MS = 30_000;
