<script lang="ts">
	import { onMount } from 'svelte';
	import Share2 from 'lucide-svelte/icons/share-2';
	import MarketplaceGrid from '$lib/components/MarketplaceGrid.svelte';
	import ListCard from '$lib/components/marketplace/ListCard.svelte';
	import { listStoreOrgs } from '$lib/api/index';
	import { type AppItem, toAppItem } from '$lib/types/marketplace';

	let loading = $state(true);
	let items: AppItem[] = $state([]);

	onMount(async () => {
		try {
			// Items shared privately with this user (org / invite / loop visibility),
			// not the public catalog. Served via the orgs endpoint.
			const res = (await listStoreOrgs().catch(() => ({ orgs: [] }))) as {
				orgs?: { items?: Record<string, unknown>[] }[];
			} | null;
			const shared = (res?.orgs ?? []).flatMap((o) => o.items ?? []);
			items = shared.map((r, i) => toAppItem(r, i));
		} catch {
			/* ignore */
		}
		loading = false;
	});
</script>

<svelte:head><title>Shared - Marketplace - Nebo</title></svelte:head>

<div class="max-w-6xl mx-auto px-6 py-6">
	<div class="mb-5">
		<div class="text-base font-semibold">Shared with you</div>
		<div class="text-xs text-base-content/50">Private agents, skills, and tools shared with you by your organizations.</div>
	</div>

	{#if loading}
		<div class="flex justify-center py-16">
			<span class="loading loading-spinner loading-md text-primary"></span>
		</div>
	{:else if items.length === 0}
		<div class="flex flex-col items-center justify-center py-16 text-center">
			<Share2 class="w-10 h-10 text-base-content/40 mb-3" />
			<p class="text-base font-medium">Nothing shared with you yet</p>
			<p class="text-xs text-base-content/50 mt-1">Items shared privately by an organization will appear here.</p>
		</div>
	{:else}
		<MarketplaceGrid>
			{#each items as item}
				<ListCard {item} />
			{/each}
		</MarketplaceGrid>
	{/if}
</div>
