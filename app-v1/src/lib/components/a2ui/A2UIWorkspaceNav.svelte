<!--
  A2UIWorkspaceNav — tabbed navigation for A2UI workspace panels.

  Fetches the agent's nav config from the backend and renders
  a horizontal tab bar. Clicking a tab sends an a2ui_navigate
  WebSocket message to switch views.
-->
<script lang="ts">
	import { onMount } from 'svelte';
	import {
		LayoutDashboard,
		ClipboardList,
		Target,
		Settings,
		Home,
		FileText,
		BarChart3,
		Users,
		Calendar,
		Mail,
		MapPin,
		ExternalLink,
		X,
		type IconProps
	} from 'lucide-svelte';
	import { getWebSocketClient } from '$lib/websocket/client';
	import type { Component } from 'svelte';

	let {
		agentId,
		activeSurfaceId,
		onClose
	}: {
		agentId: string;
		activeSurfaceId: string;
		onClose?: () => void;
	} = $props();

	interface NavItem {
		viewId: string;
		label: string;
		icon?: string;
	}

	let navItems: NavItem[] = $state([]);

	const activeViewId = $derived(() => {
		const parts = activeSurfaceId.split(':');
		return parts.length >= 3 ? parts[2] : 'default';
	});

	const ICON_MAP: Record<string, Component<IconProps>> = {
		layoutDashboard: LayoutDashboard,
		clipboardList: ClipboardList,
		target: Target,
		settings: Settings,
		home: Home,
		fileText: FileText,
		barChart3: BarChart3,
		users: Users,
		calendar: Calendar,
		mail: Mail,
		mapPin: MapPin
	};

	function getIcon(item: NavItem): Component<IconProps> {
		if (item.icon && ICON_MAP[item.icon]) return ICON_MAP[item.icon];
		// Fallback based on viewId
		if (item.viewId === 'default') return LayoutDashboard;
		if (item.viewId.includes('review') || item.viewId.includes('queue')) return ClipboardList;
		if (item.viewId.includes('accuracy') || item.viewId.includes('metric')) return BarChart3;
		if (item.viewId.includes('settings') || item.viewId.includes('config')) return Settings;
		if (item.viewId.includes('booking') || item.viewId.includes('calendar')) return Calendar;
		if (item.viewId.includes('letter') || item.viewId.includes('mail') || item.viewId.includes('postcard')) return Mail;
		if (item.viewId.includes('prospect')) return Target;
		if (item.viewId.includes('activity')) return Home;
		return FileText;
	}

	function handleNavigate(viewId: string) {
		if (viewId === activeViewId()) return;
		getWebSocketClient().send('a2ui_navigate', {
			surfaceId: activeSurfaceId,
			targetView: viewId
		});
	}

	function handlePopOut() {
		const url = `/workspace/${encodeURIComponent(agentId)}`;
		window.open(url, `nebo-workspace-${agentId}`, 'width=1200,height=800,resizable=yes');
	}

	onMount(async () => {
		try {
			const res = await fetch(`/api/v1/agents/${encodeURIComponent(agentId)}/nav`);
			if (res.ok) {
				const items = await res.json();
				if (Array.isArray(items) && items.length > 0) {
					navItems = items;
				}
			}
		} catch {
			// No nav config — component renders nothing
		}
	});
</script>

{#if navItems.length > 1}
	<nav class="a2ui-workspace-nav" role="tablist">
		{#each navItems as item (item.viewId)}
			{@const Icon = getIcon(item)}
			<button
				class="a2ui-workspace-nav-item"
				class:active={item.viewId === activeViewId()}
				role="tab"
				aria-selected={item.viewId === activeViewId()}
				onclick={() => handleNavigate(item.viewId)}
			>
				<Icon size={16} />
				<span>{item.label}</span>
			</button>
		{/each}
		<div class="a2ui-workspace-nav-actions">
			<button
				class="a2ui-workspace-nav-action"
				onclick={handlePopOut}
				aria-label="Open in new window"
			>
				<ExternalLink size={14} />
			</button>
			<button
				class="a2ui-workspace-nav-action"
				onclick={() => onClose?.()}
				aria-label="Close workspace"
			>
				<X size={14} />
			</button>
		</div>
	</nav>
{:else}
	<nav class="a2ui-workspace-nav">
		<div class="a2ui-workspace-nav-actions">
			<button
				class="a2ui-workspace-nav-action"
				onclick={handlePopOut}
				aria-label="Open in new window"
			>
				<ExternalLink size={14} />
			</button>
			<button
				class="a2ui-workspace-nav-action"
				onclick={() => onClose?.()}
				aria-label="Close workspace"
			>
				<X size={14} />
			</button>
		</div>
	</nav>
{/if}
