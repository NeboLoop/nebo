<script lang="ts">
	import { getContext } from 'svelte';
	import { Chat } from '$lib/components/chat';

	const channelState = getContext<{
		activeRoleId: string;
		activeRoleName: string;
	}>('channelState');

	const chatMode = $derived(
		channelState.activeRoleId
			? {
					type: 'role' as const,
					roleId: channelState.activeRoleId,
					roleName: channelState.activeRoleName
				}
			: { type: 'companion' as const }
	);
</script>

<svelte:head>
	<title>Nebo - {channelState.activeRoleName || 'Chat'}</title>
</svelte:head>

{#key `role-chat:${channelState.activeRoleId}`}
	<Chat mode={chatMode} />
{/key}
