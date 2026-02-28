<script lang="ts">
	import type { Snippet } from 'svelte';
	import { setContext } from 'svelte';
	import Sidebar from '$lib/components/sidebar/Sidebar.svelte';

	let { children }: { children: Snippet } = $props();

	class ChannelState {
		activeChannelId = $state('');
		activeChannelName = $state('');
		activeLoopName = $state('');
	}

	const channelState = new ChannelState();
	setContext('channelState', channelState);
</script>

<div class="flex flex-1 min-h-0">
	<Sidebar
		bind:activeChannelId={channelState.activeChannelId}
		onSelectMyChat={() => { channelState.activeChannelId = ''; channelState.activeChannelName = ''; channelState.activeLoopName = ''; }}
		onSelectChannel={(id, name, loop) => { channelState.activeChannelId = id; channelState.activeChannelName = name; channelState.activeLoopName = loop; }}
	/>

	<!-- Main Content -->
	<main class="flex-1 flex flex-col min-w-0 overflow-hidden">
		{@render children()}
	</main>
</div>
