<script lang="ts">
	import { X, Copy, Check, FileEdit, FileText, Terminal, Search, Globe } from 'lucide-svelte';
	import Markdown from '$lib/components/ui/Markdown.svelte';

	interface ToolCall {
		name: string;
		input: string;
		output?: string;
		status?: 'running' | 'complete' | 'error';
	}

	interface Props {
		tool: ToolCall | null;
		onClose: () => void;
	}

	let { tool, onClose }: Props = $props();

	let copied = $state(false);

	async function copyOutput() {
		if (!tool?.output) return;
		try {
			await navigator.clipboard.writeText(tool.output);
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

	// Extract command/path from input
	function extractCommand(inputStr: string): string {
		if (!inputStr) return '';
		try {
			const parsed = JSON.parse(inputStr);
			if (parsed.path) return parsed.path;
			if (parsed.file_path) return parsed.file_path;
			if (parsed.command) return parsed.command;
			if (parsed.query) return parsed.query;
			if (parsed.url) return parsed.url;
		} catch {
			// Not JSON
		}
		return inputStr;
	}

	const displayName = $derived(tool ? formatToolName(tool.name) : '');
	const command = $derived(tool ? extractCommand(tool.input) : '');
	const hasOutput = $derived(tool?.output && tool.output.trim().length > 0);
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
				class="p-1.5 rounded-lg hover:bg-base-200 text-base-content/60 hover:text-base-content transition-colors"
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
					<div class="text-sm font-medium text-base-content/60 mb-1">Command:</div>
					<div class="text-sm text-base-content font-mono break-all">{command}</div>
				</div>
			{/if}

			<!-- Output -->
			{#if hasOutput}
				<div class="relative">
					<button
						type="button"
						onclick={copyOutput}
						class="absolute top-2 right-2 p-1.5 rounded-md bg-base-200 hover:bg-base-300 text-base-content/60 hover:text-base-content transition-colors"
						title="Copy output"
					>
						{#if copied}
							<Check class="w-4 h-4 text-success" />
						{:else}
							<Copy class="w-4 h-4" />
						{/if}
					</button>
					<div class="bg-base-200/50 rounded-lg p-4 pr-12">
						<pre class="text-sm font-mono text-base-content/80 whitespace-pre-wrap break-all">{tool.output}</pre>
					</div>
				</div>
			{:else}
				<p class="text-base-content/60 italic">No output — tool completed successfully.</p>
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
