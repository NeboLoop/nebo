<script lang="ts">
	import { onMount } from 'svelte';
	import Card from '$lib/components/ui/Card.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import Modal from '$lib/components/ui/Modal.svelte';
	import SkillEditorModal from '$lib/components/skills/SkillEditorModal.svelte';
	import {
		Zap, RefreshCw, Power, Store, Download, Check, Star, Plus, Pencil, Trash2,
		Wrench, Tag, FileText, Hash, FolderOpen, Loader2
	} from 'lucide-svelte';
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

	let selectedSkill = $state<ExtensionSkill | null>(null);
	let showDetail = $state(false);

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
			// Update the selected skill in the detail modal if open
			if (selectedSkill?.name === name) {
				selectedSkill = skills.find(s => s.name === name) || null;
			}
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
			showDetail = false;
			selectedSkill = null;
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
		showDetail = false;
		editingSkill = skill;
		showEditor = true;
	}

	function openDetail(skill: ExtensionSkill) {
		selectedSkill = skill;
		showDetail = true;
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
	<!-- Installed Skills -->
	<div class="mb-8">
		<h3 class="text-sm font-semibold uppercase tracking-wider text-base-content/40 mb-4">Installed Skills</h3>

		{#if skills.length > 0}
			<div class="grid sm:grid-cols-2 lg:grid-cols-3 gap-3">
				{#each skills as skill}
					<button
						type="button"
						class="group text-left rounded-xl bg-base-100 p-4 shadow-sm ring-1 ring-base-content/5 transition-all hover:shadow-md hover:ring-primary/20 {!skill.enabled ? 'opacity-60' : ''}"
						onclick={() => openDetail(skill)}
					>
						<div class="flex items-center gap-3 mb-2">
							<div class="w-9 h-9 rounded-lg {skill.enabled ? 'bg-primary/10' : 'bg-base-200'} flex items-center justify-center shrink-0">
								<Zap class="w-4.5 h-4.5 {skill.enabled ? 'text-primary' : 'text-base-content/30'}" />
							</div>
							<div class="flex-1 min-w-0">
								<div class="flex items-center gap-2">
									<span class="font-display font-bold text-sm text-base-content truncate">{skill.name}</span>
									<span class="text-[10px] text-base-content/40 tabular-nums">v{skill.version}</span>
								</div>
							</div>
							{#if skill.source === 'bundled'}
								<span class="text-[10px] font-medium uppercase tracking-wide text-base-content/30">Bundled</span>
							{/if}
						</div>
						<p class="text-xs text-base-content/50 line-clamp-2 leading-relaxed">{skill.description}</p>
					</button>
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
				<div class="grid sm:grid-cols-2 lg:grid-cols-3 gap-3">
					{#each storeSkills as skill}
						<div class="rounded-xl bg-base-100 p-4 shadow-sm ring-1 ring-base-content/5 transition-all hover:shadow-md hover:ring-primary/20">
							<div class="flex items-center gap-3 mb-2">
								<div class="w-9 h-9 rounded-lg bg-base-200 flex items-center justify-center shrink-0 overflow-hidden">
									{#if skill.icon}
										<img src={skill.icon} alt={skill.name} class="w-9 h-9 rounded-lg object-cover" />
									{:else}
										<Store class="w-4.5 h-4.5 text-base-content/40" />
									{/if}
								</div>
								<div class="flex-1 min-w-0">
									<span class="font-display font-bold text-sm text-base-content truncate block">{skill.name}</span>
									<span class="text-[10px] text-base-content/40">
										by {skill.author.name}
										{#if skill.author.verified}
											<Check class="w-2.5 h-2.5 inline text-success" />
										{/if}
									</span>
								</div>
							</div>
							<p class="text-xs text-base-content/50 line-clamp-2 leading-relaxed mb-3">{skill.description}</p>
							<div class="flex items-center justify-between">
								<div class="flex items-center gap-3 text-[10px] text-base-content/30">
									{#if skill.rating > 0}
										<span class="flex items-center gap-0.5">
											<Star class="w-3 h-3" />
											{skill.rating.toFixed(1)}
										</span>
									{/if}
									{#if skill.installCount > 0}
										<span class="flex items-center gap-0.5">
											<Download class="w-3 h-3" />
											{skill.installCount}
										</span>
									{/if}
								</div>
								{#if skill.isInstalled}
									<button
										class="btn btn-xs btn-ghost text-success gap-1"
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
										class="btn btn-xs btn-primary gap-1"
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

<!-- Skill Detail Modal -->
{#if selectedSkill}
	<Modal bind:show={showDetail} title={selectedSkill.name} size="md" onclose={() => { showDetail = false; }}>
		<div class="flex flex-col gap-5">
			<!-- Header with icon and status -->
			<div class="flex items-center gap-4">
				<div class="w-14 h-14 rounded-2xl {selectedSkill.enabled ? 'bg-primary/10' : 'bg-base-200'} flex items-center justify-center shrink-0">
					<Zap class="w-7 h-7 {selectedSkill.enabled ? 'text-primary' : 'text-base-content/30'}" />
				</div>
				<div class="flex-1 min-w-0">
					<div class="flex items-center gap-2 mb-1">
						<span class="text-xs text-base-content/40 tabular-nums">v{selectedSkill.version}</span>
						{#if selectedSkill.source === 'bundled'}
							<span class="badge badge-xs badge-ghost">Bundled</span>
						{/if}
						<span class="badge badge-xs {selectedSkill.enabled ? 'badge-success' : 'badge-ghost'}">
							{selectedSkill.enabled ? 'Enabled' : 'Disabled'}
						</span>
					</div>
					<p class="text-sm text-base-content/60 leading-relaxed">{selectedSkill.description}</p>
				</div>
			</div>

			<!-- Metadata rows -->
			<div class="divide-y divide-base-200 rounded-xl bg-base-200/30 overflow-hidden">
				{#if selectedSkill.tools && selectedSkill.tools.length > 0}
					<div class="flex items-center gap-3 px-4 py-3">
						<Wrench class="w-4 h-4 text-base-content/40 shrink-0" />
						<span class="text-xs font-medium text-base-content/50 w-16 shrink-0">Tools</span>
						<div class="flex flex-wrap gap-1.5">
							{#each selectedSkill.tools as tool}
								<span class="badge badge-sm badge-outline">{tool}</span>
							{/each}
						</div>
					</div>
				{/if}

				{#if selectedSkill.tags && selectedSkill.tags.length > 0}
					<div class="flex items-center gap-3 px-4 py-3">
						<Tag class="w-4 h-4 text-base-content/40 shrink-0" />
						<span class="text-xs font-medium text-base-content/50 w-16 shrink-0">Tags</span>
						<div class="flex flex-wrap gap-1.5">
							{#each selectedSkill.tags as tag}
								<span class="badge badge-sm badge-ghost">{tag}</span>
							{/each}
						</div>
					</div>
				{/if}

				{#if selectedSkill.dependencies && selectedSkill.dependencies.length > 0}
					<div class="flex items-center gap-3 px-4 py-3">
						<FileText class="w-4 h-4 text-base-content/40 shrink-0" />
						<span class="text-xs font-medium text-base-content/50 w-16 shrink-0">Deps</span>
						<div class="flex flex-wrap gap-1.5">
							{#each selectedSkill.dependencies as dep}
								<span class="badge badge-sm badge-outline">{dep}</span>
							{/each}
						</div>
					</div>
				{/if}

				<div class="flex items-center gap-3 px-4 py-3">
					<Hash class="w-4 h-4 text-base-content/40 shrink-0" />
					<span class="text-xs font-medium text-base-content/50 w-16 shrink-0">Priority</span>
					<span class="text-sm text-base-content/70">{selectedSkill.priority}</span>
				</div>

				<div class="flex items-center gap-3 px-4 py-3">
					<FolderOpen class="w-4 h-4 text-base-content/40 shrink-0" />
					<span class="text-xs font-medium text-base-content/50 w-16 shrink-0">Source</span>
					<span class="text-sm text-base-content/70 truncate">{selectedSkill.filePath || selectedSkill.source}</span>
				</div>
			</div>
		</div>

		{#snippet footer()}
			<div class="flex items-center justify-between w-full">
				<div class="flex items-center gap-2">
					{#if selectedSkill.editable}
						<button
							class="btn btn-sm btn-ghost gap-1.5 text-base-content/60 hover:text-primary"
							onclick={() => openEdit(selectedSkill)}
						>
							<Pencil class="w-3.5 h-3.5" />
							Edit
						</button>
						<button
							class="btn btn-sm btn-ghost gap-1.5 text-base-content/60 hover:text-error"
							onclick={() => handleDelete(selectedSkill)}
							disabled={deletingSkill === selectedSkill.name}
						>
							{#if deletingSkill === selectedSkill.name}
								<Loader2 class="w-3.5 h-3.5 animate-spin" />
							{:else}
								<Trash2 class="w-3.5 h-3.5" />
							{/if}
							Delete
						</button>
					{/if}
				</div>
				<button
					class="btn btn-sm {selectedSkill.enabled ? 'btn-outline btn-success' : 'btn-primary'} gap-1.5"
					onclick={() => handleToggle(selectedSkill.name)}
					disabled={togglingSkill === selectedSkill.name}
				>
					{#if togglingSkill === selectedSkill.name}
						<Loader2 class="w-3.5 h-3.5 animate-spin" />
					{:else}
						<Power class="w-3.5 h-3.5" />
					{/if}
					{selectedSkill.enabled ? 'Enabled' : 'Enable'}
				</button>
			</div>
		{/snippet}
	</Modal>
{/if}

<SkillEditorModal
	bind:show={showEditor}
	skill={editingSkill}
	onclose={() => { showEditor = false; }}
	onsaved={() => loadAll()}
/>
