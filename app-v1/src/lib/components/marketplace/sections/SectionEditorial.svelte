<script lang="ts">
	import MediumCard from '../MediumCard.svelte';
	import { type AppItem } from '$lib/types/marketplace';

	let { title, items }: { title?: string; items: AppItem[] } = $props();
</script>

{#if items.length > 0}
	<div class="px-6 pt-6 pb-2">
		{#if title}
			<h2 class="font-display text-lg font-bold mb-4">{title}</h2>
		{/if}
		<div class="flex gap-3 overflow-x-auto scrollbar-hide pb-2">
			{#each items as item}
				<MediumCard {item} />
			{/each}
		</div>
	</div>
{/if}
