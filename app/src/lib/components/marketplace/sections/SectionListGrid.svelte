<script lang="ts">
	import MarketplaceGrid from '$lib/components/MarketplaceGrid.svelte';
	import ListCard from '../ListCard.svelte';
	import { type AppItem } from '$lib/types/marketplace';

	let { title, seeAllHref, items }: { title: string; seeAllHref?: string; items: AppItem[] } = $props();
</script>

{#if items.length > 0}
	<div class="px-6 py-6">
		<div class="flex items-baseline justify-between mb-3">
			<h2 class="font-display text-lg font-bold">{title}</h2>
			{#if seeAllHref}
				<a href={seeAllHref} class="text-sm text-primary font-medium">See All</a>
			{/if}
		</div>
		<MarketplaceGrid>
			{#each items as item}
				<ListCard {item} />
			{/each}
		</MarketplaceGrid>
	</div>
{/if}
