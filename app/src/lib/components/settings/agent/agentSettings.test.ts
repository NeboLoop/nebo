import { describe, it, expect, vi, beforeAll } from 'vitest';
import { render } from 'svelte/server';
import { addMessages, init } from 'svelte-i18n';
import en from '$lib/i18n/locales/en.json';

// Rendering settings must never reach a server.
vi.mock('$lib/api/nebo', () => ({
	getSettings: vi.fn(async () => ({ settings: { developerMode: false, appDeveloperMode: false } })),
	updateSettings: vi.fn(async () => ({ settings: { developerMode: false, appDeveloperMode: false } }))
}));

// Node's own localStorage stub has no methods; per-install state is memory here.
vi.mock('$lib/storage', () => {
	const m = new Map<string, string>();
	return {
		storage: {
			get: (k: string) => m.get(k) ?? null,
			set: (k: string, v: string) => void m.set(k, v),
			remove: (k: string) => void m.delete(k)
		}
	};
});

// The channels help chat pulls in the chat pane, which pulls in the remote
// desktop viewer; it touches `window` on import.
vi.mock('@novnc/novnc', () => ({ default: class {} }));

import AgentSettingsView from './AgentSettingsView.svelte';
import AgentSettingsModal from './AgentSettingsModal.svelte';
import viewSource from './AgentSettingsView.svelte?raw';
import { agentSettingsTabs, resolveSection, visibleParts, visibleTabs } from './sections';
import { devMode } from '$lib/stores/devmode';

beforeAll(() => {
	addMessages('en', en);
	init({ fallbackLocale: 'en', initialLocale: 'en' });
});

const EVERYDAY = ['general', 'who', 'work', 'reach', 'accounts', 'permissions'];
const DEVELOPER = ['webhooks', 'api', 'memory', 'cases'];

const agent = {
	id: 'pat',
	name: 'Pat',
	role: 'Bookkeeper',
	color: 'violet',
	editable: true,
	status: 'online',
	installedAt: 1_700_000_000
};
const workflow = { trigger: { type: 'manual' }, description: 'Close the month', isActive: true, activities: [] };
const ctx = {
	agentId: 'pat',
	agent,
	agentColor: null,
	skills: ['ledger'],
	config: { persona: '', agentMd: '', soul: '', rules: '', model: '', inputs: [], workflows: { 'Month end': workflow } },
	workflowEntries: [['Month end', workflow]],
	workflowStats: { totalRuns: 0, completed: 0, failed: 0, avgDuration: '' },
	roster: [],
	agentStatus: () => 'online',
	toggleAgentStatus: () => {},
	openWorkflow: () => {},
	openCanvas: () => {},
	toggleWorkflow: () => {},
	refreshAgent: async () => {}
};

function view(tab: string, dev: boolean, focus: string | null = null) {
	devMode.set(dev);
	return render(AgentSettingsView, {
		props: { tab, focus: focus as never },
		context: new Map([['agentPage', ctx]])
	}).body;
}

function modal(section: string, dev: boolean) {
	devMode.set(dev);
	return render(AgentSettingsModal, {
		props: { open: true, agentId: 'pat', section, agentName: 'Pat', onsection: () => {}, onclose: () => {} },
		context: new Map([['agentPage', ctx]])
	}).body;
}

describe('employee settings tabs', () => {
	it('shows the six everyday tabs with Developer mode off, and nothing else', () => {
		expect(visibleTabs(false).map((t) => t.id)).toEqual(EVERYDAY);
		const body = modal('general', false);
		for (const id of EVERYDAY) {
			const tab = agentSettingsTabs.find((t) => t.id === id)!;
			expect(body).toContain(en.agentSettings[tab.label.split('.')[1] as keyof typeof en.agentSettings] ?? '');
		}
		expect(body).not.toContain(en.agentSettings.developerGroup);
		expect(body).not.toContain(en.agentSettings.webhooks);
		expect(body).not.toContain(en.agentSettings.apiKeys);
		expect(body).not.toContain('/pat/cases');
	});

	it('adds the Developer group with Developer mode on', () => {
		expect(visibleTabs(true).map((t) => t.id)).toEqual([...EVERYDAY, ...DEVELOPER]);
		const body = modal('general', true);
		expect(body).toContain(en.agentSettings.developerGroup);
		expect(body).toContain(en.agentSettings.webhooks);
		expect(body).toContain(en.agentSettings.apiKeys);
		expect(body).toContain(en.agentSettings.memory);
		expect(body).toContain('href="/pat/cases"');
	});

	it('gates the builder parts of What they do', () => {
		expect(visibleParts('work', false)).toEqual(['configure', 'skills']);
		expect(visibleParts('work', true)).toEqual(['configure', 'questions', 'skills', 'workflows']);
	});

	it('lands every old section name on the page it now lives on', () => {
		const lands: Record<string, [string, string | null]> = {
			general: ['general', null],
			identity: ['who', 'identity'],
			persona: ['who', 'persona'],
			soul: ['who', 'soul'],
			rules: ['who', 'rules'],
			configure: ['work', 'configure'],
			skills: ['work', 'skills'],
			reach: ['reach', null],
			channels: ['reach', 'channels'],
			phone: ['reach', 'phone'],
			accounts: ['accounts', null],
			permissions: ['permissions', null]
		};
		for (const [old, [tab, focus]] of Object.entries(lands)) {
			expect(resolveSection(old, false)).toEqual({ tab, focus });
		}
		// Developer pages need Developer mode; without it they open General.
		for (const old of ['webhooks', 'api', 'memory', 'workflows']) {
			expect(resolveSection(old, false).tab).toBe('general');
		}
		expect(resolveSection('webhooks', true)).toEqual({ tab: 'webhooks', focus: null });
		expect(resolveSection('workflows', true)).toEqual({ tab: 'work', focus: 'workflows' });
		expect(resolveSection('nonsense', true)).toEqual({ tab: 'general', focus: null });
		expect(resolveSection(null, false)).toEqual({ tab: 'general', focus: null });
	});
});

