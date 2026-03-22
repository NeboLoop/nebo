<script lang="ts">
	import { onMount, getContext } from 'svelte';
	import { goto } from '$app/navigation';
	import { getActiveRoles, getRole, updateRole, activateRole, deactivateRole, deleteRole, reloadRole, checkRoleUpdate, applyRoleUpdate } from '$lib/api/nebo';
	import AlertDialog from '$lib/components/ui/AlertDialog.svelte';

	const channelState = getContext<{
		activeRoleId: string;
		activeRoleName: string;
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
			const [activeRes, roleRes] = await Promise.all([
				getActiveRoles(),
				getRole(channelState.activeRoleId).catch(() => null),
			]);
			isActive = activeRes.roles?.some(r => r.roleId === channelState.activeRoleId) ?? false;

			if (roleRes?.role) {
				nameValue = roleRes.role.name || '';
				descriptionValue = roleRes.role.description || '';
				originalName = nameValue;
				originalDescription = descriptionValue;
				isMarketplace = !!roleRes.role.kind;
				version = (roleRes as any).version || null;
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
			await updateRole(channelState.activeRoleId, data);
			if (data.name) {
				channelState.activeRoleName = data.name;
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
			await reloadRole(channelState.activeRoleId);
			await load();
		} finally {
			reloading = false;
		}
	}

	async function handleCheckUpdate() {
		checkingUpdate = true;
		try {
			const res = await checkRoleUpdate(channelState.activeRoleId);
			updateAvailable = res.hasUpdate;
			remoteVersion = res.remoteVersion || '';
		} finally {
			checkingUpdate = false;
		}
	}

	async function handleApplyUpdate() {
		applyingUpdate = true;
		try {
			await applyRoleUpdate(channelState.activeRoleId);
			updateAvailable = false;
			await load();
		} finally {
			applyingUpdate = false;
		}
	}

	async function handlePause() {
		pausing = true;
		try {
			await deactivateRole(channelState.activeRoleId);
			isActive = false;
		} finally {
			pausing = false;
		}
	}

	async function handleResume() {
		resuming = true;
		try {
			await activateRole(channelState.activeRoleId);
			isActive = true;
		} finally {
			resuming = false;
		}
	}

	async function handleDelete() {
		deleting = true;
		showDeleteDialog = false;
		try {
			if (isActive) await deactivateRole(channelState.activeRoleId);
			await deleteRole(channelState.activeRoleId);
			goto('/agents');
		} finally {
			deleting = false;
		}
	}

	onMount(() => load());
</script>

<svelte:head>
	<title>Nebo - {channelState.activeRoleName || 'Settings'} - Settings</title>
</svelte:head>

<div class="flex-1 flex flex-col min-h-0">
	<div class="flex-1 overflow-y-auto">
		<div class="max-w-3xl mx-auto px-6 py-6">
		{#if loading}
			<div class="flex items-center justify-center py-8">
				<div class="loading loading-spinner loading-md"></div>
			</div>
		{:else}
			<div class="flex items-center justify-between mb-3"><h2 class="text-xs text-base-content/80 uppercase tracking-wider font-semibold">General</h2><span class="btn btn-sm invisible">&#8203;</span></div>

			<!-- Name -->
			<div class="py-4 border-b border-base-content/10">
				<label class="block text-sm font-medium mb-1" for="role-name">Name</label>
				<input
					id="role-name"
					class="input input-bordered w-full max-w-md"
					bind:value={nameValue}
					placeholder="Agent name"
				/>
			</div>

			<!-- Description -->
			<div class="py-4 border-b border-base-content/10">
				<label class="block text-sm font-medium mb-1" for="role-desc">Description</label>
				<input
					id="role-desc"
					class="input input-bordered w-full max-w-md"
					bind:value={descriptionValue}
					placeholder="Short description (optional)"
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
						{saving ? 'Saving...' : 'Save'}
					</button>
				</div>
			{/if}

			<!-- Version & Updates -->
			<div class="py-4 border-t border-base-content/10 mt-4">
				<div class="flex items-center justify-between mb-3"><h2 class="text-xs text-base-content/80 uppercase tracking-wider font-semibold">Version</h2><span class="btn btn-sm invisible">&#8203;</span></div>
				<div class="flex items-center justify-between">
					<div>
						<p class="text-sm text-base-content/80">
							{version || 'Local'}{isMarketplace ? ' (marketplace)' : ' (user-created)'}
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
							{reloading ? 'Reloading...' : 'Reload'}
						</button>
						{#if isMarketplace}
							{#if updateAvailable}
								<button
									class="btn btn-sm btn-primary gap-1.5"
									disabled={applyingUpdate}
									onclick={handleApplyUpdate}
								>
									{applyingUpdate ? 'Updating...' : `Update to ${remoteVersion}`}
								</button>
							{:else}
								<button
									class="btn btn-sm btn-ghost gap-1.5"
									disabled={checkingUpdate}
									onclick={handleCheckUpdate}
								>
									{checkingUpdate ? 'Checking...' : 'Check for updates'}
								</button>
							{/if}
						{/if}
					</div>
				</div>
			</div>

			<!-- Status -->
			<div class="py-4 border-t border-base-content/10 mt-4">
				<div class="flex items-center justify-between mb-3"><h2 class="text-xs text-base-content/80 uppercase tracking-wider font-semibold">Status</h2><span class="btn btn-sm invisible">&#8203;</span></div>
				<div class="flex items-center justify-between">
					<p class="text-sm text-base-content/70">
						{isActive ? 'This agent is active and running.' : 'This agent is paused.'}
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
						{pausing ? 'Pausing...' : 'Pause'}
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
						{resuming ? 'Resuming...' : 'Resume'}
					</button>
				{/if}
				</div>
			</div>

			<!-- Danger zone -->
			<div class="py-4 border-t border-base-content/10 mt-4">
				<div class="flex items-center justify-between mb-3"><h2 class="text-xs text-error/80 uppercase tracking-wider font-semibold">Danger zone</h2><span class="btn btn-sm invisible">&#8203;</span></div>
				<div class="flex items-center justify-between">
					<p class="text-sm text-base-content/70">Permanently remove this agent and all its data.</p>
				<button
					class="btn btn-sm btn-error btn-outline gap-1.5"
					class:opacity-50={deleting}
					disabled={deleting}
					onclick={() => showDeleteDialog = true}
				>
					{deleting ? 'Deleting...' : 'Delete'}
				</button>
				</div>
			</div>
		{/if}
		</div>
	</div>
</div>

<AlertDialog
	bind:open={showDeleteDialog}
	title="Delete Agent"
	description="Are you sure you want to delete &quot;{channelState.activeRoleName}&quot;? This will remove the agent and all its data permanently."
	actionLabel="Delete"
	actionType="danger"
	onAction={handleDelete}
/>
