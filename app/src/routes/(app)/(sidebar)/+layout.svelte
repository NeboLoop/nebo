<script lang="ts">
	import type { Snippet } from 'svelte';
	import { onMount } from 'svelte';
	import { setContext } from 'svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';
	import Sidebar from '$lib/components/sidebar/Sidebar.svelte';

	let { children }: { children: Snippet } = $props();

	let focusMode = $state(false);

	class ChannelState {
		activeChannelId = $state('');
		activeChannelName = $state('');
		activeLoopName = $state('');
		activeAgentId = $state('');
		activeAgentName = $state('');
		activeView = $state<'companion' | 'channel' | 'agent' | 'overview'>('overview');
	}

	const channelState = new ChannelState();
	setContext('channelState', channelState);

	onMount(() => {
		// Sync activeView from URL on initial load
		const path = $page.url.pathname;
		if (path.startsWith('/agent/assistant')) {
			channelState.activeView = 'companion';
		} else if (path.startsWith('/agent/persona/')) {
			channelState.activeView = 'agent';
		}

		function handleFocus(e: Event) {
			focusMode = (e as CustomEvent).detail;
		}
		window.addEventListener('nebo:focus-mode', handleFocus);
		return () => window.removeEventListener('nebo:focus-mode', handleFocus);
	});

	function clearAll() {
		channelState.activeChannelId = '';
		channelState.activeChannelName = '';
		channelState.activeLoopName = '';
		channelState.activeAgentId = '';
		channelState.activeAgentName = '';
	}
</script>

<div class="flex flex-1 min-h-0">
	<div class={focusMode ? 'sidebar-rail' : ''}>
	<Sidebar
		bind:activeChannelId={channelState.activeChannelId}
		activeAgentId={channelState.activeAgentId}
		activeView={channelState.activeView}
		onSelectMyChat={() => { clearAll(); channelState.activeView = 'companion'; goto('/agent/assistant/chat'); }}
		onSelectChannel={(id, name, loop) => { clearAll(); channelState.activeChannelId = id; channelState.activeChannelName = name; channelState.activeLoopName = loop; channelState.activeView = 'channel'; goto(`/agent/channel/${encodeURIComponent(name.toLowerCase())}`); }}
		onSelectAgent={(id, name) => { clearAll(); channelState.activeAgentId = id; channelState.activeAgentName = name; channelState.activeView = 'agent'; goto(`/agent/persona/${id}/chat`); }}
	/>
	</div>

	<!-- Main Content -->
	<main class="flex-1 flex flex-col min-w-0 overflow-hidden">
		{@render children()}
	</main>
</div>