describe('merged pages keep every field', () => {
	it('General: status, model, spending limit, self-improvement, conversations and memory, housekeeping', () => {
		const body = view('general', false);
		for (const s of [
			en.agentSettings.general,
			en.common.online,
			en.sidebar.pause,
			en.runLimit.title,
			en.settingsSkills.title,
			en.marketplace.workflows,
			en.sidebar.duplicate,
			en.agentSettings.dangerZone,
			en.agentSettings.exportData,
			en.agentSettings.purgeData,
			en.agentSettings.deleteAgent
		]) {
			expect(body).toContain(s);
		}
		// Model, self-improvement and memory read the employee before they
		// draw anything, so a server render can't show them: the page's source
		// must still build General from all four, in this order.
		const general = viewSource.slice(viewSource.indexOf('{#snippet generalPart()}'), viewSource.indexOf('{#snippet identityPart()}'));
		const order = ['<ModelControls', '<RunLimitControls', '<LearningControls', '<IsolationControls'].map((c) => general.indexOf(c));
		expect(order.every((i) => i > 0)).toBe(true);
		expect([...order].sort((a, b) => a - b)).toEqual(order);
	});

	it('Who they are: Identity, Persona, Soul and Rules on one page', () => {
		const body = view('who', false);
		for (const s of [
			en.settings.navItems.identity,
			en.agentSettings.agentName,
			en.agentSettings.voice,
			en.agentSettings.role,
			en.agentSettings.color,
			en.agentSettings.structure,
			en.agentSettings.department,
			en.agentSettings.reportsTo,
			en.automations.status,
			en.agentPersona.title,
			en.agentSettings.personaPlaceholder,
			en.settings.navItems.soul,
			en.agentSettings.soulPlaceholder,
			en.settings.navItems.rules,
			en.agentSettings.rulesPlaceholder
		]) {
			expect(body).toContain(s);
		}
		for (const id of ['identity', 'persona', 'soul', 'rules']) {
			expect(body).toContain(`id="agent-settings-${id}"`);
		}
	});

	it('What they do: setup answers and skills; the builder tools in Developer mode', () => {
		const off = view('work', false);
		expect(off).toContain(en.agent.configure);
		expect(off).toContain(en.settings.navItems.skills);
		expect(off).toContain('ledger');
		expect(off).not.toContain(en.agentQuestions.title);
		expect(off).not.toContain(en.flows.newCallTree);

		const on = view('work', true);
		for (const s of [
			en.agent.configure,
			en.agentQuestions.title,
			en.agentQuestions.add,
			en.settings.navItems.skills,
			en.marketplace.workflows,
			'Month end',
			en.agentSettings.newWorkflow,
			en.flows.newCallTree
		]) {
			expect(on).toContain(s);
		}
	});

	it('How to reach them: address, Phone and Channels on one page', () => {
		const body = view('reach', false);
		for (const s of [
			en.agentSettings.reach,
			en.agentSettings.reachEmail,
			en.agentSettings.phone,
			en.agentSettings.channels,
			en.agentSettings.exposeToLoop
		]) {
			expect(body).toContain(s);
		}
		for (const id of ['reach', 'phone', 'channels']) {
			expect(body).toContain(`id="agent-settings-${id}"`);
		}
	});

	it('Accounts and Permissions keep their pages', () => {
		expect(view('accounts', false)).toContain(en.agentSettings.connectedAccounts);
		expect(view('permissions', false)).toContain(en.permissions.title);
	});

	it('Developer pages render only in Developer mode', () => {
		expect(view('webhooks', true)).toContain(en.agentSettings.webhookNew);
		expect(view('api', true)).toContain(en.agentSettings.apiKeys);
		expect(view('webhooks', false)).not.toContain(en.agentSettings.webhookNew);
		expect(view('api', false)).not.toContain(`id="agent-settings-api"`);
		expect(view('memory', false)).not.toContain(`id="agent-settings-memory"`);
		expect(view('memory', true)).toContain(`id="agent-settings-memory"`);
	});
});
