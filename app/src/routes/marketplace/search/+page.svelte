<script lang="ts">
	import { page } from '$app/stores';
	import Search from 'lucide-svelte/icons/search';
	import MarketplaceGrid from '$lib/components/MarketplaceGrid.svelte';
	import ListCard from '$lib/components/marketplace/ListCard.svelte';
	import webapi from '$lib/api/gocliRequest';
	import { type AppItem, toAppItem } from '$lib/types/marketplace';

	const query = $derived(($page.url.searchParams.get('q') ?? '').trim());

	let loading = $state(false);
	let results: AppItem[] = $state([]);
	let lastQuery = $state('');

	// Backend search — scales to thousands of artifacts (NeboLoop Search), unlike
	// the inline dropdown that only ever saw the first loaded page.
	$effect(() => {
		const q = query;
		if (!q) {
			results = [];
			loading = false;
			return;
		}
		loading = true;
		lastQuery = q;
		webapi
			.get<any>('/api/v1/store/products', { q, pageSize: 60 })
			.then((res: any) => {
				if (lastQuery !== q) return; // a newer query superseded this one
				results = ((res?.products as any[]) || []).map((r, i) => toAppItem(r, i));
			})
			.catch(() => {
				if (lastQuery === q) results = [];
			})
			.finally(() => {
				if (lastQuery === q) loading = false;
			});
	});
</script>

<svelte:head><title>Search — Marketplace</title></svelte:head>

<div class="max-w-6xl mx-auto px-6 py-6">
	{#if !query}
		<div class="flex flex-col items-center justify-center py-16 text-center">
			<Search class="w-10 h-10 text-base-content/40 mb-3" />
			<p class="text-base font-medium">Search the marketplace</p>
			<p class="text-xs text-base-content/50 mt-1">Find agents, apps, skills, plugins and connectors by name or what they do.</p>
		</div>
	{:else}
		<div class="mb-4">
			<h1 class="font-display text-xl font-bold">Results for "{query}"</h1>
			{#if !loading}
				<p class="text-sm text-base-content/70 mt-0.5">{results.length} result{results.length === 1 ? '' : 's'}</p>
			{/if}
		</div>

		{#if loading}
			<div class="flex justify-center py-16">
				<span class="loading loading-spinner loading-md text-primary"></span>
			</div>
		{:else if results.length === 0}
			<div class="flex flex-col items-center justify-center py-16 text-center">
				<Search class="w-10 h-10 text-base-content/40 mb-3" />
				<p class="text-base font-medium">No results found</p>
				<p class="text-xs text-base-content/50 mt-1">Try a different search term.</p>
			</div>
		{:else}
			<MarketplaceGrid>
				{#each results as item}
					<ListCard {item} />
				{/each}
			</MarketplaceGrid>
		{/if}
	{/if}
</div>
