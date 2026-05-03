<!--
  SidePanel — Context-dependent sidebar.
  
  Content changes based on active mode:
  - Chat:        conversation list (time-grouped, searchable, pinnable)
  - Workspace:   list of active A2UI surfaces across agents
  - Orchestrate: agent hierarchy overview, delegation queue
  - Agents:      full agent management list
-->

<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { ModeId } from './ModeTabs.svelte';

	let {
		activeMode = 'chat',
		panelChat,
		panelWorkspace,
		panelOrchestrate,
		panelAgents,
	}: {
		activeMode: ModeId;
		panelChat?: Snippet;
		panelWorkspace?: Snippet;
		panelOrchestrate?: Snippet;
		panelAgents?: Snippet;
	} = $props();

	const headings: Record<ModeId, string> = {
		chat: 'Chats',
		workspace: 'Workspace',
		orchestrate: 'Orchestrate',
		agents: 'Agents',
	};
</script>

<aside class="side-panel">
	<div class="side-panel-head">
		<span class="side-panel-title">{headings[activeMode]}</span>
	</div>

	<div class="side-panel-body">
		{#if activeMode === 'chat' && panelChat}
			{@render panelChat()}
		{:else if activeMode === 'workspace' && panelWorkspace}
			{@render panelWorkspace()}
		{:else if activeMode === 'orchestrate' && panelOrchestrate}
			{@render panelOrchestrate()}
		{:else if activeMode === 'agents' && panelAgents}
			{@render panelAgents()}
		{:else}
			<div class="flex items-center justify-center h-full text-sm text-base-content/40">
				Coming soon
			</div>
		{/if}
	</div>
</aside>

<style>
	.side-panel {
		display: flex;
		flex-direction: column;
		width: 260px;
		min-width: 260px;
		height: 100%;
		background: oklch(var(--b2));
		border-right: 1px solid oklch(var(--b3));
		overflow: hidden;
	}

	.side-panel-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 12px 14px 8px;
		flex-shrink: 0;
	}

	.side-panel-title {
		font-size: 13px;
		font-weight: 700;
		letter-spacing: -0.01em;
		color: oklch(var(--bc));
	}

	.side-panel-body {
		flex: 1;
		overflow-y: auto;
		overflow-x: hidden;
		min-height: 0;
	}
</style>
