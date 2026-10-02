// The ONE map of an employee's settings. A TAB is a page in the settings
// modal's nav; a PART is one of the sections a page is built from. The view
// renders each part once, wherever a tab lists it, so a merged page reuses
// the sections it combines instead of copying them. `label` holds an i18n
// key, translated with $t at render time.
//
// Six tabs are everyday. Everything advanced is still here, gated behind
// Developer mode (the bot's own setting, Settings → Developer): a developer
// part inside an everyday tab, and the developer tabs in their own group.
//
// Permissions is the employee's own page of what it may do (its job, mode,
// money limits, folders, always-allowed answers); Settings → Permissions
// holds the company defaults it inherits.

export type AgentSettingsPart =
	| 'general'
	| 'identity'
	| 'persona'
	| 'soul'
	| 'rules'
	| 'configure'
	| 'questions'
	| 'skills'
	| 'workflows'
	| 'reach'
	| 'phone'
	| 'channels'
	| 'accounts'
	| 'permissions'
	| 'webhooks'
	| 'api'
	| 'memory';

export interface AgentSettingsTab {
	id: string;
	label: string;
	/** The sections this page is built from, in order. */
	parts: readonly { id: AgentSettingsPart; developer?: boolean }[];
	/** A tab in the Developer group: shown only in Developer mode. */
	developer?: boolean;
	/** A developer tab that is a door to another page rather than a section. */
	href?: (agentId: string) => string;
}

export const agentSettingsTabs: readonly AgentSettingsTab[] = [
	// Status, model, spending limit, self-improvement, conversations and memory.
	{ id: 'general', label: 'agentSettings.general', parts: [{ id: 'general' }] },
	{
		id: 'who',
		label: 'agentSettings.tabWho',
		parts: [{ id: 'identity' }, { id: 'persona' }, { id: 'soul' }, { id: 'rules' }]
	},
	// Setup answers are everyday; writing the questions and the workflow
	// builder are a builder's tools (the Flows pane stays the everyday place
	// for an employee's workflows).
	{
		id: 'work',
		label: 'agentSettings.tabWork',
		parts: [
			{ id: 'configure' },
			{ id: 'questions', developer: true },
			{ id: 'skills' },
			{ id: 'workflows', developer: true }
		]
	},
	// Where people and other systems reach this employee: its own address,
	// its phone lines and its channels.
	{
		id: 'reach',
		label: 'agentSettings.reach',
		parts: [{ id: 'reach' }, { id: 'phone' }, { id: 'channels' }]
	},
	{ id: 'accounts', label: 'agentSettings.connectedAccounts', parts: [{ id: 'accounts' }] },
	{ id: 'permissions', label: 'permissions.title', parts: [{ id: 'permissions' }] },
	// Developer group. Two doors for outside systems (webhooks push into the
	// employee, the API calls it as a model), the raw memory inspector, and
	// the cases inspector.
	{ id: 'webhooks', label: 'agentSettings.webhooks', parts: [{ id: 'webhooks' }], developer: true },
	{ id: 'api', label: 'agentSettings.apiKeys', parts: [{ id: 'api' }], developer: true },
	{ id: 'memory', label: 'agentSettings.memory', parts: [{ id: 'memory' }], developer: true },
	{
		id: 'cases',
		label: 'cases.title',
		parts: [],
		developer: true,
		href: (agentId) => `/${agentId}/cases`
	}
];

/** The tabs the nav shows: the everyday six, plus the Developer group in Developer mode. */
export function visibleTabs(devMode: boolean): AgentSettingsTab[] {
	return agentSettingsTabs.filter((t) => devMode || !t.developer);
}

/** The parts a tab renders: developer tabs and parts only in Developer mode. */
export function visibleParts(tabId: string, devMode: boolean): AgentSettingsPart[] {
	const tab = agentSettingsTabs.find((t) => t.id === tabId);
	if (!tab || (tab.developer && !devMode)) return [];
	return tab.parts.filter((p) => devMode || !p.developer).map((p) => p.id);
}

/**
 * Where a requested section lands. Requests name either a tab or one of the
 * old per-section pages (deep links, bookmarks, `?settings=persona`): an old
 * name opens the tab it now lives on, focused on that part. A developer page
 * outside Developer mode, or a name nothing knows, opens General.
 */
export function resolveSection(
	requested: string | null | undefined,
	devMode: boolean
): { tab: string; focus: AgentSettingsPart | null } {
	const id = requested || 'general';
	const visible = visibleTabs(devMode).filter((t) => !t.href);
	const direct = visible.find((t) => t.id === id);
	if (direct) return { tab: direct.id, focus: null };
	const holder = visible.find((t) =>
		t.parts.some((p) => p.id === id && (devMode || !p.developer))
	);
	if (holder) return { tab: holder.id, focus: id as AgentSettingsPart };
	return { tab: 'general', focus: null };
}

// The section a plugin's accounts live in: a phone line reads as a capability
// of the employee (its own Phone section); every other plugin's accounts are
// under Connected Accounts. The ONE place that split is decided.
export function accountsSectionFor(pluginSlug: string): 'phone' | 'accounts' {
	return pluginSlug === 'phonecall' ? 'phone' : 'accounts';
}
