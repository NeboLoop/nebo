<script lang="ts">
	import { X, Copy, Check, FileEdit, FileText, Terminal, Search, Globe, Loader2 } from 'lucide-svelte';
	import Markdown from '$lib/components/ui/Markdown.svelte';
	import { getToolOutput } from '$lib/api';

	interface ToolCall {
		id?: string;
		name: string;
		input: unknown;
		output?: string;
		status?: 'running' | 'complete' | 'error';
	}

	interface Props {
		tool: ToolCall | null;
		chatId: string | null;
		onClose: () => void;
	}

	let { tool, chatId, onClose }: Props = $props();

	let copied = $state(false);
	let loadedOutput = $state<string | null>(null);
	let loadingOutput = $state(false);
	let loadError = $state<string | null>(null);
	let lastLoadedId = $state<string | null>(null);

	// Lazy-load tool output when the sidebar opens with a new tool
	$effect(() => {
		if (!tool || !chatId) {
			loadedOutput = null;
			lastLoadedId = null;
			loadError = null;
			return;
		}

		// Already have inline output (e.g. from streaming)
		if (tool.output) {
			loadedOutput = tool.output;
			lastLoadedId = tool.id ?? null;
			return;
		}

		// Already loaded this tool
		if (tool.id && tool.id === lastLoadedId) return;

		// No id to look up
		if (!tool.id) {
			loadedOutput = null;
			lastLoadedId = null;
			return;
		}

		const toolCallId = tool.id;
		loadingOutput = true;
		loadError = null;
		loadedOutput = null;

		getToolOutput(chatId, toolCallId)
			.then((res) => {
				loadedOutput = res.output || '';
				lastLoadedId = toolCallId;
			})
			.catch((err) => {
				loadError = 'Failed to load output';
				console.error('Failed to load tool output:', err);
			})
			.finally(() => {
				loadingOutput = false;
			});
	});

	const displayOutput = $derived(loadedOutput ?? tool?.output ?? '');
	const hasOutput = $derived(displayOutput.trim().length > 0);

	async function copyOutput() {
		if (!displayOutput) return;
		try {
			await navigator.clipboard.writeText(displayOutput);
			copied = true;
			setTimeout(() => {
				copied = false;
			}, 2000);
		} catch (err) {
			console.error('Failed to copy:', err);
		}
	}

	// Get tool icon based on name
	function getToolIcon(toolName: string) {
		const lower = toolName.toLowerCase();
		if (lower.includes('write') || lower.includes('edit')) return FileEdit;
		if (lower.includes('read') || lower.includes('file')) return FileText;
		if (lower.includes('bash') || lower.includes('shell') || lower.includes('exec')) return Terminal;
		if (lower.includes('search') || lower.includes('grep') || lower.includes('glob')) return Search;
		if (lower.includes('web') || lower.includes('fetch')) return Globe;
		return FileText;
	}

	// Format tool name for display
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

	// Extract command/path from input
	function extractCommand(inputVal: unknown): string {
		const inputStr = toInputString(inputVal);
		if (!inputStr) return '';
		const parsed = toInputObject(inputVal);
		if (parsed) {
			if (typeof parsed.path === 'string') return parsed.path;
			if (typeof parsed.file_path === 'string') return parsed.file_path;
			if (typeof parsed.command === 'string') return parsed.command;
			if (typeof parsed.query === 'string') return parsed.query;
			if (typeof parsed.url === 'string') return parsed.url;
		}
		return inputStr;
	}

	const displayName = $derived(tool ? formatToolName(tool.name, tool.input) : '');
	const command = $derived(tool ? extractCommand(tool.input) : '');
</script>

{#if tool}
	<!-- Sidebar panel -->
	<div
		class="fixed inset-y-0 right-0 w-full sm:w-[480px] bg-base-100 border-l border-base-300 shadow-2xl z-50 flex flex-col animate-slide-in-right"
	>
		<!-- Header -->
		<div class="flex items-center justify-between px-5 py-4 border-b border-base-300">
			<h2 class="text-lg font-semibold text-base-content">Tool Output</h2>
			<button
				type="button"
				onclick={onClose}
				class="p-1.5 rounded-lg hover:bg-base-200 text-base-content/70 hover:text-base-content transition-colors"
				title="Close"
			>
				<X class="w-5 h-5" />
			</button>
		</div>

		<!-- Content -->
		<div class="flex-1 overflow-y-auto p-5">
			<!-- Tool name -->
			<h3 class="text-xl font-bold text-base-content mb-4">{displayName}</h3>

			<!-- Command -->
			{#if command}
				<div class="mb-4">
					<div class="text-sm font-medium text-base-content/70 mb-1">Command:</div>
					<div class="text-sm text-base-content font-mono break-all">{command}</div>
				</div>
			{/if}

			<!-- Output -->
			{#if loadingOutput}
				<div class="flex items-center gap-2 text-base-content/70">
					<Loader2 class="w-4 h-4 animate-spin" />
					<span class="text-sm">Loading output...</span>
				</div>
			{:else if loadError}
				<p class="text-error text-sm italic">{loadError}</p>
			{:else if hasOutput}
				<div class="relative">
					<button
						type="button"
						onclick={copyOutput}
						class="absolute top-2 right-2 p-1.5 rounded-md bg-base-200 hover:bg-base-300 text-base-content/70 hover:text-base-content transition-colors"
						title="Copy output"
					>
						{#if copied}
							<Check class="w-4 h-4 text-success" />
						{:else}
							<Copy class="w-4 h-4" />
						{/if}
					</button>
					<div class="bg-base-200/50 rounded-lg p-4 pr-12">
						<pre class="text-sm font-mono text-base-content/80 whitespace-pre-wrap break-all">{displayOutput}</pre>
					</div>
				</div>
			{:else}
				<p class="text-base-content/70 italic">No output — tool completed successfully.</p>
			{/if}
		</div>
	</div>

	<!-- Backdrop for mobile -->
	<button
		type="button"
		class="fixed inset-0 bg-black/50 z-40 sm:bg-black/20"
		onclick={onClose}
		aria-label="Close sidebar"
	></button>
{/if}
