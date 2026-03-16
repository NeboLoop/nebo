<script lang="ts">
	import { onMount } from 'svelte';
	import {
		Clock,
		Bell,
		Sun,
		Moon,
		Code,
		Sparkles
	} from 'lucide-svelte';
	import LargeCard from '$lib/components/marketplace/LargeCard.svelte';
	import InstallCode from '$lib/components/InstallCode.svelte';
	import SectionEditorial from '$lib/components/marketplace/sections/SectionEditorial.svelte';
	import SectionListGrid from '$lib/components/marketplace/sections/SectionListGrid.svelte';
	import webapi from '$lib/api/gocliRequest';
	import { type AppItem, toAppItem } from '$lib/types/marketplace';

	let loading = $state(true);

	let featuredRole: AppItem | null = $state(null);
	let featuredSkills: AppItem[] = $state([]);
	let featuredWorkflows: AppItem[] = $state([]);
	let roles: AppItem[] = $state([]);
	let workflowItems: AppItem[] = $state([]);
	let skillItems: AppItem[] = $state([]);

	const chiefOfStaffWorkflows = [
		{
			icon: Sun,
			time: '7:00 AM',
			label: 'Morning Briefing',
			desc: "What's on today, what matters most, what can wait"
		},
		{
			icon: Clock,
			time: 'Every 30m',
			label: 'Day Monitor',
			desc: 'Watches for changes, interrupts only when it matters'
		},
		{
			icon: Moon,
			time: '6:00 PM',
			label: 'Evening Wrap',
			desc: "What happened, what's unresolved, what's tomorrow"
		},
		{
			icon: Bell,
			time: 'On event',
			label: 'Urgent Interrupt',
			desc: 'Something needs attention now'
		}
	];

	function loadData(data: Record<string, any>) {
		roles = (data.roles || []).map((s: any, i: number) => toAppItem({ ...s, type: 'role' }, i));
		workflowItems = (data.workflows || []).map((s: any, i: number) =>
			toAppItem({ ...s, type: 'workflow' }, i)
		);
		skillItems = (data.skills || []).map((s: any, i: number) =>
			toAppItem({ ...s, type: 'skill' }, i)
		);

		const fr = (data.featuredRole || []).map((a: any, i: number) =>
			toAppItem({ ...a, type: 'role' }, i)
		);
		featuredRole = fr[0] || null;
		featuredSkills = (data.featuredSkill || []).map((a: any, i: number) =>
			toAppItem({ ...a, type: a.type || 'skill' }, i)
		);
		featuredWorkflows = (data.featuredWorkflow || []).map((a: any, i: number) =>
			toAppItem({ ...a, type: a.type || 'workflow' }, i)
		);
	}

	onMount(async () => {
		try {
			const [
				rolesRes,
				workflowsRes,
				skillsRes,
				featuredRoleRes,
				featuredSkillRes,
				featuredWorkflowRes
			] = await Promise.all([
				webapi.get<any>('/api/v1/store/products', { type: 'role' }).catch(() => ({ skills: [] })),
				webapi.get<any>('/api/v1/store/products', { type: 'workflow' }).catch(() => ({ skills: [] })),
				webapi.get<any>('/api/v1/store/products', { type: 'skill' }).catch(() => ({ skills: [] })),
				webapi.get<any>('/api/v1/store/featured', { type: 'role' }).catch(() => ({ apps: [] })),
				webapi.get<any>('/api/v1/store/featured', { type: 'skill' }).catch(() => ({ apps: [] })),
				webapi.get<any>('/api/v1/store/featured', { type: 'workflow' }).catch(() => ({ apps: [] }))
			]);

			loadData({
				roles: rolesRes.skills || [],
				workflows: workflowsRes.skills || [],
				skills: skillsRes.skills || [],
				featuredRole: featuredRoleRes.apps || [],
				featuredSkill: featuredSkillRes.apps || [],
				featuredWorkflow: featuredWorkflowRes.apps || []
			});
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
			Roles, skills, and workflows for your desktop AI.
		</p>
	</div>

	<!-- Featured Role -- FeaturedCard or editorial card with workflows -->
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
	{:else if featuredRole}
		<div class="px-6 pt-6 pb-2">
			<div class="rounded-2xl bg-gradient-to-br {featuredRole.iconBg} p-6 sm:p-8">
				<div class="flex items-start gap-4 mb-6">
					<div
						class="w-14 h-14 rounded-2xl bg-base-100/50 flex items-center justify-center text-2xl shrink-0"
					>
						{featuredRole.iconEmoji}
					</div>
					<div class="flex-1 min-w-0">
						<p class="text-base font-semibold uppercase tracking-wider text-base-content/60">
							Featured Role
						</p>
						<h3 class="font-display text-2xl sm:text-3xl font-bold mt-0.5">{featuredRole.name}</h3>
						<p class="text-base text-base-content/90 mt-1">{featuredRole.description}</p>
					</div>
				</div>

				<div class="grid grid-cols-1 sm:grid-cols-2 gap-3 mb-6">
					{#each chiefOfStaffWorkflows as wf}
						{@const Icon = wf.icon}
						<div class="flex items-start gap-3 rounded-xl bg-base-100/30 p-4">
							<div
								class="w-9 h-9 rounded-lg bg-base-100/50 flex items-center justify-center shrink-0"
							>
								<Icon class="w-4 h-4 text-base-content/90" />
							</div>
							<div class="min-w-0">
								<div class="flex items-center gap-2">
									<p class="text-base font-semibold">{wf.label}</p>
									<span class="text-base text-base-content/80">{wf.time}</span>
								</div>
								<p class="text-base text-base-content/80 mt-0.5">{wf.desc}</p>
							</div>
						</div>
					{/each}
				</div>

				<div class="flex items-center justify-between">
					<InstallCode code={featuredRole.code} compact />
					<span class="btn-market btn-market-get">{featuredRole.price}</span>
				</div>
			</div>
		</div>
	{/if}

	<!-- Featured Skills -->
	<SectionEditorial title="Featured Skills" items={featuredSkills} />

	<!-- Featured Workflows -->
	<SectionEditorial title="Featured Workflows" items={featuredWorkflows} />

	<!-- Roles -- LargeCard grid -->
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

	<!-- Workflows -->
	<SectionListGrid title="Workflows" seeAllHref="/marketplace/workflows" items={workflowItems} />
	{#if !loading && workflowItems.length === 0}
		<div class="flex flex-col items-center justify-center py-12 text-center px-6">
			<Sparkles class="w-8 h-8 text-base-content/40 mb-2" />
			<p class="text-base text-base-content/80">No workflows available yet</p>
		</div>
	{/if}

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
				Create Skills and Workflows. Compose them into Roles. Publish to the marketplace.
			</p>
		</div>
	</div>
</div>
