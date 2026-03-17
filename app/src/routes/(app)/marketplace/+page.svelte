<script lang="ts">
	import { onMount } from 'svelte';
	import {
		Code,
		Sparkles
	} from 'lucide-svelte';
	import InstallCode from '$lib/components/InstallCode.svelte';
	import SectionEditorial from '$lib/components/marketplace/sections/SectionEditorial.svelte';
	import SectionListGrid from '$lib/components/marketplace/sections/SectionListGrid.svelte';
	import webapi from '$lib/api/gocliRequest';
	import { type AppItem, toAppItem } from '$lib/types/marketplace';

	let loading = $state(true);

	let featuredSkill: AppItem | null = $state(null);
	let featuredSkills: AppItem[] = $state([]);
	let allSkills: AppItem[] = $state([]);

	onMount(async () => {
		try {
			const [skillsRes, roleRes, workflowRes, featuredRes] = await Promise.all([
				webapi.get<any>('/api/v1/store/products', { type: 'skill' }).catch(() => ({ skills: [] })),
				webapi.get<any>('/api/v1/store/products', { type: 'role' }).catch(() => ({ skills: [] })),
				webapi.get<any>('/api/v1/store/products', { type: 'workflow' }).catch(() => ({ skills: [] })),
				webapi.get<any>('/api/v1/store/featured', { type: 'skill' }).catch(() => ({ apps: [] }))
			]);

			// Merge all types into a single skills list
			const skills = (skillsRes.skills || []).map((s: any, i: number) => toAppItem({ ...s, type: s.type || 'skill' }, i));
			const roles = (roleRes.skills || []).map((s: any, i: number) => toAppItem({ ...s, type: s.type || 'role' }, i + 100));
			const workflows = (workflowRes.skills || []).map((s: any, i: number) => toAppItem({ ...s, type: s.type || 'workflow' }, i + 200));
			allSkills = [...roles, ...skills, ...workflows];

			const featuredApps = (featuredRes.apps || []).map((a: any, i: number) =>
				toAppItem({ ...a, type: a.type || 'skill' }, i)
			);
			featuredSkills = featuredApps;
			featuredSkill = featuredApps[0] || null;
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
			Skills for your desktop AI companion.
		</p>
	</div>

	<!-- Featured Skill -->
	{#if loading}
		<div class="px-6 pt-6 pb-2">
			<div class="rounded-2xl bg-base-content/5 p-6 sm:p-8 animate-pulse">
				<div class="flex items-start gap-4 mb-6">
					<div class="w-14 h-14 rounded-2xl bg-base-content/10"></div>
					<div class="flex-1">
						<div class="h-4 w-24 bg-base-content/10 rounded mb-2"></div>
						<div class="h-6 w-48 bg-base-content/10 rounded mb-2"></div>
						<div class="h-4 w-64 bg-base-content/10 rounded"></div>
					</div>
				</div>
			</div>
		</div>
	{:else if featuredSkill}
		<div class="px-6 pt-6 pb-2">
			<div class="rounded-2xl bg-gradient-to-br {featuredSkill.iconBg} p-6 sm:p-8">
				<div class="flex items-start gap-4 mb-6">
					<div
						class="w-14 h-14 rounded-2xl bg-base-100/50 flex items-center justify-center text-2xl shrink-0"
					>
						{featuredSkill.iconEmoji}
					</div>
					<div class="flex-1 min-w-0">
						<p class="text-base font-semibold uppercase tracking-wider text-base-content/60">
							Featured Skill
						</p>
						<h3 class="font-display text-2xl sm:text-3xl font-bold mt-0.5">{featuredSkill.name}</h3>
						<p class="text-base text-base-content/90 mt-1">{featuredSkill.description}</p>
					</div>
				</div>

				<div class="flex items-center justify-between mt-6">
					<InstallCode code={featuredSkill.code} compact />
					<span class="btn-market btn-market-get">{featuredSkill.price}</span>
				</div>
			</div>
		</div>
	{/if}

	<!-- Featured Skills -->
	<SectionEditorial title="Featured Skills" items={featuredSkills} />

	<!-- All Skills -->
	<SectionListGrid title="Skills" seeAllHref="/marketplace/skills" items={allSkills} />
	{#if !loading && allSkills.length === 0}
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
				Create skills and publish them to the marketplace.
			</p>
		</div>
	</div>
</div>
