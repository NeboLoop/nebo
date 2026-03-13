<script lang="ts">
	import MarketplaceGrid from '$lib/components/MarketplaceGrid.svelte';
	import ArtifactIcon from '../ArtifactIcon.svelte';
	import PricePill from '../PricePill.svelte';
	import { type AppItem, itemHref } from '$lib/types/marketplace';

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
				<a href={itemHref(item)} class="flex flex-col gap-3 p-4 rounded-2xl hover:bg-base-content/[0.03] transition-colors">
					<ArtifactIcon emoji={item.iconEmoji} bg={item.iconBg} size="xl" />
					<div class="min-w-0">
						<p class="text-sm font-bold truncate">{item.name}</p>
						<p class="text-xs text-base-content/70 line-clamp-2 leading-relaxed mt-1">{item.description}</p>
					</div>
					<PricePill price={item.price} installed={item.installed} />
				</a>
			{/each}
		</MarketplaceGrid>
	</div>
{/if}
