<script lang="ts">
	import LargeCard from '../LargeCard.svelte';
	import { type AppItem } from '$lib/types/marketplace';

	let { title, items, label, seeAllHref }: { title?: string; items: AppItem[]; label?: string; seeAllHref?: string } = $props();
</script>

{#if items.length > 0}
	<div class="px-6 py-6">
		{#if title || seeAllHref}
			<div class="flex items-baseline justify-between mb-4">
				{#if title}<h2 class="font-display text-lg font-bold">{title}</h2>{/if}
				{#if seeAllHref}<a href={seeAllHref} class="text-sm text-primary font-medium">See All</a>{/if}
			</div>
		{/if}
		<div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
			{#each items.slice(0, 2) as item}
				<LargeCard {item} {label} />
			{/each}
		</div>
	</div>
{/if}
