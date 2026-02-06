<script lang="ts">
	import { FileEdit, FileText, Terminal, Check } from 'lucide-svelte';

	interface ToolCall {
		name: string;
		input: string;
		output?: string;
		status?: 'running' | 'complete' | 'error';
	}

	interface Props {
		tool: ToolCall;
		summary?: string;
		onView?: () => void;
	}

	let { tool, summary = '', onView }: Props = $props();

	// Get tool icon based on name
	function getToolIcon(toolName: string) {
		const lower = toolName.toLowerCase();
		if (lower.includes('write') || lower.includes('edit')) return FileEdit;
		if (lower.includes('read') || lower.includes('file')) return FileText;
		if (lower.includes('bash') || lower.includes('shell') || lower.includes('exec')) return Terminal;
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

	// Generate summary from output if not provided
	function generateSummary(toolName: string, output: string): string {
		if (!output) return '';
		const name = formatToolName(toolName).toLowerCase();

		// Check for byte counts
		const byteMatch = output.match(/(\d+)\s*bytes?\s+(?:to|written|saved)/i);
		if (byteMatch) {
			const pathMatch = output.match(/to\s+([^\s]+)/i);
			return `Successfully wrote ${byteMatch[1]} bytes${pathMatch ? ` to ${pathMatch[1]}` : ''}`;
		}

		// For read operations, show line count
		const lines = output.split('\n').length;
		if (name === 'read' && lines > 0) {
			return `Read ${lines} lines`;
		}

		// Truncate long output
		if (output.length > 60) {
			return output.slice(0, 60) + '...';
		}
		return output;
	}

	const ToolIcon = $derived(getToolIcon(tool.name));
	const displayName = $derived(formatToolName(tool.name));
	const displaySummary = $derived(summary || generateSummary(tool.name, tool.output || ''));
</script>

<div class="rounded-xl border border-base-300 bg-base-200/30 p-4">
	<!-- Summary text -->
	{#if displaySummary}
		<p class="text-sm text-base-content mb-3">{displaySummary}</p>
	{/if}

	<!-- Tool card inline -->
	<div class="flex items-center gap-3 px-3 py-2 rounded-lg bg-base-200/50 border border-base-300">
		<div class="text-base-content/60">
			<svelte:component this={ToolIcon} class="w-4 h-4" />
		</div>
		<span class="font-medium text-sm text-base-content">{displayName}</span>
		<div class="flex-1"></div>
		{#if onView}
			<button
				type="button"
				onclick={onView}
				class="flex items-center gap-1 text-sm text-primary hover:text-primary/80 transition-colors"
			>
				View
				<Check class="w-4 h-4" />
			</button>
		{/if}
	</div>

	<!-- Short output preview -->
	{#if tool.output && tool.output.length < 200}
		<div class="mt-2 px-3 py-2 rounded-lg bg-base-300/30">
			<pre class="text-xs font-mono text-base-content/70 whitespace-pre-wrap">{tool.output}</pre>
		</div>
	{/if}
</div>
