<script lang="ts">
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import { ChevronLeft, Check, Sparkles } from 'lucide-svelte';
	import webapi from '$lib/api/gocliRequest';
	import { type AppItem, toAppItem, itemHref } from '$lib/types/marketplace';
	import { slugify } from '$lib/data/categories';

	let loading = $state(true);
	let skills: AppItem[] = $state([]);
	let workflows: AppItem[] = $state([]);
	let agents: AppItem[] = $state([]);
	let catMeta: { name: string; slug: string; emoji: string } | null = $state(null);

	const slug = $derived($page.params.slug);
	const cat = $derived(catMeta);

	onMount(async () => {
		// Fetch category metadata from API
		try {
			const res = await webapi.get<any>('/api/v1/store/categories');
			const apiCats: any[] = res.categories || [];
			const match = apiCats.find((c: any) => c.slug === slug || slugify(c.name) === slug);
			if (match) {
				catMeta = {
					name: match.name,
					slug: match.slug || slugify(match.name),
					emoji: match.emoji || '📦'
				};
			}
		} catch { /* ignore */ }

		if (!catMeta) {
			loading = false;
			return;
		}

		try {
			const productsRes = await webapi.get<any>('/api/v1/store/products', { category: catMeta.name, pageSize: 100 }).catch(() => ({ skills: [] }));

			const rawProducts = productsRes.skills || [];

			// Split products by type
			const skillList: any[] = [];
			const workflowList: any[] = [];
			const agentList: any[] = [];

			for (const p of rawProducts) {
				if (p.type === 'workflow') workflowList.push(p);
				else if (p.type === 'role') agentList.push(p);
				else skillList.push(p);
			}

			skills = skillList.map((s: any, i: number) => toAppItem(s, i));
			workflows = workflowList.map((w: any, i: number) => toAppItem({ ...w, type: 'workflow' }, i));
			agents = agentList.map((r: any, i: number) => toAppItem({ ...r, type: 'role' }, i));
		} catch { /* ignore */ }
		loading = false;
	});

	const totalItems = $derived(skills.length + workflows.length + agents.length);
</script>

<!-- Sticky Header -->
<div class="sticky top-0 z-20 bg-base-100/80 backdrop-blur-xl border-b border-base-content/10">
	<div class="flex items-center px-6 h-14">
		<button type="button" class="flex items-center gap-1 text-primary text-base font-medium mr-4" onclick={() => goto('/marketplace/categories')}>
			<ChevronLeft class="w-4 h-4" />
			{$t('marketplace.categories')}
		</button>
	</div>
</div>

