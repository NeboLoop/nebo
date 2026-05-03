<!--
  AppShell — Four-mode layout inspired by Claude (Chat/Cowork/Code) and Hermes.
  
  ┌─────────────────────────────────────────────────────────────┐
  │  [🔥 Nebo]  Chat | Workspace | Orchestrate | Agents   🔍  │  ← Header
  ├──────────┬──────────────────────────────────────────────────┤
  │          │                                                  │
  │  Side    │              Main Area                           │
  │  Panel   │  (changes based on active mode)                  │
  │  260px   │                                                  │
  │          │  Chat:        conversation + composer            │
  │          │  Workspace:   A2UI surfaces for active agent     │
  │          │  Orchestrate: org chart + delegation flows       │
  │          │  Agents:      config tabs for selected agent     │
  │          │                                                  │
  └──────────┴──────────────────────────────────────────────────┘
  
  The active agent context (who you're talking to / viewing) is global,
  controlled by the composer chip in Chat mode or agent selection in other
  modes. A2UI surfaces, chat history, and orchestration state all key off
  this shared agent context.
-->

<script lang="ts">
	import type { Snippet } from 'svelte';
	import ModeTabs, { type ModeId } from './ModeTabs.svelte';
	import SidePanel from './SidePanel.svelte';
	import { NeboIcon } from '$lib/components/icons';
	import { Search } from 'lucide-svelte';
	import { goto } from '$app/navigation';

	let {
		children,
		panelChat,
		panelWorkspace,
		panelOrchestrate,
		panelAgents,
		userName = '',
		onCommandPalette,
	}: {
		children: Snippet;
		panelChat?: Snippet;
		panelWorkspace?: Snippet;
		panelOrchestrate?: Snippet;
		panelAgents?: Snippet;
		userName?: string;
		onCommandPalette?: () => void;
	} = $props();

	let activeMode = $state<ModeId>('chat');

	function handleModeSwitch(mode: ModeId) {
		activeMode = mode;
	}
</script>

<div class="app-shell">
	<!-- Header: Brand + Mode Tabs + Search -->
	<header class="app-header">
		<a href="/" class="app-brand" title="Home" aria-label="Home">
			<NeboIcon class="w-[18px] h-[18px]" />
			<span class="app-brand-name">Nebo</span>
		</a>

		<ModeTabs {activeMode} onSwitch={handleModeSwitch} />

		<div class="app-header-spacer"></div>

		<button
			type="button"
			class="app-search-btn"
			onclick={() => onCommandPalette?.()}
			aria-label="Search"
		>
			<Search class="w-[14px] h-[14px]" />
			<span class="app-search-hint">Search…</span>
			<kbd class="app-kbd">⌘K</kbd>
		</button>
	</header>

	<!-- Body: Sidebar + Main -->
	<div class="app-body">
		<SidePanel
			{activeMode}
			{panelChat}
			{panelWorkspace}
			{panelOrchestrate}
			{panelAgents}
		/>

		<main class="app-main">
			{@render children()}
		</main>
	</div>
</div>

<style>
	.app-shell {
		display: flex;
		flex-direction: column;
		width: 100%;
		height: 100%;
		overflow: hidden;
	}

	/* ── Header ─────────────────────────────────────────────── */

	.app-header {
		display: flex;
		align-items: center;
		gap: 4px;
		height: 46px;
		min-height: 46px;
		padding: 0 12px;
		background: oklch(var(--b2));
		border-bottom: 1px solid oklch(var(--b3));
		/* macOS title bar drag region */
		-webkit-app-region: drag;
	}
	/* Buttons inside header must be clickable (not draggable) */
	.app-header :global(button),
	.app-header :global(a) {
		-webkit-app-region: no-drag;
	}

	.app-brand {
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 4px 8px 4px 4px;
		border-radius: 8px;
		text-decoration: none;
		color: oklch(var(--bc));
		transition: background 0.15s;
		margin-right: 4px;
		flex-shrink: 0;
	}
	.app-brand:hover {
		background: oklch(var(--b3));
	}
	.app-brand-name {
		font-size: 14px;
		font-weight: 700;
		letter-spacing: -0.02em;
	}

	.app-header-spacer {
		flex: 1;
	}

	.app-search-btn {
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 5px 10px;
		border-radius: 8px;
		border: 1px solid oklch(var(--b3));
		background: oklch(var(--b1));
		color: oklch(var(--bc) / 0.4);
		font-size: 13px;
		cursor: pointer;
		transition: border-color 0.15s;
		flex-shrink: 0;
	}
	.app-search-btn:hover {
		border-color: oklch(var(--bc) / 0.2);
	}
	.app-search-hint {
		color: oklch(var(--bc) / 0.35);
	}
	.app-kbd {
		font-size: 11px;
		padding: 1px 5px;
		border-radius: 4px;
		background: oklch(var(--b3));
		color: oklch(var(--bc) / 0.4);
		font-family: inherit;
		border: none;
	}

	/* ── Body ───────────────────────────────────────────────── */

	.app-body {
		display: flex;
		flex: 1;
		min-height: 0;
		overflow: hidden;
	}

	.app-main {
		flex: 1;
		display: flex;
		flex-direction: column;
		min-width: 0;
		min-height: 0;
		overflow: hidden;
	}
</style>
