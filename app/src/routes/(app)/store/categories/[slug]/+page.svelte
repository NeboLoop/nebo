<script lang="ts">
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { ChevronLeft, Check, Sparkles } from 'lucide-svelte';
	import webapi from '$lib/api/gocliRequest';
	import { type AppItem, toAppItem, itemHref } from '$lib/types/marketplace';
	import { categoryBySlug, categoryByName } from '$lib/data/categories';

	let loading = $state(true);
	let skills: AppItem[] = $state([]);
	let workflows: AppItem[] = $state([]);
	let roles: AppItem[] = $state([]);
	let catMeta: { name: string; slug: string; emoji: string; gradient: string } | null = $state(null);

	const slug = $derived($page.params.slug);
	const fallback = $derived(categoryBySlug(slug) || categoryByName(decodeURIComponent(slug)));
	const cat = $derived(catMeta || fallback);

	onMount(async () => {
		if (!cat) {
			loading = false;
			return;
		}

		try {
			const productsRes = await webapi.get<any>('/api/v1/store/products', { category: cat.name }).catch(() => ({ skills: [] }));

			const rawProducts = productsRes.skills || [];

			// Split products by type
			const skillList: any[] = [];
			const workflowList: any[] = [];
			const roleList: any[] = [];

			for (const p of rawProducts) {
				if (p.type === 'workflow') workflowList.push(p);
				else if (p.type === 'role') roleList.push(p);
				else skillList.push(p);
			}

			skills = skillList.map((s: any, i: number) => toAppItem(s, i));
			workflows = workflowList.map((w: any, i: number) => toAppItem({ ...w, type: 'workflow' }, i));
			roles = roleList.map((r: any, i: number) => toAppItem({ ...r, type: 'role' }, i));
		} catch { /* ignore */ }
		loading = false;
	});

	const totalItems = $derived(skills.length + workflows.length + roles.length);
</script>

<!-- Sticky Header -->
<div class="sticky top-0 z-20 bg-base-100/80 backdrop-blur-xl border-b border-base-content/10">
	<div class="flex items-center px-6 h-14">
		<button type="button" class="flex items-center gap-1 text-primary text-sm font-medium mr-4" onclick={() => goto('/store/categories')}>
			<ChevronLeft class="w-4 h-4" />
			Categories
		</button>
	</div>
</div>

