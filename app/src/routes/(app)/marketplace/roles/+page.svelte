<script lang="ts">
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import { Sparkles } from 'lucide-svelte';
	import SectionFeaturedPair from '$lib/components/marketplace/sections/SectionFeaturedPair.svelte';
	import SectionTopRanked from '$lib/components/marketplace/sections/SectionTopRanked.svelte';
	import SectionListGrid from '$lib/components/marketplace/sections/SectionListGrid.svelte';
	import webapi from '$lib/api/gocliRequest';
	import { type AppItem, toAppItem } from '$lib/types/marketplace';
	import { slugify } from '$lib/data/categories';

	let loading = $state(true);
	let allRoles: AppItem[] = $state([]);
	let featured: AppItem[] = $state([]);

	onMount(async () => {
		try {
			const [productsRes, featuredRes] = await Promise.all([
				webapi.get<any>('/api/v1/store/products', { type: 'role', pageSize: 100 }).catch(() => ({ skills: [] })),
				webapi.get<any>('/api/v1/store/featured', { type: 'role' }).catch(() => ({ apps: [] }))
			]);

			const roles = productsRes.skills || [];
			allRoles = roles.map((r: any, i: number) => toAppItem({ ...r, type: 'role' }, i));
			featured = (featuredRes.apps || []).map((a: any, i: number) => toAppItem({ ...a, type: a.type || 'role' }, i));
		} catch { /* ignore */ }
		loading = false;
	});

	const categories = $derived([...new Set(allRoles.map(r => r.category).filter(Boolean))]);
	const byCategory = $derived(
		categories.map(cat => ({
			name: cat,
			items: allRoles.filter(r => r.category === cat)
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
	<SectionFeaturedPair items={featured} label={$t('marketplace.rolesPage.featuredRole')} />

	<!-- Description -->
	<div class="px-6 py-6 border-b border-base-content/10 max-w-2xl">
		<h2 class="font-display text-lg font-bold mb-2">{$t('marketplace.rolesPage.aboutRoles')}</h2>
		<p class="text-base text-base-content/80 leading-relaxed">{$t('marketplace.rolesPage.aboutRolesDesc')}</p>
	</div>

	<!-- Top Roles -->
	{#if allRoles.length > 0}
		<SectionTopRanked title={$t('marketplace.rolesPage.topRoles')} items={allRoles.slice(0, 21)} />
	{:else}
		<div class="px-6 py-6">
			<h2 class="font-display text-lg font-bold mb-4">{$t('marketplace.rolesPage.topRoles')}</h2>
			<div class="flex flex-col items-center justify-center py-12 text-center">
				<Sparkles class="w-10 h-10 text-base-content/40 mb-3" />
				<p class="text-base text-base-content/80">{$t('marketplace.rolesPage.noRoles')}</p>
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
