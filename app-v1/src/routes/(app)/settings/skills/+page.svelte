<script lang="ts">
	import { onMount } from 'svelte';
	import Modal from '$lib/components/ui/Modal.svelte';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import SkillEditorModal from '$lib/components/skills/SkillEditorModal.svelte';
	import AlertDialog from '$lib/components/ui/AlertDialog.svelte';
	import {
		Zap, RefreshCw, Power, Plus, Pencil, Trash2,
		Wrench, Tag, FileText, Hash, FolderOpen, Loader2
	} from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import type { ExtensionSkill } from '$lib/api/nebo';
	import { t } from 'svelte-i18n';

	let skills = $state<ExtensionSkill[]>([]);
	let isLoading = $state(true);
	let togglingSkill = $state<string | null>(null);
	let deletingSkill = $state<string | null>(null);

	let showEditor = $state(false);
	let editingSkill = $state<ExtensionSkill | null>(null);

	let selectedSkill = $state<ExtensionSkill | null>(null);
	let showDetail = $state(false);

	// Delete confirmation dialog
	let deleteTarget = $state<ExtensionSkill | null>(null);
	let showDeleteDialog = $state(false);

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
			const extensionsResp = await api.listExtensions();
			skills = extensionsResp.skills || [];
		} catch (error) {
			console.error('Failed to load skills:', error);
		} finally {
			isLoading = false;
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

	function promptDelete(skill: ExtensionSkill) {
		deleteTarget = skill;
		showDeleteDialog = true;
	}

	async function confirmDelete() {
		if (!deleteTarget) return;
		const name = deleteTarget.name;
		showDeleteDialog = false;
		deleteTarget = null;
		deletingSkill = name;
		try {
			await api.deleteSkill(name);
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
</script>

<div class="mb-6 flex items-center justify-between">
	<div>
		<h2 class="font-display text-xl font-bold text-base-content mb-1">{$t('settingsSkills.title')}</h2>
		<p class="text-base text-base-content/80">{$t('settingsSkills.description')}</p>
	</div>
	<div class="flex items-center gap-2">
		<button
			type="button"
			class="h-9 px-4 rounded-full bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all flex items-center gap-1.5"
			onclick={openCreate}
		>
			<Plus class="w-4 h-4" />
			{$t('settingsSkills.createSkill')}
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
		<p class="text-base">{$t('settingsSkills.loadingSkills')}</p>
	</div>
{:else}
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
							<span class="text-sm font-medium uppercase tracking-wide text-base-content/60">{$t('settingsSkills.bundled')}</span>
						{/if}
					</div>
					<p class="text-base text-base-content/80 line-clamp-2 leading-relaxed">{skill.description}</p>
					{#if (skill as any).needsConfiguration}
						<div class="flex items-center gap-1.5 mt-2">
							<span class="w-2 h-2 rounded-full bg-warning"></span>
							<span class="text-xs text-warning font-medium">{$t('settingsSkills.needsConfig')}</span>
						</div>
					{/if}
				</button>
			{/each}
		</div>
	{:else}
		<div class="rounded-2xl bg-base-200/50 border border-base-content/10 py-12 text-center text-base-content/90">
			<Zap class="w-12 h-12 mx-auto mb-4 opacity-20" />
			<p class="font-medium mb-2">{$t('settingsSkills.noSkills')}</p>
			<p class="text-base">{$t('settingsSkills.noSkillsDesc')}</p>
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
							<span class="text-sm font-semibold uppercase px-1.5 py-0.5 rounded bg-base-content/10 text-base-content/60">{$t('settingsSkills.bundled')}</span>
						{/if}
						<span class="text-sm font-semibold uppercase px-1.5 py-0.5 rounded {selectedSkill.enabled ? 'bg-success/10 text-success' : 'bg-base-content/10 text-base-content/60'}">
							{selectedSkill.enabled ? $t('common.enabled') : $t('common.disabled')}
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
						<span class="text-base font-medium text-base-content/80 w-16 shrink-0">{$t('settingsSkills.tools')}</span>
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
						<span class="text-base font-medium text-base-content/80 w-16 shrink-0">{$t('settingsMemories.tags')}</span>
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
						<span class="text-base font-medium text-base-content/80 w-16 shrink-0">{$t('settingsSkills.deps')}</span>
						<div class="flex flex-wrap gap-1.5">
							{#each selectedSkill.dependencies as dep}
								<span class="text-sm font-medium px-2 py-0.5 rounded-md bg-base-content/5 border border-base-content/10 text-base-content/60">{dep}</span>
							{/each}
						</div>
					</div>
				{/if}

				<div class="flex items-center gap-3 px-4 py-3">
					<Hash class="w-4 h-4 text-base-content/90 shrink-0" />
					<span class="text-base font-medium text-base-content/80 w-16 shrink-0">{$t('settingsSkills.priority')}</span>
					<span class="text-base text-base-content/80">{selectedSkill.priority}</span>
				</div>

				<div class="flex items-center gap-3 px-4 py-3">
					<FolderOpen class="w-4 h-4 text-base-content/90 shrink-0" />
					<span class="text-base font-medium text-base-content/80 w-16 shrink-0">{$t('settingsSkills.source')}</span>
					<span class="text-base text-base-content/80 truncate">{selectedSkill.filePath || selectedSkill.source}</span>
				</div>
			</div>
			<!-- Secrets Configuration -->
			{#if skillSecrets.length > 0}
				<div class="mt-5">
					<h4 class="text-sm font-semibold text-base-content/60 uppercase tracking-wider mb-3">{$t('settingsSkills.configuration')}</h4>
					<div class="space-y-3">
						{#each skillSecrets as secret (secret.key)}
							<div class="rounded-xl bg-base-content/5 border border-base-content/10 p-3">
								<div class="flex items-center justify-between mb-1">
									<div class="flex items-center gap-2">
										<span class="text-sm font-medium text-base-content">{secret.label || secret.key}</span>
										{#if secret.required}
											<span class="text-xs text-error/80">{$t('common.required')}</span>
										{/if}
									</div>
									{#if secret.configured}
										<div class="flex items-center gap-2">
											<span class="text-xs font-medium text-success">{$t('common.configured')}</span>
											<button
												type="button"
												class="text-xs text-base-content/40 hover:text-error transition-colors"
												onclick={() => selectedSkill && removeSecret(selectedSkill.name, secret.key)}
												disabled={settingSecret === secret.key}
											>
												{$t('common.remove')}
											</button>
										</div>
									{:else}
										<span class="text-xs font-medium text-warning">{$t('common.notSet')}</span>
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
											{settingSecret === secret.key ? '...' : $t('common.save')}
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
							{$t('common.edit')}
						</button>
						<button
							class="h-8 px-3 rounded-lg bg-base-content/5 border border-base-content/10 text-sm font-medium text-base-content/60 hover:border-error/30 hover:text-error transition-colors flex items-center gap-1.5 disabled:opacity-50"
							onclick={() => promptDelete(selectedSkill)}
							disabled={deletingSkill === selectedSkill.name}
						>
							{#if deletingSkill === selectedSkill.name}
								<Loader2 class="w-3.5 h-3.5 animate-spin" />
							{:else}
								<Trash2 class="w-3.5 h-3.5" />
							{/if}
							{$t('common.delete')}
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
					{selectedSkill.enabled ? $t('common.enabled') : $t('settingsSkills.enable')}
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

<AlertDialog
	bind:open={showDeleteDialog}
	title={$t('settingsSkills.deleteTitle')}
	description={$t('settingsSkills.deleteConfirm', { values: { name: deleteTarget?.name ?? '' } })}
	actionLabel={$t('common.delete')}
	actionType="danger"
	onAction={confirmDelete}
	onclose={() => { deleteTarget = null; }}
/>
