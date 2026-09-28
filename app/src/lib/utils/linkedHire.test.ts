import { describe, it, expect } from 'vitest';
import type { LinkedComputerEntry } from '$lib/api/neboComponents';
import { anyCoding, linkedHire } from './linkedHire';

// The owner's Mac as GET /agents/linked lists it: one entry per app
// installed there, each hired from the bot it names.
const mac: LinkedComputerEntry = {
	id: 'computer:Mac.lan',
	name: 'Mac.lan',
	online: true,
	local: false,
	agents: [
		{ id: 'new:claude-code', name: 'Claude Code', description: 'Works in a new folder on Mac.lan', runtime: 'claude-code', botId: 'cc', hired: false },
		{ id: 'codex', name: 'Codex', description: 'Codex on Mac.lan', runtime: 'codex', botId: 'cc', hired: false },
		{ id: 'assistant', name: 'OpenClaw', description: 'OpenClaw on Mac.lan', runtime: 'openclaw', botId: 'oc', hired: false },
		{ id: 'hermes', name: 'Hermes', description: 'Hermes on Mac.lan', runtime: 'hermes', botId: 'hm', hired: true }
	]
};

describe('linkedHire', () => {
	it('starts a coding agent through the bot its entry names, with the mode picked', () => {
		expect(linkedHire(mac.agents[0], 'ask')).toEqual({ botId: 'cc', agentId: 'new:claude-code', permissionMode: 'ask' });
		expect(linkedHire(mac.agents[1], 'plan')).toEqual({ botId: 'cc', agentId: 'codex', permissionMode: 'plan' });
	});

	it('leaves the mode to the company when the company is picked', () => {
		expect(linkedHire(mac.agents[0], 'company')).toEqual({ botId: 'cc', agentId: 'new:claude-code' });
	});

	it("hires an app's agent from its own bot, with no mode", () => {
		expect(linkedHire(mac.agents[2], 'ask')).toEqual({ botId: 'oc', agentId: 'assistant' });
	});
});

describe('anyCoding', () => {
	it('asks for a mode only when a coding agent is there to hire', () => {
		expect(anyCoding([mac])).toBe(true);
		expect(anyCoding([{ ...mac, agents: mac.agents.slice(2) }])).toBe(false);
		expect(anyCoding([{ ...mac, agents: [{ ...mac.agents[1], hired: true }] }])).toBe(false);
	});
});
