// The ONE list of an employee's settings sections — the modal's nav renders
// it and the view switches on the ids. `label` holds an i18n key, translated
// with $t at render time.
//
// Permissions is the employee's own page of what it may do (its job, mode,
// money limits, folders, always-allowed answers); Settings → Permissions
// holds the company defaults it inherits.
export const agentSettingsSections = [
	{ id: 'general', label: 'agentSettings.general' },
	{ id: 'identity', label: 'settings.navItems.identity' },
	{ id: 'persona', label: 'agentPersona.title' },
	{ id: 'soul', label: 'settings.navItems.soul' },
	{ id: 'rules', label: 'settings.navItems.rules' },
	{ id: 'configure', label: 'agent.configure' },
	{ id: 'skills', label: 'settings.navItems.skills' },
	{ id: 'channels', label: 'agentSettings.channels' },
	{ id: 'accounts', label: 'agentSettings.connectedAccounts' },
	{ id: 'phone', label: 'agentSettings.phone' },
	// Two doors for outside systems: webhooks push into the employee, the
	// API lets a client call it as a model. Each is its own page.
	{ id: 'webhooks', label: 'agentSettings.webhooks' },
	{ id: 'api', label: 'agentSettings.apiKeys' },
	{ id: 'permissions', label: 'permissions.title' },
	{ id: 'memory', label: 'agentSettings.memory' }
] as const;

// The section a plugin's accounts live in: a phone line reads as a capability
// of the employee (its own Phone section); every other plugin's accounts are
// under Connected Accounts. The ONE place that split is decided.
export function accountsSectionFor(pluginSlug: string): 'phone' | 'accounts' {
	return pluginSlug === 'phonecall' ? 'phone' : 'accounts';
}
