/**
 * Slash Command Registry and Parser
 * Defines all available slash commands and provides parsing/completion utilities.
 */

export interface SlashCommand {
	name: string;
	description: string;
	category: 'session' | 'model' | 'info' | 'agent';
	args?: string;
	argOptions?: string[];
	executeLocal: boolean;
}

export const SLASH_COMMANDS: SlashCommand[] = [
	// ── Session ──
	{ name: 'new', description: 'Start new chat session', category: 'session', executeLocal: true },
	{ name: 'reset', description: 'Start fresh (new session)', category: 'session', executeLocal: true },
	{ name: 'clear', description: 'Clear chat display', category: 'session', executeLocal: true },
	{ name: 'stop', description: 'Stop current generation', category: 'session', executeLocal: true },
	{ name: 'focus', description: 'Toggle sidebar', category: 'session', executeLocal: true },
	{ name: 'compact', description: 'Summarize conversation and clear old messages', category: 'session', executeLocal: true },

	// ── Model ──
	{ name: 'model', description: 'Show or switch model', category: 'model', args: '[name]', executeLocal: true },
	{ name: 'think', description: 'Set thinking mode', category: 'model', args: 'off|low|medium|high', argOptions: ['off', 'low', 'medium', 'high'], executeLocal: true },
	{ name: 'verbose', description: 'Toggle tool output detail', category: 'model', args: 'on|off', argOptions: ['on', 'off'], executeLocal: true },

	// ── Info ──
	{ name: 'help', description: 'Show all commands', category: 'info', executeLocal: true },
	{ name: 'status', description: 'Show agent & system status', category: 'info', executeLocal: true },
	{ name: 'usage', description: 'Show token usage', category: 'info', executeLocal: true },
	{ name: 'export', description: 'Export chat as Markdown', category: 'info', executeLocal: true },
	{ name: 'lanes', description: 'Show lane concurrency status', category: 'info', executeLocal: true },
	{ name: 'search', description: 'Search chat history', category: 'info', args: '<query>', executeLocal: true },

	// ── Agent ──
	{ name: 'skill', description: 'Activate a skill by name', category: 'agent', args: '<name>', executeLocal: false },
	{ name: 'memory', description: 'Search or list memories', category: 'agent', args: '[query]', executeLocal: true },
	{ name: 'heartbeat', description: 'Show heartbeat config or wake', category: 'agent', args: '[wake]', executeLocal: true },
	{ name: 'advisors', description: 'List active advisors', category: 'agent', executeLocal: true },
	{ name: 'voice', description: 'Toggle voice conversation', category: 'agent', executeLocal: true },
	{ name: 'personality', description: 'Show current personality', category: 'agent', executeLocal: true },
	{ name: 'wake', description: 'Trigger immediate heartbeat', category: 'agent', args: '[reason]', executeLocal: false },
];

const CATEGORY_ORDER: Record<string, number> = { session: 0, model: 1, info: 2, agent: 3 };

/**
 * Parse a slash command from user input.
 * Returns null if input is not a slash command.
 */
export function parseSlashCommand(input: string): { command: string; args: string } | null {
	const trimmed = input.trim();
	if (!trimmed.startsWith('/')) return null;

	const withoutSlash = trimmed.slice(1);
	const spaceIndex = withoutSlash.indexOf(' ');

	if (spaceIndex === -1) {
		const cmd = withoutSlash.toLowerCase();
		if (SLASH_COMMANDS.some((c) => c.name === cmd)) {
			return { command: cmd, args: '' };
		}
		return null;
	}

	const command = withoutSlash.slice(0, spaceIndex).toLowerCase();
	const args = withoutSlash.slice(spaceIndex + 1).trim();

	if (SLASH_COMMANDS.some((c) => c.name === command)) {
		return { command, args };
	}
	return null;
}

/**
 * Get slash command completions matching a prefix.
 * Sorted by category order (session → model → info → agent).
 */
export function getSlashCommandCompletions(prefix: string): SlashCommand[] {
	const lower = prefix.toLowerCase();
	return SLASH_COMMANDS
		.filter((cmd) => cmd.name.startsWith(lower))
		.sort((a, b) => (CATEGORY_ORDER[a.category] ?? 99) - (CATEGORY_ORDER[b.category] ?? 99));
}
