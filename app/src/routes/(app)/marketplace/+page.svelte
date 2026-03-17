<script lang="ts">
	import { onMount } from 'svelte';
	import {
		Code,
		Sparkles
	} from 'lucide-svelte';
	import LargeCard from '$lib/components/marketplace/LargeCard.svelte';
	import SectionEditorial from '$lib/components/marketplace/sections/SectionEditorial.svelte';
	import SectionListGrid from '$lib/components/marketplace/sections/SectionListGrid.svelte';
	import webapi from '$lib/api/gocliRequest';
	import { type AppItem, toAppItem } from '$lib/types/marketplace';

	let loading = $state(true);

	let featuredSkills: AppItem[] = $state([]);
	let roles: AppItem[] = $state([]);
	let skillItems: AppItem[] = $state([]);

	onMount(async () => {
		try {
			const [
				rolesRes,
				skillsRes,
				featuredSkillRes
			] = await Promise.all([
				webapi.get<any>('/api/v1/store/products', { type: 'role' }).catch(() => ({ skills: [] })),
				webapi.get<any>('/api/v1/store/products', { type: 'skill' }).catch(() => ({ skills: [] })),
				webapi.get<any>('/api/v1/store/featured', { type: 'skill' }).catch(() => ({ apps: [] }))
			]);

			roles = (rolesRes.skills || []).map((s: any, i: number) => toAppItem({ ...s, type: 'role' }, i));
			skillItems = (skillsRes.skills || []).map((s: any, i: number) => toAppItem({ ...s, type: 'skill' }, i));
			featuredSkills = (featuredSkillRes.apps || []).map((a: any, i: number) =>
				toAppItem({ ...a, type: a.type || 'skill' }, i)
			);
		} catch {
			/* ignore */
		}
		loading = false;
	});
</script>

<div class="max-w-7xl mx-auto">
	<!-- Hero -->
	<div class="px-6 pt-8 pb-2">
		<h2 class="font-display text-3xl sm:text-4xl font-black leading-tight">
			Marketplace
		</h2>
		<p class="text-base text-base-content/90 mt-2 max-w-xl">
			Roles, skills, and tools for your desktop AI companion.
		</p>
	</div>

	<!-- Featured Skills -->
	<SectionEditorial title="Featured Skills" items={featuredSkills} />

	<!-- Roles — LargeCard grid -->
	<div class="pt-8 pb-2">
		<div class="flex items-baseline justify-between px-6 mb-4">
			<div>
				<h3 class="font-display text-xl font-bold">Roles</h3>
				<p class="text-sm text-base-content/60 mt-0.5">Job profiles that put Nebo to work</p>
			</div>
			<a href="/marketplace/roles" class="text-base text-primary font-medium">Browse All</a>
		</div>
		{#if loading}
			<div class="grid grid-cols-1 sm:grid-cols-2 gap-4 px-6">
				{#each Array(2) as _}
					<div
						class="rounded-2xl bg-base-content/5 border border-base-content/10 h-64 animate-pulse"
					></div>
				{/each}
			</div>
		{:else if roles.length > 0}
			<div class="grid grid-cols-1 sm:grid-cols-2 gap-4 px-6">
				{#each roles.slice(0, 4) as role}
					<LargeCard item={role} />
				{/each}
			</div>
		{:else}
			<div class="flex flex-col items-center justify-center py-12 text-center px-6">
				<Sparkles class="w-8 h-8 text-base-content/40 mb-2" />
				<p class="text-base text-base-content/80">No roles available yet</p>
			</div>
		{/if}
	</div>

	<!-- Skills -->
	<SectionListGrid title="Skills" seeAllHref="/marketplace/skills" items={skillItems} />
	{#if !loading && skillItems.length === 0}
		<div class="flex flex-col items-center justify-center py-12 text-center px-6">
			<Sparkles class="w-8 h-8 text-base-content/40 mb-2" />
			<p class="text-base text-base-content/80">No skills available yet</p>
		</div>
	{/if}

	<!-- Build for Nebo -->
	<div class="px-6 pt-8 pb-8">
		<div class="rounded-2xl border border-primary/20 bg-primary/5 p-8 sm:p-10 text-center">
			<div
				class="w-14 h-14 rounded-2xl bg-primary/15 flex items-center justify-center mx-auto mb-4"
			>
				<Code class="w-7 h-7 text-primary" />
			</div>
			<h3 class="font-display text-2xl font-bold">Build for Nebo</h3>
			<p class="text-base text-base-content/80 mt-2 max-w-md mx-auto">
				Create roles and skills. Publish to the marketplace.
			</p>
		</div>
	</div>
</div>
