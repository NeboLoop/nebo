<script lang="ts">
	import type { Snippet } from 'svelte';
	import { page } from '$app/stores';
	import { getContext } from 'svelte';
	import { t } from 'svelte-i18n';

	let { children }: { children: Snippet } = $props();

	const channelState = getContext<{
		activeChannelId: string;
		activeChannelName: string;
		activeLoopName: string;
		activeAgentId: string;
		activeAgentName: string;
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
		channelState.activeAgentId = '';
		channelState.activeAgentName = 'Assistant';
		channelState.activeView = 'companion';
	});
</script>

<div class="flex flex-col flex-1 min-h-0">
	<!-- V2 Assistant Header: avatar + name + tab pills -->
	<div class="v2-main-head">
		<div class="v2-main-title">
			<div class="sidebar-agent-avatar" style="width: 26px; height: 26px; border-radius: 7px; background: var(--agent-violet-bg); color: var(--agent-violet-ink); font-size: 11px;">
				A
			</div>
			<span>{$t('agent.assistant')}</span>
		</div>
		<div class="agent-tab-bar-inline">
			<a href="{basePath}/chat" class="agent-tab-inline" class:agent-tab-inline-active={isTabActive('chat')}>{$t('agent.chatTab')}</a>
			<a href="{basePath}/persona" class="agent-tab-inline" class:agent-tab-inline-active={isTabActive('persona')}>{$t('agent.personaTab')}</a>
			<a href="{basePath}/automate" class="agent-tab-inline" class:agent-tab-inline-active={isTabActive('automate')}>{$t('agent.automate')}</a>
			<a href="{basePath}/activity" class="agent-tab-inline" class:agent-tab-inline-active={isTabActive('activity')}>{$t('agent.activity')}</a>
			<a href="{basePath}/settings" class="agent-tab-inline" class:agent-tab-inline-active={isTabActive('settings')}>{$t('agent.settingsTab')}</a>
		</div>
	</div>

	<div class="flex-1 flex flex-col min-h-0">
		{@render children()}
	</div>
</div>
