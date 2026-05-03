<script lang="ts">
	import { getContext } from 'svelte';
	import { Chat } from '$lib/components/chat';

	const channelState = getContext<{
		activeAgentId: string;
		activeAgentName: string;
	}>('channelState');

	const chatMode = $derived(
		channelState.activeAgentId
			? {
					type: 'agent' as const,
					agentId: channelState.activeAgentId,
					agentName: channelState.activeAgentName
				}
			: { type: 'companion' as const }
	);
</script>

<svelte:head>
	<title>Nebo - {channelState.activeAgentName || 'Chat'}</title>
</svelte:head>

{#key `agent-chat:${channelState.activeAgentId}`}
	<Chat mode={chatMode} />
{/key}
