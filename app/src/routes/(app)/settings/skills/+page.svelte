<script lang="ts">
	import { onMount } from 'svelte';
	import Card from '$lib/components/ui/Card.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import SkillEditorModal from '$lib/components/skills/SkillEditorModal.svelte';
	import { Zap, RefreshCw, Power, Store, Download, Check, WifiOff, Star, Plus, Pencil, Trash2 } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import type { ExtensionSkill, StoreSkill } from '$lib/api/nebo';

	let skills = $state<ExtensionSkill[]>([]);
	let storeSkills = $state<StoreSkill[]>([]);
	let neboLoopConnected = $state(false);
	let isLoading = $state(true);
	let isLoadingStore = $state(false);
	let togglingSkill = $state<string | null>(null);
	let installingSkill = $state<string | null>(null);
	let deletingSkill = $state<string | null>(null);

	let showEditor = $state(false);
	let editingSkill = $state<ExtensionSkill | null>(null);

	onMount(async () => {
		await loadAll();
	});

	async function loadAll() {
		isLoading = true;
		try {
			const [extensionsResp, loopStatus] = await Promise.all([
				api.listExtensions(),
				api.neboLoopStatus()
			]);
			skills = extensionsResp.skills || [];
			neboLoopConnected = loopStatus.connected;

			if (neboLoopConnected) {
				loadStoreSkills();
			}
		} catch (error) {
			console.error('Failed to load skills:', error);
		} finally {
			isLoading = false;
		}
	}

	async function loadStoreSkills() {
		isLoadingStore = true;
		try {
			const resp = await api.listStoreSkills();
			storeSkills = resp.skills || [];
		} catch (error) {
			console.error('Failed to load store skills:', error);
		} finally {
			isLoadingStore = false;
		}
	}

	async function handleToggle(name: string) {
		togglingSkill = name;
		try {
			await api.toggleSkill(name);
			await loadAll();
		} catch (error) {
			console.error('Failed to toggle skill:', error);
		} finally {
			togglingSkill = null;
		}
	}

	async function handleDelete(skill: ExtensionSkill) {
		if (!confirm(`Delete skill "${skill.name}"? This cannot be undone.`)) return;
		deletingSkill = skill.name;
		try {
			await api.deleteSkill(skill.name);
			await loadAll();
		} catch (error) {
			console.error('Failed to delete skill:', error);
		} finally {
			deletingSkill = null;
		}
	}

	function openCreate() {
		editingSkill = null;
		showEditor = true;
	}

	function openEdit(skill: ExtensionSkill) {
		editingSkill = skill;
		showEditor = true;
	}

	async function handleInstall(skill: StoreSkill) {
		installingSkill = skill.id;
		try {
			await api.installStoreSkill(skill.id);
			await loadAll();
		} catch (error) {
			console.error('Failed to install skill:', error);
		} finally {
			installingSkill = null;
		}
	}

	async function handleUninstall(skill: StoreSkill) {
		installingSkill = skill.id;
		try {
			await api.uninstallStoreSkill(skill.id);
			await loadAll();
		} catch (error) {
			console.error('Failed to uninstall skill:', error);
		} finally {
			installingSkill = null;
		}
	}
</script>

<div class="mb-6 flex items-center justify-between">
	<div>
		<h2 class="font-display text-xl font-bold text-base-content mb-1">Skills</h2>
		<p class="text-sm text-base-content/60">Standalone orchestration skills for the agent</p>
	</div>
	<div class="flex items-center gap-2">
		<Button type="primary" onclick={openCreate}>
			<Plus class="w-4 h-4 mr-2" />
			Create Skill
		</Button>
		<Button type="ghost" onclick={loadAll}>
			<RefreshCw class="w-4 h-4 mr-2" />
			Refresh
		</Button>
	</div>
</div>

