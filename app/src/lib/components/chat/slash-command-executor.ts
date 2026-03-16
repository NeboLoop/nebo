/**
 * Slash Command Executor
 * Handles execution of local slash commands and formats output.
 */

import * as api from '$lib/api/nebo';
import { SLASH_COMMANDS } from './slash-commands';

export interface CommandContext {
	messages: { role: string; content: string; timestamp: Date }[];
	chatId: string;
	isLoading: boolean;
	onNewSession: () => void;
	onCancel: () => void;
	onToggleDuplex: (() => void) | undefined;
	addSystemMessage: (content: string) => void;
	clearMessages: () => void;
	setVerboseMode: (on: boolean) => void;
	setThinkingLevel: (level: string) => void;
	toggleFocusMode: () => void;
	wsSend: (type: string, data?: Record<string, unknown>) => void;
}

/**
 * Execute a slash command.
 * Returns true if handled locally, false if it should be sent to the agent.
 */
export async function executeSlashCommand(
	command: string,
	args: string,
	ctx: CommandContext
): Promise<boolean> {
	switch (command) {
		// ── Session ──
		case 'new':
			ctx.onNewSession();
			return true;

		case 'reset':
			ctx.wsSend('session_reset', { session_id: ctx.chatId });
			ctx.addSystemMessage('Session reset.');
			return true;

		case 'clear':
			ctx.clearMessages();
			return true;

		case 'stop':
			if (ctx.isLoading) {
				ctx.onCancel();
			} else {
				ctx.addSystemMessage('Nothing to stop — no generation in progress.');
			}
			return true;

		case 'focus':
			ctx.toggleFocusMode();
			return true;

		case 'compact':
			ctx.wsSend('session_reset', { session_id: ctx.chatId });
			ctx.addSystemMessage('Context compacted (session reset).');
			return true;

		// ── Model ──
		case 'model':
			if (!args) {
				return await handleModelList(ctx);
			}
			// With args — let the agent handle fuzzy matching
			return false;

		case 'think':
			return handleThink(args, ctx);

		case 'verbose':
			return handleVerbose(args, ctx);

		// ── Info ──
		case 'help':
			return handleHelp(ctx);

		case 'status':
			return await handleStatus(ctx);

		case 'usage':
			return await handleUsage(ctx);

		case 'export':
			return handleExport(ctx);

		case 'lanes':
			return await handleLanes(ctx);

		case 'search':
			return await handleSearch(args, ctx);

		// ── Agent ──
		case 'skill':
			return false; // Always send to agent

		case 'memory':
			return await handleMemory(args, ctx);

		case 'heartbeat':
			if (args.toLowerCase() === 'wake') {
				return false; // Send to agent
			}
			return await handleHeartbeat(ctx);

		case 'advisors':
			return await handleAdvisors(ctx);

		case 'voice':
			if (ctx.onToggleDuplex) {
				ctx.onToggleDuplex();
			} else {
				ctx.addSystemMessage('Voice is not available.');
			}
			return true;

		case 'personality':
			return await handlePersonality(ctx);

		case 'wake':
			return false; // Always send to agent

		default:
			return false;
	}
}

// ── Handlers ──

function handleHelp(ctx: CommandContext): boolean {
	const categories = ['session', 'model', 'info', 'agent'] as const;
	const lines: string[] = ['**Slash Commands**\n'];

	for (const cat of categories) {
		lines.push(`**${cat.toUpperCase()}**`);
		for (const cmd of SLASH_COMMANDS.filter((c) => c.category === cat)) {
			const argsHint = cmd.args ? ` \`${cmd.args}\`` : '';
			lines.push(`\`/${cmd.name}\`${argsHint} — ${cmd.description}`);
		}
		lines.push('');
	}

	ctx.addSystemMessage(lines.join('\n'));
	return true;
}

function handleThink(args: string, ctx: CommandContext): boolean {
	const valid = ['off', 'low', 'medium', 'high'];
	const level = args.toLowerCase();
	if (!valid.includes(level)) {
		ctx.addSystemMessage(`Invalid thinking level. Use: ${valid.join(', ')}`);
		return true;
	}
	ctx.setThinkingLevel(level);
	ctx.addSystemMessage(`Thinking mode set to **${level}**.`);
	return true;
}

