<script lang="ts">
	import LargeCard from './LargeCard.svelte';
	import { type AppItem } from '$lib/types/marketplace';

	let { items }: { items: AppItem[] } = $props();
</script>

{#if items.length > 0}
	<div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
		{#each items as item}
			<LargeCard {item} />
		{/each}
	</div>
{/if}