<div class="max-w-7xl mx-auto">
{#if !cat}
	<div class="flex flex-col items-center justify-center py-20 text-center px-6">
		<Sparkles class="w-12 h-12 text-base-content/40 mb-4" />
		<p class="text-lg font-semibold mb-1">{$t('marketplace.categoryNotFound')}</p>
		<p class="text-base text-base-content/80 mb-6">{$t('marketplace.categoryNotFoundDesc')}</p>
		<button type="button" class="btn btn-primary btn-sm" onclick={() => goto('/marketplace/categories')}>{$t('marketplace.browseCategories')}</button>
	</div>
{:else if loading}
	<div class="flex justify-center py-16">
		<span class="loading loading-spinner loading-md text-primary"></span>
	</div>
{:else}
	<!-- Hero -->
	<div class="flex flex-col items-center pt-10 pb-8 px-6 text-center max-w-2xl mx-auto">
		<div class="w-20 h-20 rounded-3xl bg-base-200/50 border border-base-content/10 flex items-center justify-center mb-5">
			<span class="text-4xl">{cat.emoji}</span>
		</div>
		<h1 class="font-display text-2xl font-bold">{cat.name}</h1>
		<p class="text-base text-base-content/80 mt-1">{$t('marketplace.itemsAvailable', { values: { count: totalItems } })}</p>
	</div>

	{#if totalItems === 0}
		<div class="flex flex-col items-center justify-center py-12 text-center px-6">
			<Sparkles class="w-10 h-10 text-base-content/40 mb-3" />
			<p class="text-base text-base-content/80">{$t('marketplace.noItemsInCategory')}</p>
		</div>
	{/if}

	<!-- Skills Section -->
	{#if skills.length > 0}
		<div class="px-6 py-6 border-t border-base-content/10">
			<h2 class="font-display text-lg font-bold mb-4">{$t('marketplace.skills')}</h2>
			<div class="grid grid-cols-1 sm:grid-cols-2 xl:grid-cols-3 gap-px">
				{#each skills as item, i}
					<a href={itemHref(item)} class="flex items-center gap-3 py-3 pr-3 hover:bg-base-content/[0.03] transition-colors rounded-lg">
						<span class="w-5 text-right text-base text-base-content/80 font-medium shrink-0">{i + 1}</span>
						<div class="w-14 h-14 rounded-2xl {item.iconBg} flex items-center justify-center text-2xl shrink-0">{item.iconEmoji}</div>
						<div class="flex-1 min-w-0">
							<p class="text-base font-semibold truncate">{item.name}</p>
							<p class="text-sm text-base-content/60 truncate">{item.author}</p>
						</div>
						{#if item.installed}
							<span class="btn-market btn-market-installed shrink-0"><Check class="w-3.5 h-3.5" /></span>
						{:else}
							<button type="button" class="btn-market btn-market-get shrink-0">{$t('marketplace.get')}</button>
						{/if}
					</a>
				{/each}
			</div>
		</div>
	{/if}

	<!-- Workflows Section -->
	{#if workflows.length > 0}
		<div class="px-6 py-6 border-t border-base-content/10">
			<h2 class="font-display text-lg font-bold mb-4">{$t('marketplace.workflows')}</h2>
			<div class="grid grid-cols-1 sm:grid-cols-2 xl:grid-cols-3 gap-px">
				{#each workflows as item, i}
					<a href={itemHref(item)} class="flex items-center gap-3 py-3 pr-3 hover:bg-base-content/[0.03] transition-colors rounded-lg">
						<span class="w-5 text-right text-base text-base-content/80 font-medium shrink-0">{i + 1}</span>
						<div class="w-14 h-14 rounded-2xl {item.iconBg} flex items-center justify-center text-2xl shrink-0">{item.iconEmoji}</div>
						<div class="flex-1 min-w-0">
							<p class="text-base font-semibold truncate">{item.name}</p>
							<p class="text-sm text-base-content/60 truncate">{item.author}</p>
						</div>
						{#if item.installed}
							<span class="btn-market btn-market-installed shrink-0"><Check class="w-3.5 h-3.5" /></span>
						{:else}
							<button type="button" class="btn-market btn-market-get shrink-0">{$t('marketplace.get')}</button>
						{/if}
					</a>
				{/each}
			</div>
		</div>
	{/if}

	<!-- Agents Section -->
	{#if agents.length > 0}
		<div class="px-6 py-6 border-t border-base-content/10">
			<h2 class="font-display text-lg font-bold mb-4">{$t('marketplace.agents')}</h2>
			<div class="grid grid-cols-1 sm:grid-cols-2 xl:grid-cols-3 gap-px">
				{#each agents as item, i}
					<a href={itemHref(item)} class="flex items-center gap-3 py-3 pr-3 hover:bg-base-content/[0.03] transition-colors rounded-lg">
						<span class="w-5 text-right text-base text-base-content/80 font-medium shrink-0">{i + 1}</span>
						<div class="w-14 h-14 rounded-2xl {item.iconBg} flex items-center justify-center text-2xl shrink-0">{item.iconEmoji}</div>
						<div class="flex-1 min-w-0">
							<p class="text-base font-semibold truncate">{item.name}</p>
							<p class="text-sm text-base-content/60 truncate">{item.author}</p>
						</div>
						{#if item.installed}
							<span class="btn-market btn-market-installed shrink-0"><Check class="w-3.5 h-3.5" /></span>
						{:else}
							<button type="button" class="btn-market btn-market-get shrink-0">{$t('marketplace.get')}</button>
						{/if}
					</a>
				{/each}
			</div>
		</div>
	{/if}
{/if}
</div>