function handleVerbose(args: string, ctx: CommandContext): boolean {
	const on = args.toLowerCase() !== 'off';
	ctx.setVerboseMode(on);
	ctx.addSystemMessage(`Verbose mode **${on ? 'on' : 'off'}**.`);
	return true;
}

async function handleModelList(ctx: CommandContext): Promise<boolean> {
	try {
		const res = await api.listModels();
		const lines: string[] = ['**Available Models**\n'];

		for (const [provider, models] of Object.entries(res.models || {})) {
			lines.push(`**${provider}**`);
			for (const m of models) {
				const active = m.isActive ? ' (active)' : '';
				const preferred = m.preferred ? ' *' : '';
				lines.push(`- ${m.displayName}${preferred}${active}`);
			}
			lines.push('');
		}

		if (res.aliases?.length) {
			lines.push('**Aliases**');
			for (const a of res.aliases) {
				lines.push(`- ${a.alias} → ${a.modelId}`);
			}
		}

		ctx.addSystemMessage(lines.join('\n'));
	} catch {
		ctx.addSystemMessage('Failed to fetch models.');
	}
	return true;
}

async function handleStatus(ctx: CommandContext): Promise<boolean> {
	try {
		const [statusRes, lanesRes] = await Promise.all([
			api.getSimpleAgentStatus(),
			api.getLanes()
		]);
		const lines: string[] = ['**Agent Status**\n'];
		lines.push(`Connected: ${statusRes.connected ? 'Yes' : 'No'}`);
		if (statusRes.agentId) lines.push(`Agent ID: \`${statusRes.agentId}\``);
		if (statusRes.uptime) {
			const mins = Math.floor(statusRes.uptime / 60);
			const hrs = Math.floor(mins / 60);
			lines.push(`Uptime: ${hrs}h ${mins % 60}m`);
		}
		if (lanesRes.message) {
			lines.push(`\nLanes: ${lanesRes.message}`);
		}
		ctx.addSystemMessage(lines.join('\n'));
	} catch {
		ctx.addSystemMessage('Failed to fetch status.');
	}
	return true;
}

async function handleUsage(ctx: CommandContext): Promise<boolean> {
	try {
		const res = await api.neboLoopJanusUsage();
		const fmt = (w: typeof res.session) => {
			const used = Math.round(w.usedTokens / 1000);
			const limit = Math.round(w.limitTokens / 1000);
			const pct = w.percentUsed.toFixed(1);
			return `${used}K / ${limit}K (${pct}%)`;
		};
		const lines = [
			'**Token Usage**\n',
			`Session: ${fmt(res.session)}`,
			`Weekly: ${fmt(res.weekly)}`
		];
		if (res.weekly.resetAt) {
			lines.push(`Resets: ${new Date(res.weekly.resetAt).toLocaleDateString()}`);
		}
		ctx.addSystemMessage(lines.join('\n'));
	} catch {
		ctx.addSystemMessage('Failed to fetch usage.');
	}
	return true;
}

function handleExport(ctx: CommandContext): boolean {
	const lines: string[] = ['# Chat Export\n'];
	for (const msg of ctx.messages) {
		const role = msg.role === 'user' ? 'You' : msg.role === 'assistant' ? 'Nebo' : 'System';
		const time = msg.timestamp.toLocaleString();
		lines.push(`## ${role} (${time})\n`);
		lines.push(msg.content);
		lines.push('');
	}

	const blob = new Blob([lines.join('\n')], { type: 'text/markdown' });
	const url = URL.createObjectURL(blob);
	const a = document.createElement('a');
	a.href = url;
	a.download = `chat-export-${new Date().toISOString().slice(0, 10)}.md`;
	a.click();
	URL.revokeObjectURL(url);

	ctx.addSystemMessage('Chat exported as Markdown.');
	return true;
}

