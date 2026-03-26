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
	let allSkills: AppItem[] = $state([]);
	let topSkills: AppItem[] = $state([]);
	let featured: AppItem[] = $state([]);

	onMount(async () => {
		try {
			const [productsRes, topRes, featuredRes] = await Promise.all([
				webapi.get<any>('/api/v1/store/products', { type: 'skill', pageSize: 100 }).catch(() => ({ skills: [] })),
				webapi.get<any>('/api/v1/store/products/top', { pageSize: 100 }).catch(() => ({ skills: [] })),
				webapi.get<any>('/api/v1/store/featured', { type: 'skill' }).catch(() => ({ apps: [] }))
			]);

			const skills = productsRes.skills || [];
			const top = topRes.skills || [];

			allSkills = skills.map((s: any, i: number) => toAppItem(s, i));
			topSkills = top.map((s: any, i: number) => toAppItem(s, i));
			featured = (featuredRes.apps || []).map((a: any, i: number) => toAppItem({ ...a, type: a.type || 'skill' }, i));
		} catch { /* ignore */ }
		loading = false;
	});

	const categories = $derived([...new Set(allSkills.map(s => s.category).filter(Boolean))]);
	const byCategory = $derived(
		categories.map(cat => ({
			name: cat,
			items: allSkills.filter(s => s.category === cat)
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
		<h2 class="font-display text-lg font-bold mb-2">About Skills</h2>
		<p class="text-base text-base-content/80 leading-relaxed">Markdown instructions that give bots new abilities -- no code required. Install to any bot instantly and teach it new behaviors.</p>
	</div>

	<!-- Top Skills -->
	{#if topSkills.length > 0}
		<SectionTopRanked title="Top Skills" items={topSkills} />
	{:else if allSkills.length > 0}
		<SectionTopRanked title="Top Skills" items={allSkills.slice(0, 21)} />
	{:else}
		<div class="px-6 py-6">
			<h2 class="font-display text-lg font-bold mb-4">Top Skills</h2>
			<div class="flex flex-col items-center justify-center py-12 text-center">
				<Sparkles class="w-10 h-10 text-base-content/40 mb-3" />
				<p class="text-base text-base-content/80">No skills available yet</p>
			</div>
		</div>
	{/if}

	<!-- Browse by Category -->
	{#each byCategory as group}
		<div class="border-t border-base-content/10">
			<SectionListGrid title={group.name} seeAllHref="/marketplace/categories/{slugify(group.name)}" items={group.items} />
		</div>
	{/each}
{/if}
</div>
