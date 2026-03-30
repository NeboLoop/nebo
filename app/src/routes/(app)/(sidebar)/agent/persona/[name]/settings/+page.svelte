<script lang="ts">
	import { onMount, getContext } from 'svelte';
	import { goto } from '$app/navigation';
	import { getActiveAgents, getAgent, updateAgent, activateAgent, deactivateAgent, deleteAgent, reloadAgent, checkAgentUpdate, applyAgentUpdate } from '$lib/api/nebo';
	import AlertDialog from '$lib/components/ui/AlertDialog.svelte';
	import { t } from 'svelte-i18n';

	const channelState = getContext<{
		activeAgentId: string;
		activeAgentName: string;
	}>('channelState');

	let isActive = $state(true);
	let pausing = $state(false);
	let resuming = $state(false);
	let deleting = $state(false);
	let loading = $state(true);
	let showDeleteDialog = $state(false);

	let nameValue = $state('');
	let descriptionValue = $state('');
	let originalName = $state('');
	let originalDescription = $state('');
	let saving = $state(false);

	// Version & update
	let version = $state<string | null>(null);
	let isMarketplace = $state(false);
	let checkingUpdate = $state(false);
	let updateAvailable = $state(false);
	let remoteVersion = $state('');
	let applyingUpdate = $state(false);
	let reloading = $state(false);

	const hasChanges = $derived(
		nameValue.trim() !== originalName || descriptionValue.trim() !== originalDescription
	);

	async function load() {
		loading = true;
		try {
			const [activeRes, agentRes] = await Promise.all([
				getActiveAgents(),
				getAgent(channelState.activeAgentId).catch(() => null),
			]);
			isActive = activeRes.agents?.some(r => r.agentId === channelState.activeAgentId) ?? false;

			if (agentRes?.agent) {
				nameValue = agentRes.agent.name || '';
				descriptionValue = agentRes.agent.description || '';
				originalName = nameValue;
				originalDescription = descriptionValue;
				isMarketplace = !!agentRes.agent.kind;
				version = (agentRes as any).version || null;
			}
		} catch {
			// ignore
		} finally {
			loading = false;
		}
	}

	async function handleSave() {
		saving = true;
		try {
			const data: { name?: string; description?: string } = {};
			if (nameValue.trim() !== originalName) data.name = nameValue.trim();
			if (descriptionValue.trim() !== originalDescription) data.description = descriptionValue.trim();
			await updateAgent(channelState.activeAgentId, data);
			if (data.name) {
				channelState.activeAgentName = data.name;
			}
			originalName = nameValue.trim();
			originalDescription = descriptionValue.trim();
		} finally {
			saving = false;
		}
	}

	async function handleReload() {
		reloading = true;
		try {
			await reloadAgent(channelState.activeAgentId);
			await load();
		} finally {
			reloading = false;
		}
	}

	async function handleCheckUpdate() {
		checkingUpdate = true;
		try {
			const res = await checkAgentUpdate(channelState.activeAgentId);
			updateAvailable = res.hasUpdate;
			remoteVersion = res.remoteVersion || '';
		} finally {
			checkingUpdate = false;
		}
	}

	async function handleApplyUpdate() {
		applyingUpdate = true;
		try {
			await applyAgentUpdate(channelState.activeAgentId);
			updateAvailable = false;
			await load();
		} finally {
			applyingUpdate = false;
		}
	}

	async function handlePause() {
		pausing = true;
		try {
			await deactivateAgent(channelState.activeAgentId);
			isActive = false;
		} finally {
			pausing = false;
		}
	}

	async function handleResume() {
		resuming = true;
		try {
			await activateAgent(channelState.activeAgentId);
			isActive = true;
		} finally {
			resuming = false;
		}
	}

	async function handleDelete() {
		deleting = true;
		showDeleteDialog = false;
		try {
			if (isActive) await deactivateAgent(channelState.activeAgentId);
			await deleteAgent(channelState.activeAgentId);
			goto('/agents');
		} finally {
			deleting = false;
		}
	}

	onMount(() => load());
</script>

<svelte:head>
	<title>Nebo - {channelState.activeAgentName || $t('agent.settingsTab')} - {$t('agent.settingsTab')}</title>
</svelte:head>

