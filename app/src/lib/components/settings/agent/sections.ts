// The ONE list of an employee's settings sections — the modal's nav renders
// it and the view switches on the ids. `label` holds an i18n key, translated
// with $t at render time.
//
// Ambient capability permissions (file/shell/web…) are managed once, globally,
// in Settings → Permissions and inherited by every employee. Approvals is
// deliberately per-employee: the three-state control over each employee's
// gated operations, so a global setting never overrides a critical decision.
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
	// Connect: webhooks and API keys — every way an outside system reaches
	// this employee, minted and revoked in one place.
	{ id: 'webhooks', label: 'agentSettings.webhooks' },
	{ id: 'approvals', label: 'agentSettings.approvals' },
	{ id: 'memory', label: 'agentSettings.memory' }
] as const;
