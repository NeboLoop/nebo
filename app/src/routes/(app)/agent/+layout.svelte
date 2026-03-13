<script lang="ts">
	import type { Snippet } from 'svelte';
	import { setContext } from 'svelte';
	import { goto } from '$app/navigation';
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

	function updateUrl() {
		if (channelState.activeRoleId) {
			goto(`/agent/role/${encodeURIComponent(channelState.activeRoleName.toLowerCase())}`, { replaceState: true });
		} else if (channelState.activeChannelId) {
			goto(`/agent/channel/${encodeURIComponent(channelState.activeChannelName.toLowerCase())}`, { replaceState: true });
		} else {
			goto('/agent', { replaceState: true });
		}
	}
</script>

<div class="flex flex-1 min-h-0">
	<Sidebar
		bind:activeChannelId={channelState.activeChannelId}
		activeRoleId={channelState.activeRoleId}
		onSelectMyChat={() => { clearAll(); updateUrl(); }}
		onSelectChannel={(id, name, loop) => { clearAll(); channelState.activeChannelId = id; channelState.activeChannelName = name; channelState.activeLoopName = loop; updateUrl(); }}
		onSelectRole={(id, name) => { clearAll(); channelState.activeRoleId = id; channelState.activeRoleName = name; updateUrl(); }}
	/>

	<!-- Main Content -->
	<main class="flex-1 flex flex-col min-w-0 overflow-hidden">
		{@render children()}
	</main>
</div>
