<script lang="ts">
	import ToolCard from './ToolCard.svelte';

	interface ToolCall {
		name: string;
		input: string;
		output?: string;
		status?: 'running' | 'complete' | 'error';
	}

	interface Props {
		tools: ToolCall[];
		selectedIndex?: number | null;
		onSelect?: (index: number) => void;
	}

	let { tools, selectedIndex = null, onSelect }: Props = $props();
</script>

<div class="rounded-xl border border-base-300 overflow-hidden divide-y divide-base-300">
	{#each tools as tool, index (index)}
		<ToolCard
			name={tool.name}
			input={tool.input}
			output={tool.output}
			status={tool.status}
			selected={selectedIndex === index}
			onclick={() => onSelect?.(index)}
		/>
	{/each}
</div>
