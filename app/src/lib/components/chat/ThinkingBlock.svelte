<script lang="ts">
	import { ChevronDown, ChevronRight, Brain, Loader2 } from 'lucide-svelte';

	interface Props {
		content: string;
		initiallyCollapsed?: boolean;
		isStreaming?: boolean;
	}

	let { content, initiallyCollapsed = false, isStreaming = false }: Props = $props();

	// Initialize collapse state from prop (intentionally not reactive - user controls after mount)
	let isCollapsed = $state(initiallyCollapsed);

	function toggle() {
		isCollapsed = !isCollapsed;
	}
</script>

<div class="thinking-block border border-dashed rounded-lg p-3 {isStreaming ? 'border-primary/40 bg-primary/5' : 'border-base-content/20 bg-base-200/30'}">
	<button
		type="button"
		onclick={toggle}
		class="flex items-center gap-2 w-full text-left text-sm text-base-content/60 hover:text-base-content/80 transition-colors"
	>
		{#if isCollapsed}
			<ChevronRight class="w-4 h-4" />
		{:else}
			<ChevronDown class="w-4 h-4" />
		{/if}
		{#if isStreaming}
			<Loader2 class="w-4 h-4 animate-spin text-primary" />
		{:else}
			<Brain class="w-4 h-4" />
		{/if}
		<span class="font-medium italic">{isStreaming ? 'Thinking...' : 'Reasoning'}</span>
	</button>

	{#if !isCollapsed}
		<div class="mt-2 pl-6 text-sm text-base-content/70 italic whitespace-pre-wrap">
			{content}{#if isStreaming}<span class="inline-block w-0.5 h-3 bg-primary/60 animate-pulse ml-0.5 align-text-bottom rounded-full"></span>{/if}
		</div>
	{/if}
</div>
