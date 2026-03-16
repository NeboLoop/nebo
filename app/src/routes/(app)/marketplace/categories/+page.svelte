<script lang="ts">
	import { onMount } from 'svelte';
	import { Grid3x3 } from 'lucide-svelte';
	import webapi from '$lib/api/gocliRequest';
	import { categories as fallbackCategories } from '$lib/data/categories';

	interface CategoryItem {
		name: string;
		slug: string;
		emoji: string;
		gradient: string;
		skillCount: number;
		workflowCount: number;
		roleCount: number;
		toolCount: number;
	}

	let categories: CategoryItem[] = $state([]);
	let loading = $state(true);

	onMount(async () => {
		try {
			const res = await webapi.get<any>('/api/v1/store/categories');
			categories = res.categories || [];
		} catch {
			categories = fallbackCategories.map((c) => ({
				...c,
				skillCount: 0,
				workflowCount: 0,
				roleCount: 0,
				toolCount: 0
			}));
		}
		loading = false;
	});
</script>

<div class="sticky top-0 z-20 bg-base-100/80 backdrop-blur-xl border-b border-base-content/10">
	<div class="flex items-center px-6 h-14">
		<h1 class="font-display text-xl font-bold">Categories</h1>
	</div>
</div>

<div class="max-w-7xl mx-auto">
<!-- Hero -->
<div class="flex flex-col items-center pt-10 pb-8 px-6 text-center max-w-2xl mx-auto">
	<div class="w-20 h-20 rounded-3xl bg-gradient-to-br from-base-content/10 to-base-content/40 flex items-center justify-center mb-5">
		<Grid3x3 class="w-10 h-10 text-base-content/90" />
	</div>
	<h2 class="font-display text-2xl font-bold">Browse by Category</h2>
	<p class="text-base text-base-content/80 mt-1">Find skills, workflows, and roles organized by industry and function.</p>
</div>

{#if loading}
	<div class="flex justify-center py-16">
		<span class="loading loading-spinner loading-md text-primary"></span>
	</div>
{:else}
	<!-- Category Tiles -->
	<div class="px-6 pb-8 grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5 gap-3">
		{#each categories as cat}
			{@const total = (cat.skillCount || 0) + (cat.workflowCount || 0) + (cat.roleCount || 0) + (cat.toolCount || 0)}
			<a href="/marketplace/categories/{cat.slug}" class="group flex flex-col items-center gap-3 p-5 rounded-2xl bg-gradient-to-br {cat.gradient} hover:scale-[1.03] transition-all duration-200">
				<span class="text-4xl drop-shadow-lg">{cat.emoji}</span>
				<span class="text-base font-bold text-white text-center drop-shadow-sm">{cat.name}</span>
				{#if total > 0}
					<span class="text-sm font-medium text-white/80">{total} {total === 1 ? 'item' : 'items'}</span>
				{/if}
			</a>
		{/each}
	</div>
{/if}
</div>
