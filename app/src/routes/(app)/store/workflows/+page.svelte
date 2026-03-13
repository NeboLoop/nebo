<script lang="ts">
	import { onMount } from 'svelte';
	import { Sparkles } from 'lucide-svelte';
	import SectionEditorial from '$lib/components/marketplace/sections/SectionEditorial.svelte';
	import SectionTopRanked from '$lib/components/marketplace/sections/SectionTopRanked.svelte';
	import SectionListGrid from '$lib/components/marketplace/sections/SectionListGrid.svelte';
	import webapi from '$lib/api/gocliRequest';
	import { type AppItem, toAppItem } from '$lib/types/marketplace';
	import { slugify } from '$lib/data/categories';

	let loading = $state(true);
	let allWorkflows: AppItem[] = $state([]);
	let featured: AppItem[] = $state([]);

	onMount(async () => {
		try {
			const [productsRes, featuredRes] = await Promise.all([
				webapi.get<any>('/api/v1/store/products', { type: 'workflow' }).catch(() => ({ skills: [] })),
				webapi.get<any>('/api/v1/store/featured', { type: 'workflow' }).catch(() => ({ apps: [] }))
			]);

			const workflows = productsRes.skills || [];
			allWorkflows = workflows.map((w: any, i: number) => toAppItem({ ...w, type: 'workflow' }, i));
			featured = (featuredRes.apps || []).map((a: any, i: number) => toAppItem({ ...a, type: a.type || 'workflow' }, i));
		} catch { /* ignore */ }
		loading = false;
	});

	const categories = $derived([...new Set(allWorkflows.map(w => w.category).filter(Boolean))]);
	const byCategory = $derived(
		categories.map(cat => ({
			name: cat,
			items: allWorkflows.filter(w => w.category === cat)
		})).filter(g => g.items.length > 0)
	);
</script>

<div class="max-w-7xl mx-auto">
{#if loading}
	<div class="flex justify-center py-16">
		<span class="loading loading-spinner loading-md text-primary"></span>
	</div>
{:else}
	<!-- Featured -->
	<SectionEditorial items={featured} />

	<!-- Description -->
	<div class="px-6 py-6 border-b border-base-content/10 max-w-2xl">
		<h2 class="font-display text-lg font-bold mb-2">About Workflows</h2>
		<p class="text-sm text-base-content/80 leading-relaxed">Coordinate multiple bots and tools in sequence to complete complex tasks. Chain actions together -- from collection to processing to delivery.</p>
	</div>

	<!-- Top Workflows -->
	{#if allWorkflows.length > 0}
		<SectionTopRanked title="Top Workflows" items={allWorkflows.slice(0, 21)} />
	{:else}
		<div class="px-6 py-6">
			<h2 class="font-display text-lg font-bold mb-4">Top Workflows</h2>
			<div class="flex flex-col items-center justify-center py-12 text-center">
				<Sparkles class="w-10 h-10 text-base-content/10 mb-3" />
				<p class="text-sm text-base-content/90">No workflows available yet</p>
			</div>
		</div>
	{/if}

	<!-- Browse by Category -->
	{#each byCategory as group}
		<div class="border-t border-base-content/10">
			<SectionListGrid title={group.name} seeAllHref="/store/categories/{slugify(group.name)}" items={group.items} />
		</div>
	{/each}
{/if}
</div>