<div class="flex-1 flex flex-col min-h-0">
	<div class="flex-1 overflow-y-auto">
		<div class="max-w-3xl mx-auto px-6 py-6">
		{#if loading}
			<div class="flex items-center justify-center py-8">
				<div class="loading loading-spinner loading-md"></div>
			</div>
		{:else}
			<div class="flex items-center justify-between mb-3"><h2 class="text-xs text-base-content/80 uppercase tracking-wider font-semibold">{$t('agentSettings.general')}</h2><span class="btn btn-sm invisible">&#8203;</span></div>

			<!-- Name -->
			<div class="py-4 border-b border-base-content/10">
				<label class="block text-sm font-medium mb-1" for="role-name">{$t('agentSettings.nameLabel')}</label>
				<input
					id="role-name"
					class="input input-bordered w-full max-w-md"
					bind:value={nameValue}
					placeholder={$t('agentSettings.namePlaceholder')}
				/>
			</div>

			<!-- Description -->
			<div class="py-4 border-b border-base-content/10">
				<label class="block text-sm font-medium mb-1" for="role-desc">{$t('agentSettings.descriptionLabel')}</label>
				<input
					id="role-desc"
					class="input input-bordered w-full max-w-md"
					bind:value={descriptionValue}
					placeholder={$t('agentSettings.descriptionPlaceholder')}
				/>
			</div>

			{#if hasChanges}
				<div class="py-3">
					<button
						class="btn btn-sm btn-primary"
						class:opacity-50={saving}
						disabled={saving || !nameValue.trim()}
						onclick={handleSave}
					>
						{saving ? $t('common.saving') : $t('common.save')}
					</button>
				</div>
			{/if}

			<!-- Version & Updates -->
			<div class="py-4 border-t border-base-content/10 mt-4">
				<div class="flex items-center justify-between mb-3"><h2 class="text-xs text-base-content/80 uppercase tracking-wider font-semibold">{$t('agentSettings.versionSection')}</h2><span class="btn btn-sm invisible">&#8203;</span></div>
				<div class="flex items-center justify-between">
					<div>
						<p class="text-sm text-base-content/80">
							{version || $t('agentSettings.local')}{isMarketplace ? ' ' + $t('agentSettings.marketplaceCreated') : ' ' + $t('agentSettings.userCreated')}
						</p>
					</div>
					<div class="flex items-center gap-2">
						<button
							class="btn btn-sm btn-ghost gap-1.5"
							disabled={reloading}
							onclick={handleReload}
						>
							<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
								<path d="M21.5 2v6h-6M2.5 22v-6h6M2 11.5a10 10 0 0 1 18.8-4.3M22 12.5a10 10 0 0 1-18.8 4.2" />
							</svg>
							{reloading ? $t('agentSettings.reloading') : $t('agentSettings.reload')}
						</button>
						{#if isMarketplace}
							{#if updateAvailable}
								<button
									class="btn btn-sm btn-primary gap-1.5"
									disabled={applyingUpdate}
									onclick={handleApplyUpdate}
								>
									{applyingUpdate ? $t('agentSettings.updating') : $t('agentSettings.updateTo', { values: { version: remoteVersion } })}
								</button>
							{:else}
								<button
									class="btn btn-sm btn-ghost gap-1.5"
									disabled={checkingUpdate}
									onclick={handleCheckUpdate}
								>
									{checkingUpdate ? $t('agentSettings.checking') : $t('agentSettings.checkForUpdates')}
								</button>
							{/if}
						{/if}
					</div>
				</div>
			</div>

			<!-- Status -->
			<div class="py-4 border-t border-base-content/10 mt-4">
				<div class="flex items-center justify-between mb-3"><h2 class="text-xs text-base-content/80 uppercase tracking-wider font-semibold">{$t('agentSettings.statusSection')}</h2><span class="btn btn-sm invisible">&#8203;</span></div>
				<div class="flex items-center justify-between">
					<p class="text-sm text-base-content/70">
						{isActive ? $t('agentSettings.agentActive') : $t('agentSettings.agentPaused')}
					</p>
				{#if isActive}
					<button
						class="btn btn-sm btn-ghost gap-1.5"
						class:opacity-50={pausing}
						disabled={pausing}
						onclick={handlePause}
					>
						<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
							<rect x="6" y="4" width="4" height="16" /><rect x="14" y="4" width="4" height="16" />
						</svg>
						{pausing ? $t('agentSettings.pausing') : $t('sidebar.pause')}
					</button>
				{:else}
					<button
						class="btn btn-sm btn-primary gap-1.5"
						class:opacity-50={resuming}
						disabled={resuming}
						onclick={handleResume}
					>
						<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
							<polygon points="5 3 19 12 5 21 5 3" />
						</svg>
						{resuming ? $t('agentSettings.resuming') : $t('sidebar.resume')}
					</button>
				{/if}
				</div>
			</div>

			<!-- Danger zone -->
			<div class="py-4 border-t border-base-content/10 mt-4">
				<div class="flex items-center justify-between mb-3"><h2 class="text-xs text-error/80 uppercase tracking-wider font-semibold">{$t('agentSettings.dangerZone')}</h2><span class="btn btn-sm invisible">&#8203;</span></div>
				<div class="flex items-center justify-between">
					<p class="text-sm text-base-content/70">{$t('agentSettings.dangerDesc')}</p>
				<button
					class="btn btn-sm btn-error btn-outline gap-1.5"
					class:opacity-50={deleting}
					disabled={deleting}
					onclick={() => showDeleteDialog = true}
				>
					{deleting ? $t('agentSettings.deleting') : $t('common.delete')}
				</button>
				</div>
			</div>
		{/if}
		</div>
	</div>
</div>

<AlertDialog
	bind:open={showDeleteDialog}
	title={$t('agentSettings.deleteAgent')}
	description={$t('agentSettings.deleteConfirm', { values: { name: channelState.activeAgentName } })}
	actionLabel={$t('common.delete')}
	actionType="danger"
	onAction={handleDelete}
/>
