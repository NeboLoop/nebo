<script lang="ts">
	import MarketplaceGrid from '$lib/components/MarketplaceGrid.svelte';
	import ListCard from '../ListCard.svelte';
	import { type AppItem } from '$lib/types/marketplace';

	let { title, seeAllHref, items }: { title: string; seeAllHref?: string; items: AppItem[] } = $props();

	const ranked = $derived(items.slice(0, 21));
</script>

{#if ranked.length > 0}
	<div class="px-6 py-6">
		<div class="flex items-baseline justify-between mb-3">
			<h2 class="font-display text-lg font-bold">{title}</h2>
			{#if seeAllHref}
				<a href={seeAllHref} class="text-base text-primary font-medium">See All</a>
			{/if}
		</div>
		<MarketplaceGrid>
			{#each ranked as item, i}
				<ListCard {item} rank={i + 1} />
			{/each}
		</MarketplaceGrid>
	</div>
{/if}