<div class="max-w-7xl mx-auto">
{#if !cat}
	<div class="flex flex-col items-center justify-center py-20 text-center px-6">
		<Sparkles class="w-12 h-12 text-base-content/10 mb-4" />
		<p class="text-lg font-semibold mb-1">Category not found</p>
		<p class="text-sm text-base-content/80 mb-6">This category doesn't exist or may have been removed.</p>
		<button type="button" class="btn btn-primary btn-sm" onclick={() => goto('/store/categories')}>Browse Categories</button>
	</div>
{:else if loading}
	<div class="flex justify-center py-16">
		<span class="loading loading-spinner loading-md text-primary"></span>
	</div>
{:else}
	<!-- Hero -->
	<div class="flex flex-col items-center pt-10 pb-8 px-6 text-center max-w-2xl mx-auto">
		<div class="w-20 h-20 rounded-3xl bg-gradient-to-br {cat.gradient} flex items-center justify-center mb-5 shadow-lg">
			<span class="text-4xl">{cat.emoji}</span>
		</div>
		<h1 class="font-display text-2xl font-bold">{cat.name}</h1>
		<p class="text-sm text-base-content/90 mt-1">{totalItems} {totalItems === 1 ? 'item' : 'items'} available</p>
	</div>

	{#if totalItems === 0}
		<div class="flex flex-col items-center justify-center py-12 text-center px-6">
			<Sparkles class="w-10 h-10 text-base-content/10 mb-3" />
			<p class="text-sm text-base-content/90">No items in this category yet</p>
		</div>
	{/if}

	<!-- Skills Section -->
	{#if skills.length > 0}
		<div class="px-6 py-6 border-t border-base-content/10">
			<h2 class="font-display text-lg font-bold mb-4">Skills</h2>
			<div class="grid grid-cols-1 sm:grid-cols-2 xl:grid-cols-3 gap-px">
				{#each skills as item, i}
					<a href={itemHref(item)} class="flex items-center gap-3 py-3 pr-3 hover:bg-base-content/[0.03] transition-colors rounded-lg">
						<span class="w-5 text-right text-sm text-base-content/70 font-medium shrink-0">{i + 1}</span>
						<div class="w-14 h-14 rounded-2xl {item.iconBg} flex items-center justify-center text-2xl shrink-0">{item.iconEmoji}</div>
						<div class="flex-1 min-w-0">
							<p class="text-sm font-semibold truncate">{item.name}</p>
							<p class="text-xs text-base-content/80 truncate">{item.author}</p>
						</div>
						{#if item.installed}
							<span class="btn-market btn-market-installed shrink-0"><Check class="w-3.5 h-3.5" /></span>
						{:else}
							<button type="button" class="btn-market btn-market-get shrink-0">GET</button>
						{/if}
					</a>
				{/each}
			</div>
		</div>
	{/if}

	<!-- Workflows Section -->
	{#if workflows.length > 0}
		<div class="px-6 py-6 border-t border-base-content/10">
			<h2 class="font-display text-lg font-bold mb-4">Workflows</h2>
			<div class="grid grid-cols-1 sm:grid-cols-2 xl:grid-cols-3 gap-px">
				{#each workflows as item, i}
					<a href={itemHref(item)} class="flex items-center gap-3 py-3 pr-3 hover:bg-base-content/[0.03] transition-colors rounded-lg">
						<span class="w-5 text-right text-sm text-base-content/70 font-medium shrink-0">{i + 1}</span>
						<div class="w-14 h-14 rounded-2xl {item.iconBg} flex items-center justify-center text-2xl shrink-0">{item.iconEmoji}</div>
						<div class="flex-1 min-w-0">
							<p class="text-sm font-semibold truncate">{item.name}</p>
							<p class="text-xs text-base-content/80 truncate">{item.author}</p>
						</div>
						{#if item.installed}
							<span class="btn-market btn-market-installed shrink-0"><Check class="w-3.5 h-3.5" /></span>
						{:else}
							<button type="button" class="btn-market btn-market-get shrink-0">GET</button>
						{/if}
					</a>
				{/each}
			</div>
		</div>
	{/if}

	<!-- Roles Section -->
	{#if roles.length > 0}
		<div class="px-6 py-6 border-t border-base-content/10">
			<h2 class="font-display text-lg font-bold mb-4">Roles</h2>
			<div class="grid grid-cols-1 sm:grid-cols-2 xl:grid-cols-3 gap-px">
				{#each roles as item, i}
					<a href={itemHref(item)} class="flex items-center gap-3 py-3 pr-3 hover:bg-base-content/[0.03] transition-colors rounded-lg">
						<span class="w-5 text-right text-sm text-base-content/70 font-medium shrink-0">{i + 1}</span>
						<div class="w-14 h-14 rounded-2xl {item.iconBg} flex items-center justify-center text-2xl shrink-0">{item.iconEmoji}</div>
						<div class="flex-1 min-w-0">
							<p class="text-sm font-semibold truncate">{item.name}</p>
							<p class="text-xs text-base-content/80 truncate">{item.author}</p>
						</div>
						{#if item.installed}
							<span class="btn-market btn-market-installed shrink-0"><Check class="w-3.5 h-3.5" /></span>
						{:else}
							<button type="button" class="btn-market btn-market-get shrink-0">GET</button>
						{/if}
					</a>
				{/each}
			</div>
		</div>
	{/if}
{/if}
</div>
