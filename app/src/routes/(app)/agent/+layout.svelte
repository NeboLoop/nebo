<script lang="ts">
	import type { Snippet } from 'svelte';
	import { setContext } from 'svelte';
	import Sidebar from '$lib/components/sidebar/Sidebar.svelte';

	let { children }: { children: Snippet } = $props();

	class ChannelState {
		activeChannelId = $state('');
		activeChannelName = $state('');
		activeLoopName = $state('');
		activeRoleId = $state('');
		activeRoleName = $state('');
	}

	const channelState = new ChannelState();
	setContext('channelState', channelState);

	function clearAll() {
		channelState.activeChannelId = '';
		channelState.activeChannelName = '';
		channelState.activeLoopName = '';
		channelState.activeRoleId = '';
		channelState.activeRoleName = '';
	}
</script>

<div class="flex flex-1 min-h-0">
	<Sidebar
		bind:activeChannelId={channelState.activeChannelId}
		activeRoleId={channelState.activeRoleId}
		onSelectMyChat={() => { clearAll(); }}
		onSelectChannel={(id, name, loop) => { clearAll(); channelState.activeChannelId = id; channelState.activeChannelName = name; channelState.activeLoopName = loop; }}
		onSelectRole={(id, name) => { clearAll(); channelState.activeRoleId = id; channelState.activeRoleName = name; }}
	/>

	<!-- Main Content -->
	<main class="flex-1 flex flex-col min-w-0 overflow-hidden">
		{@render children()}
	</main>
</div>
