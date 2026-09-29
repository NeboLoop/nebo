import { describe, it, expect } from 'vitest';
import type { LinkedAgentEntry, LinkedComputerEntry } from '$lib/api/neboComponents';
import { appLine, linkedApps, linkedHire, mergeLinkedApps, nameTaken, suggestedName } from './linkedHire';

const APPS: Record<string, string> = {
	'claude-code': 'Claude Code',
	codex: 'Codex',
	gemini: 'Gemini CLI',
	hermes: 'Hermes',
	openclaw: 'OpenClaw',
	goose: 'goose'
};

function start(runtime: string, botId: string, computer: string): LinkedAgentEntry {
	return {
		id: `new:${runtime}`,
		name: APPS[runtime],
		app: APPS[runtime],
		description: `Works in a new folder on ${computer}`,
		runtime,
		botId,
		hired: false,
		employeeId: null
	};
}

function agent(id: string, name: string, runtime: string, botId: string, employeeId: string | null = null): LinkedAgentEntry {
	return {
		id,
		name,
		app: APPS[runtime],
		description: `${APPS[runtime]} on Mac.lan`,
		runtime,
		botId,
		hired: employeeId !== null,
		employeeId
	};
}

function computer(id: string, agents: LinkedAgentEntry[], local = false): LinkedComputerEntry {
	return { id: `computer:${id}`, name: local ? 'This computer' : id, online: true, local, agents };
}

// The owner's two computers as GET /agents/linked lists them: this one with
// Claude Code (two of them on the team already), the Mac with Claude Code,
// Codex, a Hermes running two agents (one on the team) and OpenClaw
// running one.
const here = computer(
	'Studio',
	[
		agent('frontend', 'Frontend', 'claude-code', 'self', 'e1'),
		agent('backend', 'Backend', 'claude-code', 'self', 'e2'),
		start('claude-code', 'self', 'this computer')
	],
	true
);
const mac = computer('Mac.lan', [
	start('claude-code', 'cc', 'Mac.lan'),
	start('codex', 'cc', 'Mac.lan'),
	agent('research', 'Researcher', 'hermes', 'hm', 'e3'),
	agent('writer', 'Writer', 'hermes', 'hm'),
	agent('assistant', 'OpenClaw', 'openclaw', 'oc')
]);

describe('linkedApps', () => {
	it('lists one row per installed app, coding apps first, in one order', () => {
		const apps = linkedApps([here, mac]);
		expect(apps.map((a) => a.app)).toEqual(['Claude Code', 'Codex', 'Hermes', 'OpenClaw']);
	});

	it("keeps a coding app's hired ones off the row: they are its team, and a new one starts on either computer", () => {
		const [claude] = linkedApps([here, mac]);
		expect(claude.team).toEqual([
			{ employeeId: 'e1', name: 'Frontend' },
			{ employeeId: 'e2', name: 'Backend' }
		]);
		expect(claude.choices.map((c) => c.computer.name)).toEqual(['This computer', 'Mac.lan']);
		expect(claude.choices.every((c) => c.agent.id === 'new:claude-code')).toBe(true);
	});

	it("lists every agent of an app that runs several, the hired one included", () => {
		const hermes = linkedApps([here, mac]).find((a) => a.runtime === 'hermes')!;
		expect(hermes.choices.map((c) => [c.agent.name, c.agent.hired])).toEqual([
			['Researcher', true],
			['Writer', false]
		]);
	});
});

describe('appLine', () => {
	it('says one plain line under each row', () => {
		const line = Object.fromEntries(linkedApps([here, mac]).map((a) => [a.runtime, appLine(a)]));
		expect(line['claude-code']).toEqual({ key: 'newEmployee.linkedHireNew' });
		expect(line.hermes).toEqual({ key: 'newEmployee.linkedRunning', values: { count: 2 } });
		expect(line.openclaw).toEqual({ key: 'newEmployee.linkedConnect' });
		const hired = linkedApps([computer('Mac.lan', [agent('assistant', 'OpenClaw', 'openclaw', 'oc', 'e9')])]);
		expect(appLine(hired[0])).toEqual({ key: 'newEmployee.onYourTeam' });
	});
});

describe('mergeLinkedApps', () => {
	it('never moves a row: an app found later is added at the end', () => {
		const shown = linkedApps([computer('Mac.lan', [start('codex', 'cc', 'Mac.lan'), agent('assistant', 'OpenClaw', 'openclaw', 'oc')])]);
		const later = [
			computer('Mac.lan', [
				start('claude-code', 'cc', 'Mac.lan'),
				start('codex', 'cc', 'Mac.lan'),
				agent('assistant', 'OpenClaw', 'openclaw', 'oc', 'e4')
			])
		];
		const merged = mergeLinkedApps(shown, later);
		expect(merged.map((a) => a.app)).toEqual(['Codex', 'OpenClaw', 'Claude Code']);
		expect(merged[1].choices[0].agent.hired).toBe(true);
	});

	it('keeps a row the newer listing has nothing of', () => {
		const shown = linkedApps([mac]);
		expect(mergeLinkedApps(shown, []).map((a) => a.app)).toEqual(shown.map((a) => a.app));
	});

	it('lays the first listing out whole, in order', () => {
		expect(mergeLinkedApps([], [here, mac]).map((a) => a.app)).toEqual(['Claude Code', 'Codex', 'Hermes', 'OpenClaw']);
	});
});

describe('naming a new one', () => {
	it("starts the first of an app with the app's name, the next with none", () => {
		const apps = linkedApps([here, mac]);
		expect(suggestedName(apps.find((a) => a.runtime === 'codex')!)).toBe('Codex');
		expect(suggestedName(apps.find((a) => a.runtime === 'claude-code')!)).toBe('');
	});

	it('refuses a name already on the team, whatever its case', () => {
		expect(nameTaken(' frontend ', ['Frontend', 'Backend'])).toBe('Frontend');
		expect(nameTaken('Docs', ['Frontend', 'Backend'])).toBeNull();
		expect(nameTaken('  ', ['Frontend'])).toBeNull();
	});
});

describe('linkedHire', () => {
	it('starts a new coding agent through the bot its entry names, with the name and mode picked', () => {
		expect(linkedHire(start('claude-code', 'cc', 'Mac.lan'), 'ask', ' Docs ')).toEqual({
			linked: { botId: 'cc', agentId: 'new:claude-code', permissionMode: 'ask' },
			name: 'Docs'
		});
	});

	it('leaves the mode to the company when the company is picked', () => {
		expect(linkedHire(start('codex', 'cc', 'Mac.lan'), 'company', 'Codex')).toEqual({
			linked: { botId: 'cc', agentId: 'new:codex' },
			name: 'Codex'
		});
	});

	it("hires an app's agent from its own bot, as it is named there", () => {
		expect(linkedHire(agent('writer', 'Writer', 'hermes', 'hm'), 'company')).toEqual({
			linked: { botId: 'hm', agentId: 'writer' }
		});
	});
});

describe('the hire list', () => {
	it('never polls: it answers from what Nebo knows and listens for what changes', async () => {
		const modal = (await import('$lib/components/NewEmployeeModal.svelte?raw')).default;
		expect(modal).not.toMatch(/setInterval/);
		expect(modal).toContain("onWsEvent<ListLinkedAgentsResponse>('linked_apps_changed'");
	});
});
