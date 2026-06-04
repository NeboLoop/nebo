<script lang="ts">
	import { page } from '$app/stores';
	import { onMount } from 'svelte';
	import Search from 'lucide-svelte/icons/search';
	import FeaturedCard from '$lib/components/marketplace/FeaturedCard.svelte';
	import SectionTopRanked from '$lib/components/marketplace/sections/SectionTopRanked.svelte';
	import SectionFeaturedPair from '$lib/components/marketplace/sections/SectionFeaturedPair.svelte';
	import SectionListGrid from '$lib/components/marketplace/sections/SectionListGrid.svelte';
	import MarketplaceGrid from '$lib/components/MarketplaceGrid.svelte';
	import ListCard from '$lib/components/marketplace/ListCard.svelte';
	import webapi from '$lib/api/gocliRequest';
	import { type AppItem, toAppItem } from '$lib/types/marketplace';
	import { slugify } from '$lib/data/categories';

	const KIND_TYPE: Record<string, string> = {
		all: '', agents: 'agent', apps: 'app', skills: 'skill',
		plugins: 'plugin', connectors: 'connector', collections: 'collection'
	};
	const KIND_LABEL: Record<string, string> = {
		all: 'Marketplace', agents: 'Agents', apps: 'Apps', skills: 'Skills',
		plugins: 'Plugins', connectors: 'Connectors', collections: 'Collections'
	};

	const kind = $derived($page.url.searchParams.get('kind') || 'all');
	const price = $derived($page.url.searchParams.get('price') || 'all');
	const category = $derived($page.url.searchParams.get('category') || '');
	const publisher = $derived($page.url.searchParams.get('publisher') || '');
	const kindType = $derived(KIND_TYPE[kind] ?? '');
	const isFiltering = $derived(kind !== 'all' || price !== 'all' || category !== '' || publisher !== '');

	let loading = $state(true);
	let items: AppItem[] = $state([]);
	let featured: AppItem[] = $state([]);
	let categoryOrder: string[] = $state([]);

	// The proxy pages the catalog (~100/page via limit/offset); fetch in PARALLEL and dedupe.
	async function fetchAllProducts(): Promise<AppItem[]> {
		const PAGES = 6;
		const results = await Promise.all(
			Array.from({ length: PAGES }, (_, i) =>
				webapi
					.get<any>('/api/v1/store/products', { page: i + 1, pageSize: 100 })
					.catch(() => ({ products: [] }))
			)
		);
		const seen = new Set<string>();
		const out: AppItem[] = [];
		for (const res of results) {
			for (const r of (res?.products as any[]) || []) {
				const id = String(r?.id ?? '');
				if (!id || seen.has(id)) continue;
				seen.add(id);
				out.push(toAppItem(r, out.length));
			}
		}
		return out;
	}

	onMount(async () => {
		try {
			const [products, featuredRes, catsRes] = await Promise.all([
				fetchAllProducts(),
				webapi.get<any>('/api/v1/store/featured', {}).catch(() => ({ products: [] })),
				webapi.get<any>('/api/v1/store/categories', {}).catch(() => ({ categories: [] }))
			]);
			items = products;
			featured = ((featuredRes?.products as any[]) || []).map((r, i) => toAppItem(r, i));
			const cats = (catsRes?.categories as any[]) || [];
			categoryOrder = cats.map((c) => String(c.name || ''));
		} catch { /* ignore */ }
		loading = false;
	});

	const filteredItems = $derived.by(() => {
		let result = items;
		if (kindType) result = result.filter((it) => it.type === kindType);
		if (price === 'free') result = result.filter((it) => it.free);
		else if (price === 'paid') result = result.filter((it) => !it.free);
		if (category) result = result.filter((it) => slugify(it.category) === category);
		if (publisher) result = result.filter((it) => it.author === publisher);
		return result;
	});

	const spotlight = $derived(featured[0] ?? items[0] ?? null);

	// Group all items by category, ordered by the catalog's category order.
	const byCategory = $derived.by(() => {
		const groups = new Map<string, AppItem[]>();
		for (const it of items) {
			if (!it.category) continue;
			if (!groups.has(it.category)) groups.set(it.category, []);
			groups.get(it.category)!.push(it);
		}
		const order = (name: string) => {
			const idx = categoryOrder.indexOf(name);
			return idx === -1 ? 999 : idx;
		};
		return [...groups.entries()]
			.map(([name, list]) => ({ name, items: list }))
			.filter((g) => g.items.length > 0)
			.sort((a, b) => order(a.name) - order(b.name));
	});
</script>

<svelte:head><title>Marketplace - Nebo</title></svelte:head>

{#if loading}
	<div class="flex justify-center py-16">
		<span class="loading loading-spinner loading-md text-primary"></span>
	</div>
{:else if isFiltering}
	<!-- Filtered: flat result list -->
	<div class="max-w-6xl mx-auto px-6 py-6">
		{#if publisher}
			<h1 class="font-display text-xl font-bold mb-1">By {publisher}</h1>
		{/if}
		<div class="mb-4 text-sm text-base-content/70">
			{filteredItems.length} result{filteredItems.length === 1 ? '' : 's'}
		</div>
		{#if filteredItems.length === 0}
			<div class="flex flex-col items-center justify-center py-16 text-center">
				<Search class="w-10 h-10 text-base-content/40 mb-3" />
				<p class="text-base font-medium">No results found</p>
				<p class="text-xs text-base-content/50 mt-1">Try adjusting your filters.</p>
			</div>
		{:else}
			<MarketplaceGrid>
				{#each filteredItems as item}
					<ListCard {item} />
				{/each}
			</MarketplaceGrid>
		{/if}
	</div>
{:else if items.length === 0}
	<div class="flex flex-col items-center justify-center py-16 text-center">
		<Search class="w-10 h-10 text-base-content/40 mb-3" />
		<p class="text-base font-medium">No items in the marketplace yet</p>
	</div>
{:else}
	<!-- Editorial home — Apple App Store cadence: a large hero, a ranked list,
	     then dense category lists punctuated by larger feature pairs. Every card
	     (large or small) carries its install code. -->
	<div class="max-w-6xl mx-auto pb-10">
		{#if spotlight}
			<div class="px-6 pt-6">
				<FeaturedCard item={spotlight} />
			</div>
		{/if}

		<SectionTopRanked title="Top in {KIND_LABEL[kind] ?? 'the Marketplace'}" items={items.slice(0, 9)} />

		{#each byCategory as group, i}
			{#if i % 4 === 0}
				<SectionFeaturedPair
					title={group.name}
					items={group.items.slice(0, 2)}
					seeAllHref="/marketplace?category={slugify(group.name)}"
				/>
			{:else}
				<SectionListGrid
					title={group.name}
					items={group.items.slice(0, 6)}
					seeAllHref="/marketplace?category={slugify(group.name)}"
				/>
			{/if}
		{/each}
	</div>
{/if}
