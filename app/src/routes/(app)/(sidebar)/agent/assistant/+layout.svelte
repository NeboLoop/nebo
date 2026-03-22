<script lang="ts">
	import type { Snippet } from 'svelte';
	import { page } from '$app/stores';
	import { getContext } from 'svelte';

	let { children }: { children: Snippet } = $props();

	const channelState = getContext<{
		activeChannelId: string;
		activeChannelName: string;
		activeLoopName: string;
		activeRoleId: string;
		activeRoleName: string;
		activeView: string;
	}>('channelState');

	const currentPath = $derived($page.url.pathname);
	const basePath = '/agent/assistant';

	function isTabActive(tab: string): boolean {
		if (tab === 'chat') {
			return currentPath === basePath || currentPath === `${basePath}/chat`;
		}
		return currentPath === `${basePath}/${tab}`;
	}

	$effect(() => {
		channelState.activeChannelId = '';
		channelState.activeChannelName = '';
		channelState.activeLoopName = '';
		channelState.activeRoleId = '';
		channelState.activeRoleName = 'Assistant';
		channelState.activeView = 'companion';
	});
</script>

<div class="flex flex-col flex-1 min-h-0">
	<header class="border-b border-base-300 bg-base-100/80 backdrop-blur-sm shrink-0">
		<div class="flex items-center justify-between px-6 h-12">
			<div class="flex items-center gap-3">
				<svg class="w-5 h-5 text-primary" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
					<path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
				</svg>
				<h1 class="text-base font-semibold text-base-content leading-tight">Assistant</h1>
			</div>
			<div class="agent-tab-bar-inline">
				<a href="{basePath}/chat" class="agent-tab-inline" class:agent-tab-inline-active={isTabActive('chat')}>Chat</a>
				<a href="{basePath}/role" class="agent-tab-inline" class:agent-tab-inline-active={isTabActive('role')}>Role</a>
				<a href="{basePath}/automate" class="agent-tab-inline" class:agent-tab-inline-active={isTabActive('automate')}>Automate</a>
				<a href="{basePath}/activity" class="agent-tab-inline" class:agent-tab-inline-active={isTabActive('activity')}>Activity</a>
				<a href="{basePath}/settings" class="agent-tab-inline" class:agent-tab-inline-active={isTabActive('settings')}>Settings</a>
			</div>
		</div>
	</header>

	<div class="flex-1 flex flex-col min-h-0">
		{@render children()}
	</div>
</div>