async function handleLanes(ctx: CommandContext): Promise<boolean> {
	try {
		const res = await api.getLanes();
		ctx.addSystemMessage(`**Lane Status**\n\n${res.message}`);
	} catch {
		ctx.addSystemMessage('Failed to fetch lanes.');
	}
	return true;
}

async function handleSearch(query: string, ctx: CommandContext): Promise<boolean> {
	if (!query) {
		ctx.addSystemMessage('Usage: `/search <query>`');
		return true;
	}
	try {
		const res = await api.searchChatMessages({ query });
		if (!res.messages?.length) {
			ctx.addSystemMessage(`No results for "${query}".`);
			return true;
		}
		const lines = [`**Search Results** (${res.total} match${res.total === 1 ? '' : 'es'})\n`];
		for (const msg of res.messages.slice(0, 10)) {
			const role = msg.role === 'user' ? 'You' : 'Nebo';
			const date = new Date(msg.createdAt).toLocaleDateString();
			const preview = msg.content.length > 120 ? msg.content.slice(0, 120) + '...' : msg.content;
			lines.push(`**${role}** (${date}): ${preview}`);
		}
		if (res.total > 10) {
			lines.push(`\n_...and ${res.total - 10} more_`);
		}
		ctx.addSystemMessage(lines.join('\n'));
	} catch {
		ctx.addSystemMessage('Failed to search messages.');
	}
	return true;
}

async function handleMemory(query: string, ctx: CommandContext): Promise<boolean> {
	try {
		if (query) {
			const res = await api.searchMemories({ query });
			if (!res.memories?.length) {
				ctx.addSystemMessage(`No memories matching "${query}".`);
				return true;
			}
			const lines = [`**Memory Search** (${res.total} result${res.total === 1 ? '' : 's'})\n`];
			for (const m of res.memories.slice(0, 10)) {
				lines.push(`- **${m.key}** [${m.namespace}]: ${m.value.slice(0, 100)}${m.value.length > 100 ? '...' : ''}`);
			}
			ctx.addSystemMessage(lines.join('\n'));
		} else {
			const res = await api.listMemories({});
			if (!res.memories?.length) {
				ctx.addSystemMessage('No memories stored.');
				return true;
			}
			const lines = [`**Memories** (${res.total} total)\n`];
			for (const m of res.memories.slice(0, 15)) {
				lines.push(`- **${m.key}** [${m.namespace}]: ${m.value.slice(0, 80)}${m.value.length > 80 ? '...' : ''}`);
			}
			if (res.total > 15) {
				lines.push(`\n_...and ${res.total - 15} more. Use \`/memory <query>\` to search._`);
			}
			ctx.addSystemMessage(lines.join('\n'));
		}
	} catch {
		ctx.addSystemMessage('Failed to fetch memories.');
	}
	return true;
}

async function handleHeartbeat(ctx: CommandContext): Promise<boolean> {
	try {
		const res = await api.getHeartbeat();
		ctx.addSystemMessage(`**Heartbeat**\n\n${res.content || 'No heartbeat configured.'}`);
	} catch {
		ctx.addSystemMessage('Failed to fetch heartbeat.');
	}
	return true;
}

async function handleAdvisors(ctx: CommandContext): Promise<boolean> {
	try {
		const res = await api.listAdvisors();
		if (!res.advisors?.length) {
			ctx.addSystemMessage('No advisors configured.');
			return true;
		}
		const lines = ['**Advisors**\n'];
		for (const a of res.advisors) {
			const status = a.enabled ? 'enabled' : 'disabled';
			lines.push(`- **${a.name}** (${a.role}) — ${status}, priority ${a.priority}`);
			if (a.description) lines.push(`  ${a.description}`);
		}
		ctx.addSystemMessage(lines.join('\n'));
	} catch {
		ctx.addSystemMessage('Failed to fetch advisors.');
	}
	return true;
}

async function handlePersonality(ctx: CommandContext): Promise<boolean> {
	try {
		const res = await api.getPersonality();
		ctx.addSystemMessage(`**Personality**\n\n${res.content || 'No personality configured.'}`);
	} catch {
		ctx.addSystemMessage('Failed to fetch personality.');
	}
	return true;
}
