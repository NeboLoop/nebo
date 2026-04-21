<script lang="ts">
	import { FileEdit, FileText, Terminal, Search, Globe, Check, Loader2, Zap, ChevronDown, ChevronRight, X } from 'lucide-svelte';

	interface Props {
		name: string;
		input?: unknown;
		output?: string;
		status?: 'running' | 'complete' | 'error';
		selected?: boolean;
		onclick?: () => void;
	}

	let { name, input = '', output = '', status = 'running', selected = false, onclick }: Props = $props();

	let expanded = $state(false);

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
		if (toolName.startsWith('mcp__')) {
			const parts = toolName.split('__');
			toolName = parts[parts.length - 1];
		}
		let domain = toolName;
		const domainMatch = toolName.match(/^(\w+)\(/);
		if (domainMatch) {
			domain = domainMatch[1];
		}
		const parsed = toInputObject(inputVal);
		if (parsed) {
			const action = parsed.action;
			const resource = parsed.resource;
			const server = parsed.server;
			if (typeof action === 'string' && action.length > 0) {
				if (domain === 'mcp' && typeof server === 'string' && typeof resource === 'string') {
					return `${server}→${resource}:${action}`;
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

	function extractPath(inputVal: unknown): string {
		const inputStr = toInputString(inputVal);
		if (!inputStr) return '';
		const parsed = toInputObject(inputVal);
		if (parsed) {
			const action = typeof parsed.action === 'string' ? parsed.action : '';
			if (action && name === 'skill') {
				if (typeof parsed.name === 'string' && parsed.name.length > 0) return `${action}: ${parsed.name}`;
				return action;
			}
			if (typeof parsed.command === 'string') return truncate(parsed.command, 80);
			if (typeof parsed.path === 'string') return parsed.path.replace(/^\/Users\/\w+/, '~');
			if (typeof parsed.file_path === 'string') return parsed.file_path.replace(/^\/Users\/\w+/, '~');
			if (typeof parsed.query === 'string') return truncate(parsed.query, 80);
			if (typeof parsed.url === 'string') return truncate(parsed.url, 80);
			if (typeof parsed.pattern === 'string') return truncate(parsed.pattern, 80);
			if (typeof parsed.regex === 'string') return truncate(parsed.regex, 80);
			if (typeof parsed.subject === 'string') return truncate(parsed.subject, 80);
			if (typeof parsed.name === 'string') return truncate(parsed.name, 80);
			if (typeof parsed.key === 'string') return truncate(parsed.key, 80);
			if (typeof parsed.ref === 'string') return parsed.ref;
			if (typeof parsed.selector === 'string') return truncate(parsed.selector, 80);
			if (typeof parsed.to === 'string') return truncate(parsed.to, 80);
			if (typeof parsed.topic === 'string') return truncate(parsed.topic, 80);
			if (typeof parsed.task_id === 'string') return parsed.task_id;
			if (typeof parsed.agent_id === 'string') return parsed.agent_id;
			if (typeof parsed.session_id === 'string') return parsed.session_id;
			if (typeof parsed.channel_id === 'string') return parsed.channel_id;
			if (typeof parsed.id === 'string') return truncate(parsed.id, 80);
			if (typeof parsed.text === 'string') return truncate(parsed.text, 60);
			if (typeof parsed.prompt === 'string') return truncate(parsed.prompt, 60);
			if (typeof parsed.message === 'string') return truncate(parsed.message, 60);
			if (typeof parsed.description === 'string') return truncate(parsed.description, 60);
			if (typeof parsed.status === 'string') return parsed.status;
			if (typeof parsed.image === 'string') return truncate(parsed.image, 80);
			for (const key of Object.keys(parsed)) {
				if (key === 'action' || key === 'resource') continue;
				const val = parsed[key];
				if (typeof val === 'string' && val.length > 0 && val.length < 200) {
					return truncate(val, 80);
				}
			}
		}
		const pathMatch = inputStr.match(/path['\":\s]+([^\s'\"]+)/i);
		if (pathMatch) return pathMatch[1].replace(/^\/Users\/\w+/, '~');
		if (inputStr.length > 0) {
			return truncate(inputStr, 80);
		}
		return '';
	}

	function truncate(str: string, maxLen: number): string {
		if (str.length <= maxLen) return str;
		return str.slice(0, maxLen) + '...';
	}

	function formatJson(val: unknown): string {
		if (!val) return '';
		if (typeof val === 'string') {
			try { return JSON.stringify(JSON.parse(val), null, 2); } catch { return val; }
		}
		try { return JSON.stringify(val, null, 2); } catch { return String(val); }
	}

	const ToolIcon = $derived(getToolIcon(name));
	const displayName = $derived(formatToolName(name, input));
	const path = $derived(extractPath(input));

	const statusBadgeClass = $derived(
		status === 'running' ? 'bg-warning/10 text-warning'
		: status === 'complete' ? 'bg-success/10 text-success'
		: 'bg-error/10 text-error'
	);
	const statusLabel = $derived(
		status === 'running' ? 'Running' : status === 'complete' ? 'Done' : 'Error'
	);
</script>

<div class="rounded-lg border border-base-300 bg-base-100 transition-all duration-150 {selected ? 'border-primary bg-primary/5' : 'hover:border-base-300'}">
	<!-- Header row -->
	<button
		type="button"
		class="w-full text-left px-3 py-2.5 flex items-center gap-2.5"
		onclick={(e) => { if (status !== 'running') { e.stopPropagation(); expanded = !expanded; } else { onclick?.(); } }}
	>
		<div class="shrink-0 text-base-content/60">
			{#if status === 'running'}
				<Loader2 class="w-3.5 h-3.5 animate-spin text-warning" />
			{:else}
				<svelte:component this={ToolIcon} class="w-3.5 h-3.5" />
			{/if}
		</div>

		<div class="flex-1 min-w-0">
			<span class="text-sm font-medium font-mono text-base-content">{displayName}</span>
			{#if path}
				<span class="text-xs text-base-content/40 ml-1.5 truncate">{path}</span>
			{/if}
		</div>

		<span class="px-2 py-0.5 rounded-full text-[10px] font-semibold uppercase tracking-wider {statusBadgeClass}">
			{statusLabel}
		</span>

		{#if status !== 'running'}
			<div class="shrink-0 text-base-content/40">
				{#if expanded}
					<ChevronDown class="w-3.5 h-3.5" />
				{:else}
					<ChevronRight class="w-3.5 h-3.5" />
				{/if}
			</div>
		{/if}
	</button>

	<!-- Expandable inputs/outputs -->
	{#if expanded && status !== 'running'}
		<div class="border-t border-base-300 px-3 py-2.5 space-y-2">
			{#if input}
				<div>
					<div class="text-[10px] font-semibold uppercase tracking-wider text-base-content/40 mb-1">Input</div>
					<pre class="text-xs font-mono text-base-content/60 bg-base-200 rounded-md p-2 overflow-x-auto max-h-40 whitespace-pre-wrap break-all">{formatJson(input)}</pre>
				</div>
			{/if}
			{#if output}
				<div>
					<div class="text-[10px] font-semibold uppercase tracking-wider text-base-content/40 mb-1">Output</div>
					<pre class="text-xs font-mono text-base-content/60 bg-base-200 rounded-md p-2 overflow-x-auto max-h-40 whitespace-pre-wrap break-all">{truncate(typeof output === 'string' ? output : JSON.stringify(output), 2000)}</pre>
				</div>
			{/if}
		</div>
	{/if}
</div>
