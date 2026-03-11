<script lang="ts">
	import { getContext } from 'svelte';
	import { Chat } from '$lib/components/chat';

	const channelState = getContext<{
		activeChannelId: string;
		activeChannelName: string;
		activeLoopName: string;
		activeRoleId: string;
		activeRoleName: string;
	}>('channelState');

	const mode = $derived(
		channelState.activeRoleId
			? {
					type: 'role' as const,
					roleId: channelState.activeRoleId,
					roleName: channelState.activeRoleName
				}
			: channelState.activeChannelId
				? {
						type: 'channel' as const,
						channelId: channelState.activeChannelId,
						channelName: channelState.activeChannelName,
						loopName: channelState.activeLoopName
					}
				: { type: 'companion' as const }
	);
</script>

<svelte:head>
	<title>Nebo - {mode.type === 'role' ? mode.roleName : 'Your AI Companion'}</title>
</svelte:head>

{#key mode.type === 'role' ? `role:${mode.roleId}` : mode.type === 'channel' ? mode.channelId : 'companion'}
	<Chat {mode} />
{/key}
