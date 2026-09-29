import type { LinkedAgentEntry, LinkedComputerEntry } from '$lib/api/neboComponents';

// The runtimes that are coding agents: they run their own tools on their
// computer, under the permission mode the employee is hired with, and every
// hire of one is a new one, named by the owner.
export const CODING = new Set(['claude-code', 'codex', 'gemini', 'opencode', 'acp']);

// `company`: the employee follows the company's mode, now and when it
// changes; any other is its own.
export type LinkedMode = 'company' | 'automatic' | 'ask' | 'plan' | 'full_access';

// Where each app's row sits: the coding apps first, then the apps that run
// agents of their own. Any other comes after them, as it was found.
const ORDER = ['claude-code', 'codex', 'gemini', 'opencode', 'acp', 'hermes', 'openclaw'];

// One entry of an app, on the computer it is on.
export interface LinkedChoice {
	computer: LinkedComputerEntry;
	agent: LinkedAgentEntry;
}

// One row of "Hire from another app": an app, wherever it is installed.
// A coding app's choices are where a new one can start (one per computer),
// and `team` the ones already hired, by name. Another app's choices are its
// agents, the ones on the team included (`agent.hired`).
export interface LinkedApp {
	runtime: string;
	app: string;
	coding: boolean;
	choices: LinkedChoice[];
	team: { employeeId: string; name: string }[];
}

// The owner's computers as one list of apps, one row per app, in one order.
export function linkedApps(computers: LinkedComputerEntry[]): LinkedApp[] {
	const apps: LinkedApp[] = [];
	for (const computer of computers) {
		for (const agent of computer.agents) {
			let app = apps.find((a) => a.runtime === agent.runtime);
			if (!app) {
				app = { runtime: agent.runtime, app: agent.app, coding: CODING.has(agent.runtime), choices: [], team: [] };
				apps.push(app);
			}
			if (app.coding && agent.hired) {
				if (agent.employeeId) app.team.push({ employeeId: agent.employeeId, name: agent.name });
			} else {
				app.choices.push({ computer, agent });
			}
		}
	}
	const at = (runtime: string) => {
		const i = ORDER.indexOf(runtime);
		return i < 0 ? ORDER.length : i;
	};
	return apps
		.filter((a) => a.choices.length > 0)
		.map((a, found) => ({ a, found }))
		.sort((x, y) => at(x.a.runtime) - at(y.a.runtime) || x.found - y.found)
		.map(({ a }) => a);
}

// A newer listing laid over the rows shown, never moving them: each row
// keeps its place with what the listing now says of it (or what it said,
// while the listing has nothing of it), and an app found since is added at
// the end.
export function mergeLinkedApps(shown: LinkedApp[], computers: LinkedComputerEntry[]): LinkedApp[] {
	const next = linkedApps(computers);
	const kept = shown.map((row) => next.find((a) => a.runtime === row.runtime) ?? row);
	return [...kept, ...next.filter((a) => !shown.some((row) => row.runtime === a.runtime))];
}

// The one plain line under a row, as an i18n key and its values.
export function appLine(app: LinkedApp): { key: string; values?: Record<string, number> } {
	if (app.coding) return { key: 'newEmployee.linkedHireNew' };
	if (app.choices.length > 1) return { key: 'newEmployee.linkedRunning', values: { count: app.choices.length } };
	if (app.choices[0]?.agent.hired) return { key: 'newEmployee.onYourTeam' };
	return { key: 'newEmployee.linkedConnect' };
}

// The name a new one of a coding app starts with: the app's own for the
// first, none after it (the owner types it).
export function suggestedName(app: LinkedApp): string {
	return app.team.length === 0 ? app.app : '';
}

// The employee already named `name`, whatever its case, if there is one.
export function nameTaken(name: string, names: string[]): string | null {
	const wanted = name.trim().toLowerCase();
	if (!wanted) return null;
	return names.find((n) => n.trim().toLowerCase() === wanted) ?? null;
}

// The create-agent body for an entry: the bot the entry says its hire goes
// to; for a coding agent the permission mode picked, unless it follows the
// company's, and the name the owner gave it.
export function linkedHire(agent: LinkedAgentEntry, mode: LinkedMode, name?: string) {
	const own = CODING.has(agent.runtime) && mode !== 'company';
	const named = name?.trim();
	return {
		linked: { botId: agent.botId, agentId: agent.id, ...(own ? { permissionMode: mode } : {}) },
		...(named ? { name: named } : {})
	};
}
