<script lang="ts">
	import { getContext } from 'svelte';
	import { Chat } from '$lib/components/chat';

	const channelState = getContext<{
		activeChannelId: string;
		activeChannelName: string;
		activeLoopName: string;
	}>('channelState');

	const mode = $derived(
		channelState.activeChannelId
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
	<title>Nebo - Your AI Companion</title>
</svelte:head>

{#key mode.type === 'channel' ? mode.channelId : 'companion'}
	<Chat {mode} />
{/key}
