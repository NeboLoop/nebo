<script lang="ts">
	import { onMount } from 'svelte';
	import { ChevronDown, ChevronRight, Brain, Loader2 } from 'lucide-svelte';

	interface Props {
		content: string;
		initiallyCollapsed?: boolean;
		isStreaming?: boolean;
	}

	let { content, initiallyCollapsed = false, isStreaming = false }: Props = $props();

	let isCollapsed = $state(initiallyCollapsed);
	let startTime = $state(Date.now());
	let elapsed = $state(0);
	let intervalId: ReturnType<typeof setInterval> | undefined;

	onMount(() => {
		startTime = Date.now();
		intervalId = setInterval(() => {
			elapsed = (Date.now() - startTime) / 1000;
		}, 100);
		return () => { if (intervalId) clearInterval(intervalId); };
	});

	// Stop timer when streaming ends
	$effect(() => {
		if (!isStreaming && intervalId) {
			clearInterval(intervalId);
			intervalId = undefined;
		}
	});

	const durationLabel = $derived(
		elapsed < 1 ? '' : elapsed < 10 ? `${elapsed.toFixed(1)}s` : `${Math.round(elapsed)}s`
	);

	function toggle() {
		isCollapsed = !isCollapsed;
	}
</script>

<div class="border border-dashed rounded-lg p-3 {isStreaming ? 'border-primary/40 bg-primary/5' : 'border-base-300 bg-base-200'}">
	<button
		type="button"
		onclick={toggle}
		class="flex items-center gap-2 w-full text-left text-sm text-base-content/60 hover:text-base-content transition-colors"
	>
		{#if isCollapsed}
			<ChevronRight class="w-3.5 h-3.5" />
		{:else}
			<ChevronDown class="w-3.5 h-3.5" />
		{/if}
		{#if isStreaming}
			<Loader2 class="w-3.5 h-3.5 animate-spin text-primary" />
		{:else}
			<Brain class="w-3.5 h-3.5" />
		{/if}
		<span class="font-medium italic">{isStreaming ? 'Thinking' : 'Reasoning'}</span>
		{#if durationLabel}
			<span class="text-base-content/40 font-normal not-italic"> · {durationLabel}</span>
		{/if}
	</button>

	{#if !isCollapsed}
		<div class="mt-2 pl-6 text-sm text-base-content/60 italic whitespace-pre-wrap leading-relaxed">
			{content}{#if isStreaming}<span class="inline-block w-0.5 h-[1em] bg-base-content animate-blink ml-0.5 align-text-bottom"></span>{/if}
		</div>
	{/if}
</div>
