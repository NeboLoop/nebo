<script lang="ts">
	import { onMount, getContext } from 'svelte';
	import { goto } from '$app/navigation';
	import { getActiveRoles, getRole, updateRole, activateRole, deactivateRole, deleteRole } from '$lib/api/nebo';
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

<div class="flex-1 overflow-y-auto">
	<div class="max-w-3xl mx-auto px-6 py-8">
		<h2 class="font-display text-lg font-bold mb-6">Settings</h2>

		{#if loading}
			<div class="flex items-center justify-center py-8">
				<div class="loading loading-spinner loading-md"></div>
			</div>
		{:else}
			<!-- Name -->
			<div class="py-4 border-b border-base-content/10">
				<label class="block font-medium mb-1" for="role-name">Name</label>
				<input
					id="role-name"
					class="input input-bordered w-full max-w-md"
					bind:value={nameValue}
					placeholder="Agent name"
				/>
			</div>

			<!-- Description -->
			<div class="py-4 border-b border-base-content/10">
				<label class="block font-medium mb-1" for="role-desc">Description</label>
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

			<!-- Status -->
			<div class="flex items-center justify-between py-4 border-b border-base-content/10 mt-4">
				<div>
					<p class="font-medium">Status</p>
					<p class="text-sm text-base-content/60">
						{isActive ? 'This agent is active and running.' : 'This agent is paused.'}
					</p>
				</div>
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

			<!-- Danger zone -->
			<div class="flex items-center justify-between py-4 mt-8 border-b border-error/20">
				<div>
					<p class="font-medium text-error">Delete Agent</p>
					<p class="text-sm text-base-content/60">Permanently remove this agent and all its data.</p>
				</div>
				<button
					class="btn btn-sm btn-error btn-outline gap-1.5"
					class:opacity-50={deleting}
					disabled={deleting}
					onclick={() => showDeleteDialog = true}
				>
					{deleting ? 'Deleting...' : 'Delete'}
				</button>
			</div>
		{/if}
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
