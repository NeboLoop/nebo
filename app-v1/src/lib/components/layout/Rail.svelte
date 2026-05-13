<!--
  Rail — Vertical icon strip for switching sidebar panels.
  Hermes-style: icons only, active state highlight, fixed left edge.
  
  Each button swaps what the sidebar shows (chat list, agents, skills, etc.)
  without navigating away from the current conversation.
-->

<script lang="ts">
	import {
		MessageSquare,
		Bot,
		Layers,
		Store,
		Settings,
		Zap
	} from 'lucide-svelte';
	import { NeboIcon } from '$lib/components/icons';
	import type { ComponentType } from 'svelte';

	export type PanelId = 'chat' | 'agents' | 'skills' | 'marketplace' | 'integrations' | 'settings';

	let {
		activePanel = 'chat',
		onSwitch
	}: {
		activePanel: PanelId;
		onSwitch: (panel: PanelId) => void;
	} = $props();

	interface RailItem {
		id: PanelId;
		icon: ComponentType;
		label: string;
		bottom?: boolean;
	}

	const items: RailItem[] = [
		{ id: 'chat', icon: MessageSquare, label: 'Chats' },
		{ id: 'agents', icon: Bot, label: 'Agents' },
		{ id: 'skills', icon: Layers, label: 'Skills' },
		{ id: 'integrations', icon: Zap, label: 'Integrations' },
		{ id: 'marketplace', icon: Store, label: 'Marketplace' },
		{ id: 'settings', icon: Settings, label: 'Settings', bottom: true },
	];

	const topItems = $derived(items.filter(i => !i.bottom));
	const bottomItems = $derived(items.filter(i => i.bottom));
</script>

<nav class="rail-nav" aria-label="Primary navigation">
	<!-- Brand mark -->
	<a href="/" class="rail-brand" title="Home" aria-label="Home">
		<NeboIcon class="w-[20px] h-[20px]" />
	</a>

	<!-- Top items -->
	<div class="rail-top">
		{#each topItems as item (item.id)}
			<button
				class="rail-btn"
				class:active={activePanel === item.id}
				title={item.label}
				aria-label={item.label}
				aria-pressed={activePanel === item.id}
				onclick={() => onSwitch(item.id)}
			>
				<item.icon class="w-[18px] h-[18px]" />
			</button>
		{/each}
	</div>

	<!-- Spacer -->
	<div class="flex-1"></div>

	<!-- Bottom items -->
	<div class="rail-bottom">
		{#each bottomItems as item (item.id)}
			<button
				class="rail-btn"
				class:active={activePanel === item.id}
				title={item.label}
				aria-label={item.label}
				aria-pressed={activePanel === item.id}
				onclick={() => onSwitch(item.id)}
			>
				<item.icon class="w-[18px] h-[18px]" />
			</button>
		{/each}
	</div>
</nav>

<style>
	.rail-nav {
		display: flex;
		flex-direction: column;
		align-items: center;
		width: 52px;
		min-width: 52px;
		height: 100%;
		padding: 8px 0;
		background: oklch(var(--b2));
		border-right: 1px solid oklch(var(--b3));
		gap: 2px;
		overflow: hidden;
	}

	.rail-brand {
		display: grid;
		place-items: center;
		width: 36px;
		height: 36px;
		margin-bottom: 8px;
		border-radius: 10px;
		color: oklch(var(--p));
		text-decoration: none;
		transition: background 0.15s;
	}
	.rail-brand:hover {
		background: oklch(var(--b3));
	}

	.rail-top,
	.rail-bottom {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 2px;
	}

	.rail-btn {
		display: grid;
		place-items: center;
		width: 36px;
		height: 36px;
		border-radius: 8px;
		border: none;
		background: transparent;
		color: oklch(var(--bc) / 0.5);
		cursor: pointer;
		transition: background 0.15s, color 0.15s;
	}
	.rail-btn:hover {
		background: oklch(var(--b3));
		color: oklch(var(--bc) / 0.8);
	}
	.rail-btn.active {
		background: oklch(var(--p) / 0.12);
		color: oklch(var(--p));
	}
</style>