{#if isLoading}
	<Card>
		<div class="py-12 text-center text-base-content/60">
			<span class="loading loading-spinner loading-md"></span>
			<p class="mt-2">Loading skills...</p>
		</div>
	</Card>
{:else}
	<!-- Local Skills -->
	<div class="mb-8">
		<h3 class="text-sm font-semibold uppercase tracking-wider text-base-content/40 mb-4">Installed Skills</h3>

		{#if skills.length > 0}
			<div class="grid sm:grid-cols-2 lg:grid-cols-3 gap-4">
				{#each skills as skill}
					<Card class="hover:border-primary/30 transition-colors">
						<div class="flex items-start gap-3">
							<div class="w-10 h-10 rounded-xl {skill.enabled ? 'bg-primary/10' : 'bg-base-200'} flex items-center justify-center shrink-0">
								<Zap class="w-5 h-5 {skill.enabled ? 'text-primary' : 'text-base-content/30'}" />
							</div>
							<div class="flex-1 min-w-0">
								<div class="flex items-center justify-between gap-2 mb-1">
									<div class="flex items-center gap-2">
										<h3 class="font-display font-bold text-base-content">{skill.name}</h3>
										<span class="badge badge-sm badge-outline">v{skill.version}</span>
										{#if skill.source === 'bundled'}
											<span class="badge badge-sm badge-ghost">Bundled</span>
										{/if}
									</div>
									<div class="flex items-center gap-1">
										{#if skill.editable}
											<button
												class="btn btn-xs btn-ghost text-base-content/40 hover:text-primary"
												onclick={() => openEdit(skill)}
												title="Edit skill"
											>
												<Pencil class="w-3.5 h-3.5" />
											</button>
											<button
												class="btn btn-xs btn-ghost text-base-content/40 hover:text-error"
												onclick={() => handleDelete(skill)}
												disabled={deletingSkill === skill.name}
												title="Delete skill"
											>
												{#if deletingSkill === skill.name}
													<span class="loading loading-spinner loading-xs"></span>
												{:else}
													<Trash2 class="w-3.5 h-3.5" />
												{/if}
											</button>
										{/if}
										<button
											class="btn btn-xs btn-ghost {skill.enabled ? 'text-success' : 'text-base-content/40'}"
											onclick={() => handleToggle(skill.name)}
											disabled={togglingSkill === skill.name}
											title={skill.enabled ? 'Click to disable' : 'Click to enable'}
										>
											{#if togglingSkill === skill.name}
												<span class="loading loading-spinner loading-xs"></span>
											{:else}
												<Power class="w-4 h-4" />
											{/if}
										</button>
									</div>
								</div>
								<p class="text-sm text-base-content/60 mb-2 {!skill.enabled ? 'opacity-50' : ''}">{skill.description}</p>

								{#if skill.tags && skill.tags.length > 0}
									<div class="flex flex-wrap gap-1 mb-2 {!skill.enabled ? 'opacity-50' : ''}">
										{#each skill.tags.slice(0, 3) as tag}
											<span class="badge badge-sm badge-outline">{tag}</span>
										{/each}
										{#if skill.tags.length > 3}
											<span class="badge badge-sm badge-ghost">+{skill.tags.length - 3}</span>
										{/if}
									</div>
								{/if}

								{#if skill.tools && skill.tools.length > 0}
									<div class="text-xs text-base-content/50 {!skill.enabled ? 'opacity-50' : ''}">
										Uses: {skill.tools.join(', ')}
									</div>
								{/if}
							</div>
						</div>
					</Card>
				{/each}
			</div>
		{:else}
			<Card>
				<div class="py-12 text-center text-base-content/60">
					<Zap class="w-12 h-12 mx-auto mb-4 opacity-20" />
					<p class="font-medium mb-2">No skills found</p>
					<p class="text-sm">Create a skill or browse the store.</p>
				</div>
			</Card>
		{/if}
	</div>

	<!-- Skill Store -->
	{#if neboLoopConnected}
		<div>
			<h3 class="text-sm font-semibold uppercase tracking-wider text-base-content/40 mb-4">Skill Store</h3>

			{#if isLoadingStore}
				<Card>
					<div class="py-8 text-center text-base-content/60">
						<span class="loading loading-spinner loading-md"></span>
						<p class="mt-2">Loading store...</p>
					</div>
				</Card>
			{:else if storeSkills.length > 0}
				<div class="grid sm:grid-cols-2 lg:grid-cols-3 gap-4">
					{#each storeSkills as skill}
						<Card class="hover:border-primary/30 transition-colors">
							<div class="flex items-start gap-3">
								<div class="w-10 h-10 rounded-xl bg-base-200 flex items-center justify-center shrink-0">
									{#if skill.icon}
										<img src={skill.icon} alt={skill.name} class="w-8 h-8 rounded" />
									{:else}
										<Store class="w-5 h-5 text-base-content/40" />
									{/if}
								</div>
								<div class="flex-1 min-w-0">
									<h3 class="font-display font-bold text-base-content mb-0.5">{skill.name}</h3>
									<p class="text-xs text-base-content/50 mb-1">
										by {skill.author.name}
										{#if skill.author.verified}
											<Check class="w-3 h-3 inline text-success" />
										{/if}
									</p>
									<p class="text-sm text-base-content/60 mb-3 line-clamp-2">{skill.description}</p>

									<div class="flex items-center justify-between">
										<div class="flex items-center gap-3 text-xs text-base-content/40">
											{#if skill.rating > 0}
												<span class="flex items-center gap-1">
													<Star class="w-3 h-3" />
													{skill.rating.toFixed(1)}
												</span>
											{/if}
											{#if skill.installCount > 0}
												<span class="flex items-center gap-1">
													<Download class="w-3 h-3" />
													{skill.installCount}
												</span>
											{/if}
										</div>

										{#if skill.isInstalled}
											<button
												class="btn btn-xs btn-ghost text-success"
												onclick={() => handleUninstall(skill)}
												disabled={installingSkill === skill.id}
											>
												{#if installingSkill === skill.id}
													<span class="loading loading-spinner loading-xs"></span>
												{:else}
													<Check class="w-3 h-3" />
													Installed
												{/if}
											</button>
										{:else}
											<button
												class="btn btn-xs btn-primary"
												onclick={() => handleInstall(skill)}
												disabled={installingSkill === skill.id}
											>
												{#if installingSkill === skill.id}
													<span class="loading loading-spinner loading-xs"></span>
												{:else}
													Install
												{/if}
											</button>
										{/if}
									</div>
								</div>
							</div>
						</Card>
					{/each}
				</div>
			{:else}
				<Card>
					<div class="py-8 text-center text-base-content/60">
						<Store class="w-10 h-10 mx-auto mb-3 opacity-20" />
						<p class="font-medium mb-1">No skills available yet</p>
						<p class="text-sm">Check back later for new skills.</p>
					</div>
				</Card>
			{/if}
		</div>
	{/if}
{/if}

<SkillEditorModal
	bind:show={showEditor}
	skill={editingSkill}
	onclose={() => { showEditor = false; }}
	onsaved={() => loadAll()}
/>
