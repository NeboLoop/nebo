<script lang="ts">
	import { FileEdit, FileText, Terminal, Search, Globe, Check, Loader2 } from 'lucide-svelte';

	interface Props {
		name: string;
		input?: string;
		output?: string;
		status?: 'running' | 'complete' | 'error';
		selected?: boolean;
		onclick?: () => void;
	}

	let { name, input = '', output = '', status = 'running', selected = false, onclick }: Props = $props();

	function getToolIcon(toolName: string) {
		const lower = toolName.toLowerCase();
		if (lower.includes('write') || lower.includes('edit')) return FileEdit;
		if (lower.includes('read') || lower.includes('file')) return FileText;
		if (lower.includes('bash') || lower.includes('shell') || lower.includes('exec')) return Terminal;
		if (lower.includes('search') || lower.includes('grep') || lower.includes('glob')) return Search;
		if (lower.includes('web') || lower.includes('fetch')) return Globe;
		return FileText;
	}

	function formatToolName(toolName: string): string {
		// Strip MCP prefix: "mcp__nebo-agent__shell" → "shell"
		if (toolName.startsWith('mcp__')) {
			const parts = toolName.split('__');
			toolName = parts[parts.length - 1];
		}
		const domainMatch = toolName.match(/^(\w+)\(/);
		if (domainMatch) {
			const actionMatch = toolName.match(/action:\s*(\w+)/);
			if (actionMatch) return actionMatch[1];
			return domainMatch[1];
		}
		return toolName;
	}

	function extractPath(inputStr: string): string {
		if (!inputStr) return '';
		try {
			const parsed = JSON.parse(inputStr);
			// Try common field names
			if (parsed.command) return truncate(parsed.command, 80);
			if (parsed.path) return parsed.path.replace(/^\/Users\/\w+/, '~');
			if (parsed.file_path) return parsed.file_path.replace(/^\/Users\/\w+/, '~');
			if (parsed.query) return truncate(parsed.query, 80);
			if (parsed.url) return truncate(parsed.url, 80);
			if (parsed.pattern) return truncate(parsed.pattern, 80);
			// For generic tool inputs, show first meaningful value
			for (const key of Object.keys(parsed)) {
				const val = parsed[key];
				if (typeof val === 'string' && val.length > 0 && val.length < 200) {
					return truncate(val, 80);
				}
			}
		} catch {
			// Not JSON - try to extract from string
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
	const displayName = $derived(formatToolName(name));
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
