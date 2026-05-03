<!--
  ModeTabs — Top-level mode switcher.
  
  Claude-style horizontal text tabs that switch both the sidebar content
  AND the main area simultaneously. Four modes:
  
  - Chat:        Talk to agents, see delegation inline
  - Workspace:   A2UI surfaces — see what agents are building
  - Orchestrate: Org chart, delegation flows, coordination view
  - Agents:      Backstage management (config, persona, skills, settings)
  
  Settings and Marketplace are separate routes, not modes.
-->

<script lang="ts">
	export type ModeId = 'chat' | 'workspace' | 'orchestrate' | 'agents';

	let {
		activeMode = 'chat',
		onSwitch
	}: {
		activeMode: ModeId;
		onSwitch: (mode: ModeId) => void;
	} = $props();

	interface ModeItem {
		id: ModeId;
		label: string;
	}

	const modes: ModeItem[] = [
		{ id: 'chat', label: 'Chat' },
		{ id: 'workspace', label: 'Workspace' },
		{ id: 'orchestrate', label: 'Orchestrate' },
		{ id: 'agents', label: 'Agents' },
	];
</script>

<div class="mode-tabs" role="tablist" aria-label="Application mode">
	{#each modes as mode (mode.id)}
		<button
			class="mode-tab"
			class:active={activeMode === mode.id}
			role="tab"
			aria-selected={activeMode === mode.id}
			onclick={() => onSwitch(mode.id)}
		>
			{mode.label}
		</button>
	{/each}
</div>

<style>
	.mode-tabs {
		display: flex;
		align-items: center;
		gap: 2px;
		padding: 6px 8px 0;
		flex-shrink: 0;
	}

	.mode-tab {
		position: relative;
		padding: 5px 10px 7px;
		border: none;
		background: transparent;
		border-radius: 6px;
		font-size: 13px;
		font-weight: 500;
		color: oklch(var(--bc) / 0.45);
		cursor: pointer;
		transition: color 0.15s, background 0.15s;
		white-space: nowrap;
	}
	.mode-tab:hover {
		color: oklch(var(--bc) / 0.7);
		background: oklch(var(--b3) / 0.5);
	}
	.mode-tab.active {
		color: oklch(var(--bc));
		background: oklch(var(--b3));
	}
</style>
