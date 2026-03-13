<script lang="ts">
	import { page } from '$app/stores';
	import { getContext, onMount } from 'svelte';
	import { getActiveRoles } from '$lib/api/nebo';
	import { Chat } from '$lib/components/chat';

	const channelState = getContext<{
		activeChannelId: string;
		activeChannelName: string;
		activeLoopName: string;
		activeRoleId: string;
		activeRoleName: string;
	}>('channelState');

	let loading = $state(true);
	let notFound = $state(false);

	const mode = $derived(
		channelState.activeRoleId
			? {
					type: 'role' as const,
					roleId: channelState.activeRoleId,
					roleName: channelState.activeRoleName
				}
			: { type: 'companion' as const }
	);

	onMount(async () => {
		const roleName = decodeURIComponent($page.params.name);

		// If context already has this role selected, skip the lookup
		if (channelState.activeRoleId && channelState.activeRoleName.toLowerCase() === roleName.toLowerCase()) {
			loading = false;
			return;
		}

		try {
			const data = await getActiveRoles();
			if (data?.roles) {
				const match = data.roles.find(
					(r) => r.name.toLowerCase() === roleName.toLowerCase()
				);
				if (match) {
					channelState.activeChannelId = '';
					channelState.activeChannelName = '';
					channelState.activeLoopName = '';
					channelState.activeRoleId = match.roleId;
					channelState.activeRoleName = match.name;
				} else {
					notFound = true;
				}
			} else {
				notFound = true;
			}
		} catch {
			notFound = true;
		}

		loading = false;
	});
</script>

<svelte:head>
	<title>Nebo - {channelState.activeRoleName || $page.params.name}</title>
</svelte:head>

{#if loading}
	<div class="flex items-center justify-center h-full">
		<span class="loading loading-spinner loading-lg"></span>
	</div>
{:else if notFound}
	<div class="flex flex-col items-center justify-center h-full gap-4 text-base-content/70">
		<svg class="w-12 h-12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
			<circle cx="12" cy="12" r="10" />
			<path d="M16 16s-1.5-2-4-2-4 2-4 2" />
			<line x1="9" y1="9" x2="9.01" y2="9" />
			<line x1="15" y1="9" x2="15.01" y2="9" />
		</svg>
		<p class="text-sm">Role "{decodeURIComponent($page.params.name)}" not found or not active</p>
		<a href="/agent" class="btn btn-sm btn-ghost">Back to Chat</a>
	</div>
{:else}
	{#key `role:${mode.type === 'role' ? mode.roleId : 'companion'}`}
		<Chat {mode} />
	{/key}
{/if}
