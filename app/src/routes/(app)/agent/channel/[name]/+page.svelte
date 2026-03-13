<script lang="ts">
	import { page } from '$app/stores';
	import { getContext, onMount } from 'svelte';
	import { getLoops } from '$lib/api/nebo';
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
		channelState.activeChannelId
			? {
					type: 'channel' as const,
					channelId: channelState.activeChannelId,
					channelName: channelState.activeChannelName,
					loopName: channelState.activeLoopName
				}
			: { type: 'companion' as const }
	);

	onMount(async () => {
		const channelName = decodeURIComponent($page.params.name);

		// If context already has this channel selected, skip the lookup
		if (channelState.activeChannelId && channelState.activeChannelName.toLowerCase() === channelName.toLowerCase()) {
			loading = false;
			return;
		}

		try {
			const data = await getLoops();
			if (data?.loops) {
				for (const loop of data.loops) {
					if (loop.channels) {
						const match = loop.channels.find(
							(c) => c.channelName.toLowerCase() === channelName.toLowerCase()
						);
						if (match) {
							channelState.activeRoleId = '';
							channelState.activeRoleName = '';
							channelState.activeChannelId = match.channelId;
							channelState.activeChannelName = match.channelName;
							channelState.activeLoopName = loop.name;
							loading = false;
							return;
						}
					}
				}
			}
			notFound = true;
		} catch {
			notFound = true;
		}

		loading = false;
	});
</script>

<svelte:head>
	<title>Nebo - #{channelState.activeChannelName || $page.params.name}</title>
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
		<p class="text-sm">Channel "{decodeURIComponent($page.params.name)}" not found</p>
		<a href="/agent" class="btn btn-sm btn-ghost">Back to Chat</a>
	</div>
{:else}
	{#key mode.type === 'channel' ? mode.channelId : 'companion'}
		<Chat {mode} />
	{/key}
{/if}
