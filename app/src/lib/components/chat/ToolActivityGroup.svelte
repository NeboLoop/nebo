<script lang="ts">
	import { ChevronDown, ChevronRight, Loader2, CheckCircle2, AlertCircle, FileEdit, FileText, Terminal, Search, Globe, Zap } from 'lucide-svelte';

	interface ToolCall {
		name: string;
		input: string;
		output?: string;
		status?: 'running' | 'complete' | 'error';
	}

	interface Props {
		tools: ToolCall[];
		onViewToolOutput?: (tool: ToolCall) => void;
	}

	let { tools, onViewToolOutput }: Props = $props();

	const isRunning = $derived(tools.some(t => t.status === 'running'));
	const hasError = $derived(tools.some(t => t.status === 'error'));

	let userOverride = $state<boolean | null>(null);
	const isExpanded = $derived(userOverride ?? isRunning);

	let wasRunning = $state(false);
	$effect(() => {
		if (wasRunning && !isRunning) {
			userOverride = null;
		}
		wasRunning = isRunning;
	});

	function toggle() {
		userOverride = !isExpanded;
	}

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

	function formatToolName(toolName: string, inputVal?: unknown): string {
		if (toolName.startsWith('mcp__')) {
			const parts = toolName.split('__');
			toolName = parts[parts.length - 1];
		}
		let domain = toolName;
		const domainMatch = toolName.match(/^(\w+)\(/);
		if (domainMatch) domain = domainMatch[1];
		const parsed = toInputObject(inputVal);
		if (parsed) {
			const action = parsed.action;
			const resource = parsed.resource;
			const server = parsed.server;
			if (typeof action === 'string' && action.length > 0) {
				if (domain === 'mcp' && typeof server === 'string' && typeof resource === 'string') {
					return `${server} ${resource}:${action}`;
				}
				if (typeof resource === 'string' && resource.length > 0) return `${resource}:${action}`;
				return `${domain}:${action}`;
			}
		}
		if (domainMatch) {
			const actionMatch = toolName.match(/action:\s*(\w+)/);
			if (actionMatch) return actionMatch[1];
			return domain;
		}
		return toolName;
	}

	function extractDescription(tool: ToolCall): string {
		const parsed = toInputObject(tool.input);
		if (!parsed) return '';
		if (typeof parsed.command === 'string') return truncate(parsed.command, 80);
		if (typeof parsed.path === 'string') return parsed.path.replace(/^\/Users\/\w+/, '~');
		if (typeof parsed.file_path === 'string') return parsed.file_path.replace(/^\/Users\/\w+/, '~');
		if (typeof parsed.query === 'string') return truncate(parsed.query, 80);
		if (typeof parsed.url === 'string') return truncate(parsed.url, 80);
		if (typeof parsed.pattern === 'string') return truncate(parsed.pattern, 80);
		if (typeof parsed.subject === 'string') return truncate(parsed.subject, 80);
		if (typeof parsed.name === 'string') return truncate(parsed.name as string, 80);
		if (typeof parsed.text === 'string') return truncate(parsed.text, 80);
		if (typeof parsed.prompt === 'string') return truncate(parsed.prompt, 80);
		if (typeof parsed.message === 'string') return truncate(parsed.message, 80);
		return '';
	}

	function extractOutputPreview(output: string | undefined): string {
		if (!output) return '';
		const lines = output.split('\n').filter(l => l.trim().length > 0);
		if (lines.length === 0) return '';
		const first = lines[0].trim();
		if (first.startsWith('{') || first.startsWith('[')) {
			try {
				const parsed = JSON.parse(output);
				if (typeof parsed === 'string') return truncate(parsed, 80);
				if (parsed?.message) return truncate(String(parsed.message), 80);
				if (parsed?.result) return truncate(String(parsed.result), 80);
				if (parsed?.summary) return truncate(String(parsed.summary), 80);
			} catch { /* not JSON */ }
		}
		return truncate(first, 100);
	}

	function truncate(str: string, maxLen: number): string {
		if (str.length <= maxLen) return str;
		return str.slice(0, maxLen) + '\u2026';
	}

	const summary = $derived(() => {
		const count = tools.length;
		return `Used ${count} tool${count !== 1 ? 's' : ''}`;
	});
</script>

<div class="border border-base-300/60 rounded-xl overflow-hidden">
	<button
		type="button"
		class="flex items-center gap-2 w-full px-3 py-2 bg-transparent text-base-content text-sm cursor-pointer text-left hover:bg-base-300/30 transition-colors"
		onclick={toggle}
	>
		{#if isExpanded}
			<ChevronDown class="w-3.5 h-3.5 shrink-0 opacity-50" />
		{:else}
			<ChevronRight class="w-3.5 h-3.5 shrink-0 opacity-50" />
		{/if}
		{#if isRunning}
			<Loader2 class="w-3.5 h-3.5 shrink-0 animate-spin text-primary" />
			<span class="flex-1 opacity-70">Working{tools.length > 1 ? ` (${tools.length} tools)` : ''}...</span>
		{:else if hasError}
			<span class="flex-1 text-error">{summary()} (with errors)</span>
		{:else}
			<CheckCircle2 class="w-3.5 h-3.5 shrink-0 text-success/60" />
			<span class="flex-1 opacity-70">{summary()}</span>
		{/if}
	</button>

	{#if isExpanded}
		<div class="border-t border-base-300/40 py-1">
			{#each tools as tool, i (i)}
				{@const displayName = formatToolName(tool.name, tool.input)}
				{@const desc = extractDescription(tool)}
				{@const outputPreview = tool.status === 'complete' ? extractOutputPreview(tool.output) : ''}
				<button
					type="button"
					class="flex items-start gap-2 w-full px-3 py-1.5 bg-transparent text-base-content text-sm cursor-pointer text-left hover:bg-base-300/25 transition-colors"
					onclick={() => onViewToolOutput?.(tool)}
				>
					<div class="shrink-0 pt-px">
						{#if tool.status === 'running'}
							<Loader2 class="w-3.5 h-3.5 animate-spin text-primary" />
						{:else if tool.status === 'error'}
							<AlertCircle class="w-3.5 h-3.5 text-error" />
						{:else}
							<CheckCircle2 class="w-3.5 h-3.5 text-success/50" />
						{/if}
					</div>
					<div class="flex-1 min-w-0">
						<div class="flex items-baseline gap-1.5">
							<span class="font-medium whitespace-nowrap shrink-0">{displayName}</span>
							{#if desc}
								<span class="text-xs opacity-50 truncate min-w-0">{desc}</span>
							{/if}
						</div>
						{#if outputPreview}
							<div class="text-xs opacity-40 truncate mt-0.5">{outputPreview}</div>
						{/if}
					</div>
				</button>
			{/each}
		</div>
	{/if}
</div>
