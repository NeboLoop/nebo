<script lang="ts">
	import { onMount } from 'svelte';
	import Modal from '$lib/components/ui/Modal.svelte';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import SkillEditorModal from '$lib/components/skills/SkillEditorModal.svelte';
	import {
		Zap, RefreshCw, Power, Store, Download, Check, Star, Plus, Pencil, Trash2,
		Wrench, Tag, FileText, Hash, FolderOpen, Loader2, Key
	} from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import type { ExtensionSkill, SkillItem } from '$lib/api/nebo';

	let skills = $state<ExtensionSkill[]>([]);
	let storeSkills = $state<SkillItem[]>([]);
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

	// Secrets state
	interface SecretInfo { key: string; label: string; hint: string; required: boolean; configured: boolean }
	let skillSecrets = $state<SecretInfo[]>([]);
	let settingSecret = $state<string | null>(null);
	let secretInputs = $state<Record<string, string>>({});

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

	async function openDetail(skill: ExtensionSkill) {
		selectedSkill = skill;
		skillSecrets = [];
		secretInputs = {};
		showDetail = true;
		// Load secrets if the skill declares any
		try {
			const resp = await api.listSkillSecrets(skill.name);
			skillSecrets = resp.secrets || [];
		} catch { /* skill may not have secrets */ }
	}

	async function saveSecret(skillName: string, key: string) {
		const value = secretInputs[key];
		if (!value) return;
		settingSecret = key;
		try {
			await api.setSkillSecret(skillName, key, value);
			secretInputs[key] = '';
			// Reload secrets status
			const resp = await api.listSkillSecrets(skillName);
			skillSecrets = resp.secrets || [];
		} catch (err: any) {
			console.error('Failed to save secret:', err);
		} finally {
			settingSecret = null;
		}
	}

	async function removeSecret(skillName: string, key: string) {
		settingSecret = key;
		try {
			await api.deleteSkillSecret(skillName, key);
			const resp = await api.listSkillSecrets(skillName);
			skillSecrets = resp.secrets || [];
		} catch (err: any) {
			console.error('Failed to delete secret:', err);
		} finally {
			settingSecret = null;
		}
	}

	async function handleInstall(skill: SkillItem) {
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

	async function handleUninstall(skill: SkillItem) {
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
		<p class="text-base text-base-content/80">Standalone orchestration skills for the agent</p>
	</div>
	<div class="flex items-center gap-2">
		<button
			type="button"
			class="h-9 px-4 rounded-full bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all flex items-center gap-1.5"
			onclick={openCreate}
		>
			<Plus class="w-4 h-4" />
			Create Skill
		</button>
		<button
			type="button"
			class="h-8 px-3 rounded-lg bg-base-content/5 border border-base-content/10 text-sm font-medium text-base-content/60 hover:border-base-content/40 hover:text-base-content transition-colors flex items-center gap-1.5"
			onclick={loadAll}
		>
			<RefreshCw class="w-3.5 h-3.5" />
		</button>
	</div>
</div>

{#if isLoading}
	<div class="rounded-2xl bg-base-200/50 border border-base-content/10 py-12 text-center text-base-content/90">
		<Spinner class="w-5 h-5 mx-auto mb-2" />
		<p class="text-base">Loading skills...</p>
	</div>
{:else}
	<!-- Installed Skills -->
	<div class="mb-8">
		<h3 class="text-base font-semibold uppercase tracking-wider text-base-content/60 mb-4">Installed Skills</h3>

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
								<Zap class="w-4.5 h-4.5 {skill.enabled ? 'text-primary' : 'text-base-content/90'}" />
							</div>
							<div class="flex-1 min-w-0">
								<div class="flex items-center gap-2">
									<span class="font-display font-bold text-base text-base-content truncate">{skill.name}</span>
									<span class="text-sm text-base-content/60 tabular-nums">v{skill.version}</span>
								</div>
							</div>
							{#if skill.source === 'bundled'}
								<span class="text-sm font-medium uppercase tracking-wide text-base-content/60">Bundled</span>
							{/if}
						</div>
						<p class="text-base text-base-content/80 line-clamp-2 leading-relaxed">{skill.description}</p>
						{#if (skill as any).needsConfiguration}
							<div class="flex items-center gap-1.5 mt-2">
								<span class="w-2 h-2 rounded-full bg-warning"></span>
								<span class="text-xs text-warning font-medium">Needs configuration</span>
							</div>
						{/if}
					</button>
				{/each}
			</div>
		{:else}
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 py-12 text-center text-base-content/90">
				<Zap class="w-12 h-12 mx-auto mb-4 opacity-20" />
				<p class="font-medium mb-2">No skills found</p>
				<p class="text-base">Create a skill or browse the store.</p>
			</div>
		{/if}
	</div>

	<!-- Skill Store -->
	{#if neboLoopConnected}
		<div>
			<h3 class="text-base font-semibold uppercase tracking-wider text-base-content/60 mb-4">Skill Store</h3>

			{#if isLoadingStore}
				<div class="rounded-2xl bg-base-200/50 border border-base-content/10 py-8 text-center text-base-content/90">
					<Spinner class="w-5 h-5 mx-auto mb-2" />
					<p class="text-base">Loading store...</p>
				</div>
			{:else if storeSkills.length > 0}
				<div class="grid sm:grid-cols-2 lg:grid-cols-3 gap-3">
					{#each storeSkills as skill}
						<div class="rounded-xl bg-base-100 p-4 shadow-sm ring-1 ring-base-content/5 transition-all hover:shadow-md hover:ring-primary/20">
							<div class="flex items-center gap-3 mb-2">
								<div class="w-9 h-9 rounded-lg bg-base-200 flex items-center justify-center shrink-0 overflow-hidden">
									{#if skill.icon}
										<img src={skill.icon} alt={skill.name} class="w-9 h-9 rounded-lg object-cover" />
									{:else}
										<Store class="w-4.5 h-4.5 text-base-content/90" />
									{/if}
								</div>
								<div class="flex-1 min-w-0">
									<span class="font-display font-bold text-base text-base-content truncate block">{skill.name}</span>
									<span class="text-sm text-base-content/60">
										by {skill.author.name}
										{#if skill.author.verified}
											<Check class="w-2.5 h-2.5 inline text-success" />
										{/if}
									</span>
								</div>
							</div>
							<p class="text-base text-base-content/80 line-clamp-2 leading-relaxed mb-3">{skill.description}</p>
							<div class="flex items-center justify-between">
								<div class="flex items-center gap-3 text-sm text-base-content/60">
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
										class="h-7 px-2.5 rounded-md bg-success/10 text-success text-sm font-semibold flex items-center gap-1 hover:bg-success/20 transition-colors disabled:opacity-50"
										onclick={() => handleUninstall(skill)}
										disabled={installingSkill === skill.id}
									>
										{#if installingSkill === skill.id}
											<Loader2 class="w-3 h-3 animate-spin" />
										{:else}
											<Check class="w-3 h-3" />
											Installed
										{/if}
									</button>
								{:else}
									<button
										class="h-7 px-2.5 rounded-md bg-primary text-primary-content text-sm font-semibold flex items-center gap-1 hover:brightness-110 transition-all disabled:opacity-50"
										onclick={() => handleInstall(skill)}
										disabled={installingSkill === skill.id}
									>
										{#if installingSkill === skill.id}
											<Loader2 class="w-3 h-3 animate-spin" />
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
				<div class="rounded-2xl bg-base-200/50 border border-base-content/10 py-8 text-center text-base-content/90">
					<Store class="w-10 h-10 mx-auto mb-3 opacity-20" />
					<p class="font-medium mb-1">No skills available yet</p>
					<p class="text-base">Check back later for new skills.</p>
				</div>
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
					<Zap class="w-7 h-7 {selectedSkill.enabled ? 'text-primary' : 'text-base-content/90'}" />
				</div>
				<div class="flex-1 min-w-0">
					<div class="flex items-center gap-2 mb-1">
						<span class="text-base text-base-content/80 tabular-nums">v{selectedSkill.version}</span>
						{#if selectedSkill.source === 'bundled'}
							<span class="text-sm font-semibold uppercase px-1.5 py-0.5 rounded bg-base-content/10 text-base-content/60">Bundled</span>
						{/if}
						<span class="text-sm font-semibold uppercase px-1.5 py-0.5 rounded {selectedSkill.enabled ? 'bg-success/10 text-success' : 'bg-base-content/10 text-base-content/60'}">
							{selectedSkill.enabled ? 'Enabled' : 'Disabled'}
						</span>
					</div>
					<p class="text-base text-base-content/80 leading-relaxed">{selectedSkill.description}</p>
				</div>
			</div>

			<!-- Metadata rows -->
			<div class="divide-y divide-base-200 rounded-xl bg-base-200/30 overflow-hidden">
				{#if selectedSkill.tools && selectedSkill.tools.length > 0}
					<div class="flex items-center gap-3 px-4 py-3">
						<Wrench class="w-4 h-4 text-base-content/90 shrink-0" />
						<span class="text-base font-medium text-base-content/80 w-16 shrink-0">Tools</span>
						<div class="flex flex-wrap gap-1.5">
							{#each selectedSkill.tools as tool}
								<span class="text-sm font-medium px-2 py-0.5 rounded-md bg-base-content/5 border border-base-content/10 text-base-content/60">{tool}</span>
							{/each}
						</div>
					</div>
				{/if}

				{#if selectedSkill.tags && selectedSkill.tags.length > 0}
					<div class="flex items-center gap-3 px-4 py-3">
						<Tag class="w-4 h-4 text-base-content/90 shrink-0" />
						<span class="text-base font-medium text-base-content/80 w-16 shrink-0">Tags</span>
						<div class="flex flex-wrap gap-1.5">
							{#each selectedSkill.tags as tag}
								<span class="text-sm font-medium px-2 py-0.5 rounded-md bg-base-content/5 text-base-content/60">{tag}</span>
							{/each}
						</div>
					</div>
				{/if}

				{#if selectedSkill.dependencies && selectedSkill.dependencies.length > 0}
					<div class="flex items-center gap-3 px-4 py-3">
						<FileText class="w-4 h-4 text-base-content/90 shrink-0" />
						<span class="text-base font-medium text-base-content/80 w-16 shrink-0">Deps</span>
						<div class="flex flex-wrap gap-1.5">
							{#each selectedSkill.dependencies as dep}
								<span class="text-sm font-medium px-2 py-0.5 rounded-md bg-base-content/5 border border-base-content/10 text-base-content/60">{dep}</span>
							{/each}
						</div>
					</div>
				{/if}

				<div class="flex items-center gap-3 px-4 py-3">
					<Hash class="w-4 h-4 text-base-content/90 shrink-0" />
					<span class="text-base font-medium text-base-content/80 w-16 shrink-0">Priority</span>
					<span class="text-base text-base-content/80">{selectedSkill.priority}</span>
				</div>

				<div class="flex items-center gap-3 px-4 py-3">
					<FolderOpen class="w-4 h-4 text-base-content/90 shrink-0" />
					<span class="text-base font-medium text-base-content/80 w-16 shrink-0">Source</span>
					<span class="text-base text-base-content/80 truncate">{selectedSkill.filePath || selectedSkill.source}</span>
				</div>
			</div>
			<!-- Secrets Configuration -->
			{#if skillSecrets.length > 0}
				<div class="mt-5">
					<h4 class="text-sm font-semibold text-base-content/60 uppercase tracking-wider mb-3">Configuration</h4>
					<div class="space-y-3">
						{#each skillSecrets as secret (secret.key)}
							<div class="rounded-xl bg-base-content/5 border border-base-content/10 p-3">
								<div class="flex items-center justify-between mb-1">
									<div class="flex items-center gap-2">
										<span class="text-sm font-medium text-base-content">{secret.label || secret.key}</span>
										{#if secret.required}
											<span class="text-xs text-error/80">required</span>
										{/if}
									</div>
									{#if secret.configured}
										<div class="flex items-center gap-2">
											<span class="text-xs font-medium text-success">Configured</span>
											<button
												type="button"
												class="text-xs text-base-content/40 hover:text-error transition-colors"
												onclick={() => selectedSkill && removeSecret(selectedSkill.name, secret.key)}
												disabled={settingSecret === secret.key}
											>
												Remove
											</button>
										</div>
									{:else}
										<span class="text-xs font-medium text-warning">Not set</span>
									{/if}
								</div>
								{#if secret.hint}
									<p class="text-xs text-base-content/50 mb-2">{secret.hint}</p>
								{/if}
								{#if !secret.configured}
									<div class="flex gap-2 mt-2">
										<input
											type="password"
											placeholder={secret.key}
											bind:value={secretInputs[secret.key]}
											class="flex-1 h-8 rounded-lg bg-base-content/5 border border-base-content/10 px-3 text-sm focus:outline-none focus:border-primary/50 transition-colors"
										/>
										<button
											type="button"
											class="h-8 px-3 rounded-lg bg-primary text-primary-content text-sm font-bold hover:brightness-110 transition-all disabled:opacity-50"
											onclick={() => selectedSkill && saveSecret(selectedSkill.name, secret.key)}
											disabled={settingSecret === secret.key || !secretInputs[secret.key]}
										>
											{settingSecret === secret.key ? '...' : 'Save'}
										</button>
									</div>
								{/if}
							</div>
						{/each}
					</div>
				</div>
			{/if}
		</div>

		{#snippet footer()}
			<div class="flex items-center justify-between w-full">
				<div class="flex items-center gap-2">
					{#if selectedSkill.editable}
						<button
							class="h-8 px-3 rounded-lg bg-base-content/5 border border-base-content/10 text-sm font-medium text-base-content/60 hover:border-primary/30 hover:text-primary transition-colors flex items-center gap-1.5"
							onclick={() => openEdit(selectedSkill)}
						>
							<Pencil class="w-3.5 h-3.5" />
							Edit
						</button>
						<button
							class="h-8 px-3 rounded-lg bg-base-content/5 border border-base-content/10 text-sm font-medium text-base-content/60 hover:border-error/30 hover:text-error transition-colors flex items-center gap-1.5 disabled:opacity-50"
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
					class="h-8 px-4 rounded-lg text-sm font-bold flex items-center gap-1.5 transition-all disabled:opacity-50 {selectedSkill.enabled ? 'bg-success/10 border border-success/20 text-success hover:bg-success/20' : 'bg-primary text-primary-content hover:brightness-110'}"
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
