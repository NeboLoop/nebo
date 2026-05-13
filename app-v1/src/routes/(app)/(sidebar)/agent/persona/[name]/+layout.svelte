<script lang="ts">
	import type { Snippet } from 'svelte';
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { tick, getContext, untrack } from 'svelte';
	import { getActiveAgents, getAgent, updateAgent } from '$lib/api/nebo';
	import { t } from 'svelte-i18n';
	import { workspaceOpen, surfacesForAgent } from '$lib/stores/a2ui';
	import { getWebSocketClient } from '$lib/websocket/client';

	let { children }: { children: Snippet } = $props();

	const channelState = getContext<{
		activeChannelId: string;
		activeChannelName: string;
		activeLoopName: string;
		activeAgentId: string;
		activeAgentName: string;
		activeView: string;
	}>('channelState');

	let loading = $state(true);
	let notFound = $state(false);
	let editing = $state(false);
	let editValue = $state('');
	let inputEl: HTMLInputElement | undefined = $state();
	const param = $derived($page.params.name);
	const currentPath = $derived($page.url.pathname);
	const basePath = $derived(`/agent/persona/${param}`);
	const displayName = $derived(channelState.activeAgentName || param);

	function isTabActive(tab: string): boolean {
		if (tab === 'chat') {
			return currentPath === basePath || currentPath === `${basePath}/chat`;
		}
		return currentPath === `${basePath}/${tab}`;
	}

	async function startEditing() {
		editValue = displayName;
		editing = true;
		await tick();
		inputEl?.select();
	}

	async function saveRename() {
		const trimmed = editValue.trim();
		if (!trimmed || trimmed === displayName) {
			editing = false;
			return;
		}
		try {
			await updateAgent(channelState.activeAgentId, { name: trimmed });
			channelState.activeAgentName = trimmed;
		} catch {
			// revert on error
		}
		editing = false;
	}

	function cancelEditing() {
		editing = false;
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter') {
			e.preventDefault();
			saveRename();
		} else if (e.key === 'Escape') {
			e.preventDefault();
			cancelEditing();
		}
	}

	// Reset to chat view when switching agents
	$effect(() => {
		param;
		workspaceOpen.set(false);
	});

	// React only to param (route) changes — untrack channelState reads to avoid
	// re-triggering when sidebar navigation clears activeAgentId before goto.
	$effect(() => {
		const name = param;
		let cancelled = false;

		// If context already has this agent selected by ID, skip lookup
		if (untrack(() => channelState.activeAgentId) === name) {
			loading = false;
			notFound = false;
			return;
		}

		loading = true;
		notFound = false;

		getActiveAgents().then(async (data) => {
			if (cancelled) return;
			const match = data?.agents?.find((r) => r.agentId === name);
			if (match) {
				channelState.activeChannelId = '';
				channelState.activeChannelName = '';
				channelState.activeLoopName = '';
				channelState.activeAgentId = match.agentId;
				channelState.activeAgentName = match.name;
				channelState.activeView = 'agent';
				loading = false;
				return;
			}
			// Fallback: agent may be disabled — fetch it directly
			try {
				const detail = await getAgent(name);
				if (cancelled) return;
				if (detail?.agent) {
					channelState.activeChannelId = '';
					channelState.activeChannelName = '';
					channelState.activeLoopName = '';
					channelState.activeAgentId = detail.agent.id;
					channelState.activeAgentName = detail.agent.name;
					channelState.activeView = 'agent';
				} else {
					notFound = true;
				}
			} catch {
				if (cancelled) return;
				notFound = true;
			}
			loading = false;
		}).catch(() => {
			if (cancelled) return;
			notFound = true;
			loading = false;
		});

		return () => { cancelled = true; };
	});

</script>

{#if loading}
	<div class="flex items-center justify-center h-full">
		<span class="loading loading-spinner loading-lg"></span>
	</div>
{:else if notFound}
	<div class="flex flex-col items-center justify-center h-full gap-4 text-base-content/90">
		<svg class="w-12 h-12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
			<circle cx="12" cy="12" r="10" />
			<path d="M16 16s-1.5-2-4-2-4 2-4 2" />
			<line x1="9" y1="9" x2="9.01" y2="9" />
			<line x1="15" y1="9" x2="15.01" y2="9" />
		</svg>
		<p class="text-sm">{$t('agent.agentNotFound')}</p>
		<a href="/" class="btn btn-sm btn-ghost">{$t('agent.backToAgents')}</a>
	</div>
{:else}
	<div class="flex flex-col flex-1 min-h-0">
		<!-- V2 Agent Header: avatar + name + tab pills -->
		<div class="v2-main-head">
			<div class="v2-main-title">
				<div class="sidebar-agent-avatar w-[26px] h-[26px] rounded-[7px] bg-primary/10 text-primary text-[11px]">
					{(displayName || '?').charAt(0).toUpperCase()}
				</div>
				{#if editing}
					<input
						bind:this={inputEl}
						bind:value={editValue}
						class="agent-name-inline-input"
						onkeydown={handleKeydown}
						onblur={saveRename}
					/>
				{:else}
					<button class="agent-name-inline-btn" onclick={startEditing}>{displayName}</button>
				{/if}
			</div>
			<div class="agent-tab-bar-inline">
				<a href="{basePath}/chat" class="agent-tab-inline" class:agent-tab-inline-active={isTabActive('chat') && !$workspaceOpen} onclick={() => workspaceOpen.set(false)}>{$t('agent.chatTab')}</a>
				<button
					class="agent-tab-inline"
					class:agent-tab-inline-active={isTabActive('chat') && $workspaceOpen}
					onclick={() => {
						const opening = !$workspaceOpen;
						workspaceOpen.set(opening);
						if (opening) {
							if (!isTabActive('chat')) {
								goto(`${basePath}/chat`);
							}
							getWebSocketClient().send('a2ui_init', { agentId: param });
						}
					}}
				>Workspace</button>
				<a href="{basePath}/persona" class="agent-tab-inline" class:agent-tab-inline-active={isTabActive('persona')}>{$t('agent.personaTab')}</a>
				<a href="{basePath}/configure" class="agent-tab-inline" class:agent-tab-inline-active={isTabActive('configure')}>{$t('agent.configure')}</a>
				<a href="{basePath}/automate" class="agent-tab-inline" class:agent-tab-inline-active={isTabActive('automate')}>{$t('agent.automate')}</a>
				<a href="{basePath}/activity" class="agent-tab-inline" class:agent-tab-inline-active={isTabActive('activity')}>{$t('agent.activity')}</a>
				<a href="{basePath}/settings" class="agent-tab-inline" class:agent-tab-inline-active={isTabActive('settings')}>{$t('agent.settingsTab')}</a>
			</div>
		</div>

		<div class="flex-1 flex flex-col min-h-0">
			{@render children()}
		</div>
	</div>
{/if}
