import { describe, it, expect } from 'vitest';
import type { LinkedComputerEntry } from '$lib/api/neboComponents';
import { anyCoding, linkedHire } from './linkedHire';

// The owner's Mac with three apps linked, as GET /agents/linked lists it:
// the Mac once, its apps' agents, and the coding employees it can start.
const mac: LinkedComputerEntry = {
	id: 'computer:Mac.lan',
	name: 'Mac.lan',
	online: true,
	local: false,
	agents: [
		{ id: 'codex', name: 'Codex', description: 'Codex on Mac.lan', runtime: 'codex', botId: 'cc', hired: false },
		{ id: 'hermes', name: 'Hermes', description: 'Hermes on Mac.lan', runtime: 'hermes', botId: 'hm', hired: true },
		{ id: 'assistant', name: 'OpenClaw', description: 'OpenClaw on Mac.lan', runtime: 'openclaw', botId: 'oc', hired: false }
	],
	new: [{ id: 'new:gemini', name: 'Gemini CLI', description: 'Works in a new folder on Mac.lan', runtime: 'gemini', botId: 'cc' }]
};

describe('linkedHire', () => {
	it('starts a coding employee through the bot its row names, with the mode picked', () => {
		expect(linkedHire(mac.new[0], 'ask')).toEqual({ botId: 'cc', agentId: 'new:gemini', permissionMode: 'ask' });
	});

	it("hires an app's agent from its own bot, with a mode only for a coding agent", () => {
		expect(linkedHire(mac.agents[2], 'ask')).toEqual({ botId: 'oc', agentId: 'assistant' });
		expect(linkedHire(mac.agents[0], 'plan')).toEqual({ botId: 'cc', agentId: 'codex', permissionMode: 'plan' });
	});
});

describe('anyCoding', () => {
	it('asks for a mode when a coding employee can be started or a coding agent hired', () => {
		expect(anyCoding([mac])).toBe(true);
		expect(anyCoding([{ ...mac, new: [], agents: mac.agents.slice(1) }])).toBe(false);
		expect(anyCoding([{ ...mac, new: [], agents: [{ ...mac.agents[0], hired: true }] }])).toBe(false);
	});
});
