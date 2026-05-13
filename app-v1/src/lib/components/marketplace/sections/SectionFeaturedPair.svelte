<script lang="ts">
	import LargeCard from '../LargeCard.svelte';
	import { type AppItem } from '$lib/types/marketplace';

	let { title, items, label }: { title?: string; items: AppItem[]; label?: string } = $props();
</script>

{#if items.length > 0}
	<div class="px-6 pt-8 pb-2">
		{#if title}
			<h2 class="font-display text-lg font-bold mb-4">{title}</h2>
		{/if}
		<div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
			{#each items.slice(0, 2) as item}
				<LargeCard {item} {label} />
			{/each}
		</div>
	</div>
{/if}
