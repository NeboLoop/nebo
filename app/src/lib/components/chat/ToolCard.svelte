<script lang="ts">
	import { FileEdit, FileText, Terminal, Search, Globe, Check, Loader2, Zap } from 'lucide-svelte';

	interface Props {
		name: string;
		input?: unknown;
		output?: string;
		status?: 'running' | 'complete' | 'error';
		selected?: boolean;
		onclick?: () => void;
	}

	let { name, input = '', output = '', status = 'running', selected = false, onclick }: Props = $props();

	function getToolIcon(toolName: string) {
		const lower = toolName.toLowerCase();
		if (lower === 'skill') return Zap;
		if (lower.includes('write') || lower.includes('edit')) return FileEdit;
		if (lower.includes('read') || lower.includes('file')) return FileText;
		if (lower.includes('bash') || lower.includes('shell') || lower.includes('exec')) return Terminal;
		if (lower.includes('search') || lower.includes('grep') || lower.includes('glob')) return Search;
		if (lower.includes('web') || lower.includes('fetch')) return Globe;
		return FileText;
	}

	function toInputObject(inputVal: unknown): Record<string, unknown> | null {
		if (!inputVal) return null;
		if (typeof inputVal === 'object') return inputVal as Record<string, unknown>;
		if (typeof inputVal !== 'string') return null;
		try {
			const parsed = JSON.parse(inputVal);
			return parsed && typeof parsed === 'object' ? (parsed as Record<string, unknown>) : null;
		} catch {
			return null;
		}
	}

	function toInputString(inputVal: unknown): string {
		if (typeof inputVal === 'string') return inputVal;
		if (inputVal == null) return '';
		if (typeof inputVal === 'object') {
			try {
				return JSON.stringify(inputVal);
			} catch {
				return String(inputVal);
			}
		}
		return String(inputVal);
	}

	function formatToolName(toolName: string, inputVal?: unknown): string {
		// Strip MCP prefix: "mcp__nebo-agent__shell" → "shell"
		if (toolName.startsWith('mcp__')) {
			const parts = toolName.split('__');
			toolName = parts[parts.length - 1];
		}
		// Extract domain from tool name
		let domain = toolName;
		const domainMatch = toolName.match(/^(\w+)\(/);
		if (domainMatch) {
			domain = domainMatch[1];
		}
		// Build resource:action or domain:action from input
		const parsed = toInputObject(inputVal);
		if (parsed) {
			const action = parsed.action;
			const resource = parsed.resource;
			if (typeof action === 'string' && action.length > 0) {
				if (typeof resource === 'string' && resource.length > 0) return `${resource}:${action}`;
					return `${domain}:${action}`;
			}
		}
		// Fallback: check if action is embedded in tool name string
		if (domainMatch) {
			const actionMatch = toolName.match(/action:\s*(\w+)/);
			if (actionMatch) return actionMatch[1];
			return domain;
		}
		return toolName;
	}

	function extractPath(inputVal: unknown): string {
		const inputStr = toInputString(inputVal);
		if (!inputStr) return '';
		const parsed = toInputObject(inputVal);
		if (parsed) {
			// Skill tool: show "action: skill-name"
			const action = typeof parsed.action === 'string' ? parsed.action : '';
			if (action && name === 'skill') {
				if (typeof parsed.name === 'string' && parsed.name.length > 0) return `${action}: ${parsed.name}`;
				return action;
			}
			// Priority fields — ordered by specificity for best subtitle
			if (typeof parsed.command === 'string') return truncate(parsed.command, 80);
			if (typeof parsed.path === 'string') return parsed.path.replace(/^\/Users\/\w+/, '~');
			if (typeof parsed.file_path === 'string') return parsed.file_path.replace(/^\/Users\/\w+/, '~');
			if (typeof parsed.query === 'string') return truncate(parsed.query, 80);
			if (typeof parsed.url === 'string') return truncate(parsed.url, 80);
			if (typeof parsed.pattern === 'string') return truncate(parsed.pattern, 80);
			if (typeof parsed.regex === 'string') return truncate(parsed.regex, 80);
			// Named entities (tasks, events, apps, skills, memory keys)
			if (typeof parsed.subject === 'string') return truncate(parsed.subject, 80);
			if (typeof parsed.name === 'string') return truncate(parsed.name, 80);
			if (typeof parsed.key === 'string') return truncate(parsed.key, 80);
			// Browser element targeting
			if (typeof parsed.ref === 'string') return parsed.ref;
			if (typeof parsed.selector === 'string') return truncate(parsed.selector, 80);
			// Messaging targets
			if (typeof parsed.to === 'string') return truncate(parsed.to, 80);
			if (typeof parsed.topic === 'string') return truncate(parsed.topic, 80);
			// Identifiers
			if (typeof parsed.task_id === 'string') return parsed.task_id;
			if (typeof parsed.agent_id === 'string') return parsed.agent_id;
			if (typeof parsed.session_id === 'string') return parsed.session_id;
			if (typeof parsed.channel_id === 'string') return parsed.channel_id;
			if (typeof parsed.id === 'string') return truncate(parsed.id, 80);
			// Text content (truncate shorter — these can be long)
			if (typeof parsed.text === 'string') return truncate(parsed.text, 60);
			if (typeof parsed.prompt === 'string') return truncate(parsed.prompt, 60);
			if (typeof parsed.message === 'string') return truncate(parsed.message, 60);
			if (typeof parsed.description === 'string') return truncate(parsed.description, 60);
			// Status / metadata
			if (typeof parsed.status === 'string') return parsed.status;
			if (typeof parsed.image === 'string') return truncate(parsed.image, 80);
			// Generic fallback — first meaningful string value (skip action/resource — shown in title)
			for (const key of Object.keys(parsed)) {
				if (key === 'action' || key === 'resource') continue;
				const val = parsed[key];
				if (typeof val === 'string' && val.length > 0 && val.length < 200) {
					return truncate(val, 80);
				}
			}
		}
		// Try to extract path from string format
		const pathMatch = inputStr.match(/path['\":\s]+([^\s'\"]+)/i);
		if (pathMatch) return pathMatch[1].replace(/^\/Users\/\w+/, '~');
		// Just show truncated raw input as fallback
		if (inputStr.length > 0) {
			return truncate(inputStr, 80);
		}
		return '';
	}

	function truncate(str: string, maxLen: number): string {
		if (str.length <= maxLen) return str;
		return str.slice(0, maxLen) + '...';
	}

	const ToolIcon = $derived(getToolIcon(name));
	const displayName = $derived(formatToolName(name, input));
	const path = $derived(extractPath(input));
</script>

<button
	type="button"
	class="w-full text-left px-4 py-3 rounded-lg border transition-all duration-150 {selected ? 'border-primary bg-primary/5' : 'border-base-300 bg-base-200/30 hover:bg-base-200/50'}"
	onclick={onclick}
>
	<div class="flex items-start gap-3">
		<div class="shrink-0 mt-0.5 text-base-content/60">
			<svelte:component this={ToolIcon} class="w-4 h-4" />
		</div>

		<div class="flex-1 min-w-0">
			<div class="font-medium text-sm text-base-content">{displayName}</div>
			{#if path}
				<div class="text-xs text-base-content/50 truncate mt-0.5">{path}</div>
			{/if}
			<div class="text-xs mt-1 {status === 'complete' ? 'text-base-content/40' : status === 'running' ? 'text-warning' : 'text-error'}">
				{status === 'running' ? 'Running...' : status === 'complete' ? 'Completed' : 'Error'}
			</div>
		</div>

		<div class="shrink-0">
			{#if status === 'running'}
				<Loader2 class="w-4 h-4 animate-spin text-warning" />
			{:else if status === 'complete'}
				<Check class="w-4 h-4 text-success" />
			{:else}
				<span class="text-error text-xs">!</span>
			{/if}
		</div>
	</div>
</button>
