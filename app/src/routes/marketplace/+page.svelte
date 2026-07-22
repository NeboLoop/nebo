<script lang="ts">
	import { page } from '$app/stores';
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import Search from 'lucide-svelte/icons/search';
	import FeaturedCard from '$lib/components/marketplace/FeaturedCard.svelte';
	import SectionTopRanked from '$lib/components/marketplace/sections/SectionTopRanked.svelte';
	import SectionFeaturedPair from '$lib/components/marketplace/sections/SectionFeaturedPair.svelte';
	import SectionListGrid from '$lib/components/marketplace/sections/SectionListGrid.svelte';
	import MarketplaceGrid from '$lib/components/MarketplaceGrid.svelte';
	import ListCard from '$lib/components/marketplace/ListCard.svelte';
	import { listStoreProducts, listStoreFeatured, listStoreCategories } from '$lib/api/nebo';
	import { type AppItem, toAppItem } from '$lib/types/marketplace';
	import ResumeCard from '$lib/components/marketplace/ResumeCard.svelte';
	import { loadMarketplaceMap, type MarketplaceMap } from '$lib/data/marketplaceMap';
	import { slugify, categoryMeta } from '$lib/data/categories';

	const KIND_TYPE: Record<string, string> = {
		all: '', agents: 'agent', apps: 'app', skills: 'skill',
		plugins: 'plugin', connectors: 'connector', collections: 'collection'
	};
	const KIND_LABEL_KEY: Record<string, string> = {
		all: 'marketplace.title', agents: 'marketplace.nav.agents', apps: 'marketplace.nav.apps', skills: 'marketplace.skills',
		plugins: 'marketplace.nav.plugins', connectors: 'nav.connectors', collections: 'marketplace.nav.collections'
	};

	const kind = $derived($page.url.searchParams.get('kind') || 'all');
	const price = $derived($page.url.searchParams.get('price') || 'all');
	const category = $derived($page.url.searchParams.get('category') || '');
	const publisher = $derived($page.url.searchParams.get('publisher') || '');
	const kindType = $derived(KIND_TYPE[kind] ?? '');
	const isFiltering = $derived(kind !== 'all' || price !== 'all' || category !== '' || publisher !== '');

	let loading = $state(true);
	let items: AppItem[] = $state([]);
	// Curated Employees/Tools presentation map (same single source as the website).
	let mktMap: MarketplaceMap | null = $state(null);
	let featured: AppItem[] = $state([]);
	let categoryOrder: string[] = $state([]);

	// The proxy caps at 100/page. Fetch page 1, read `total`, then fetch only the
	// remaining pages that actually have data (in parallel) — no fixed page count
	// to over-fetch empty pages or under-fetch as the catalog grows.
	const PAGE_SIZE = 100;
	async function fetchAllProducts(): Promise<AppItem[]> {
		const first = (await listStoreProducts(undefined, undefined, 1, PAGE_SIZE).catch(
			() => ({ products: [], total: 0 })
		)) as { products?: any[]; total?: number };
		const total = Number(first?.total ?? (first?.products?.length ?? 0));
		const pages = Math.max(1, Math.ceil(total / PAGE_SIZE));
		const rest = await Promise.all(
			Array.from({ length: pages - 1 }, (_, i) =>
				listStoreProducts(undefined, undefined, i + 2, PAGE_SIZE).catch(() => ({ products: [] }))
			)
		);
		const seen = new Set<string>();
		const out: AppItem[] = [];
		for (const res of [first, ...rest]) {
			for (const r of ((res as { products?: any[] })?.products as any[]) || []) {
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
			const [products, featuredRes, catsRes, mapRes] = await Promise.all([
				fetchAllProducts(),
				listStoreFeatured().catch(() => ({ products: [] })),
				listStoreCategories().catch(() => ({ categories: [] })),
				loadMarketplaceMap()
			]);
			mktMap = mapRes;
			items = products;
			featured = (((featuredRes as { products?: any[] })?.products as any[]) || []).map((r, i) => toAppItem(r, i));
			const cats = ((catsRes as { categories?: any[] })?.categories as any[]) || [];
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

	// ── Employees / Tools views (map-driven, joined on artifact Code) ──
	const mapOf = (it: AppItem) => mktMap?.entries[it.code];
	const respOf = (it: AppItem) => mktMap?.responsibilities[mapOf(it)?.role ?? ''] ?? [];
	const employees = $derived(mktMap ? items.filter((it) => mapOf(it)?.d === 'E') : []);
	const employeesByDept = $derived.by(() => {
		if (!mktMap) return [] as { name: string; roles: AppItem[] }[];
		return mktMap.departments
			.map((d) => ({ name: d, roles: employees.filter((e) => mapOf(e)?.dept === d) }))
			.filter((g) => g.roles.length > 0);
	});
	const toolItems = $derived(mktMap ? items.filter((it) => mapOf(it)?.d === 'T') : []);
	const toolsByCategory = $derived.by(() => {
		if (!mktMap) return [] as { name: string; items: AppItem[] }[];
		return mktMap.toolCategories
			.map((c) => ({ name: c, items: toolItems.filter((t) => mapOf(t)?.tc === c) }))
			.filter((g) => g.items.length > 0);
	});

	// A category on its own (no kind/price/publisher) gets the editorial
	// storefront treatment: headline + lede + featured + Top + All.
	const isCategoryView = $derived(!!category && kind === 'all' && price === 'all' && !publisher);
	const categoryItems = $derived(category ? items.filter((it) => slugify(it.category) === category) : []);
	const categoryName = $derived(categoryItems[0]?.category ?? '');
	const catMeta = $derived(categoryName ? categoryMeta[categoryName] : undefined);
	const categoryFeatured = $derived(
		featured.find((f) => slugify(f.category) === category) ?? categoryItems[0] ?? null
	);

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
{:else if isCategoryView}
	<!-- Category storefront — headline + lede + featured + Top + All -->
	<div class="max-w-6xl mx-auto pb-10">
		<div class="px-6 pt-6">
			<h1 class="font-display text-3xl font-bold tracking-tight">{categoryName || $t('marketplace.detail.category')}</h1>
			{#if catMeta}
				<p class="text-base text-base-content/70 mt-2 max-w-3xl leading-relaxed">{catMeta.lede}</p>
			{/if}
		</div>

		{#if categoryItems.length === 0}
			<div class="flex flex-col items-center justify-center py-16 text-center">
				<Search class="w-10 h-10 text-base-content/40 mb-3" />
				<p class="text-base font-medium">{$t('marketplace.nothingHereYet')}</p>
			</div>
		{:else}
			{#if categoryFeatured}
				<div class="px-6 pt-6">
					<FeaturedCard item={categoryFeatured} />
				</div>
			{/if}
			<SectionListGrid title={$t('marketplace.topIn', { values: { name: categoryName } })} items={categoryItems.slice(0, 6)} />
			{#if categoryItems.length > 6}
				<SectionListGrid title={$t('marketplace.allIn', { values: { name: categoryName } })} items={categoryItems.slice(6)} />
			{/if}
		{/if}
	</div>
{:else if kind === 'employees'}
	<!-- Employees — the website's hiring view: departments of roles as resume cards -->
	<div class="max-w-6xl mx-auto px-6 py-8 pb-12">
		<h1 class="font-display text-3xl font-bold tracking-tight">{$t('marketplace.employeesHeadline')}</h1>
		<p class="text-base text-base-content/70 mt-2 max-w-3xl leading-relaxed">{$t('marketplace.employeesLede')}</p>
		{#if !mktMap || employees.length === 0}
			<div class="flex flex-col items-center justify-center py-16 text-center">
				<Search class="w-10 h-10 text-base-content/40 mb-3" />
				<p class="text-base font-medium">{$t('marketplace.nothingHereYet')}</p>
			</div>
		{:else}
			{#each employeesByDept as group}
				<section class="mt-10">
					<div class="flex items-baseline gap-3 mb-4">
						<h2 class="text-xl font-bold tracking-tight">{group.name}</h2>
						<span class="text-sm text-base-content/50">{group.roles.length} {group.roles.length === 1 ? 'role' : 'roles'}</span>
					</div>
					<div class="grid grid-cols-1 md:grid-cols-2 gap-5">
						{#each group.roles as e (e.id)}
							<ResumeCard item={e} department={group.name} title={mapOf(e)?.role} responsibilities={respOf(e)} />
						{/each}
					</div>
				</section>
			{/each}
		{/if}
	</div>
{:else if kind === 'tools'}
	<!-- Tools — the website's tool-category view over the same catalog -->
	<div class="max-w-6xl mx-auto px-6 py-8 pb-12">
		<h1 class="font-display text-3xl font-bold tracking-tight">{$t('marketplace.toolsHeadline')}</h1>
		<p class="text-base text-base-content/70 mt-2 max-w-3xl leading-relaxed">{$t('marketplace.toolsLede')}</p>
		{#if !mktMap || toolItems.length === 0}
			<div class="flex flex-col items-center justify-center py-16 text-center">
				<Search class="w-10 h-10 text-base-content/40 mb-3" />
				<p class="text-base font-medium">{$t('marketplace.nothingHereYet')}</p>
			</div>
		{:else}
			{#each toolsByCategory as group}
				<section class="mt-8">
					<div class="flex items-baseline gap-3 mb-3">
						<h2 class="text-lg font-bold tracking-tight">{group.name}</h2>
						<span class="text-sm text-base-content/50">{group.items.length}</span>
					</div>
					<MarketplaceGrid>
						{#each group.items as item (item.id)}
							<ListCard {item} />
						{/each}
					</MarketplaceGrid>
				</section>
			{/each}
		{/if}
	</div>
{:else if isFiltering}
	<!-- Filtered: flat result list (kind / price / publisher) -->
	<div class="max-w-6xl mx-auto px-6 py-6">
		{#if publisher}
			<h1 class="font-display text-xl font-bold mb-1">{$t('marketplace.byPublisher', { values: { name: publisher } })}</h1>
		{/if}
		<div class="mb-4 text-sm text-base-content/70">
			{filteredItems.length === 1 ? $t('marketplace.resultCountSingular', { values: { count: filteredItems.length } }) : $t('marketplace.resultCount', { values: { count: filteredItems.length } })}
		</div>
		{#if filteredItems.length === 0}
			<div class="flex flex-col items-center justify-center py-16 text-center">
				<Search class="w-10 h-10 text-base-content/40 mb-3" />
				<p class="text-base font-medium">{$t('common.noResultsFound')}</p>
				<p class="text-xs text-base-content/50 mt-1">{$t('marketplace.tryAdjustingFilters')}</p>
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
		<p class="text-base font-medium">{$t('marketplace.noItemsYet')}</p>
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

		<SectionTopRanked title={$t('marketplace.topIn', { values: { name: KIND_LABEL_KEY[kind] ? $t(KIND_LABEL_KEY[kind]) : $t('marketplace.theMarketplace') } })} items={items.slice(0, 9)} />

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
