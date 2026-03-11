<script lang="ts">
	import { onMount } from 'svelte';
	import { getWebSocketClient } from '$lib/websocket/client';
	import { getLoops, getActiveRoles } from '$lib/api/nebo';
	import type { GetLoopsResponse, LoopChannelEntry, LoopEntry } from '$lib/api/neboComponents';

	interface ActiveRoleEntry {
		roleId: string;
		name: string;
		channelId?: string;
		workflowCount: number;
		skillCount: number;
	}

	let {
		activeChannelId = $bindable(''),
		activeRoleId = '',
		onSelectMyChat = () => {},
		onSelectChannel = (_channelId: string, _channelName: string, _loopName: string) => {},
		onSelectRole = (_roleId: string, _roleName: string) => {}
	}: {
		activeChannelId?: string;
		activeRoleId?: string;
		onSelectMyChat?: () => void;
		onSelectChannel?: (channelId: string, channelName: string, loopName: string) => void;
		onSelectRole?: (roleId: string, roleName: string) => void;
	} = $props();

	let loops: LoopEntry[] = $state([]);
	let expandedLoops: Set<string> = $state(new Set());
	let activeRoles: ActiveRoleEntry[] = $state([]);
	let desktopActive = $state(false);
	let heartbeatActive = $state(false);
	let eventsActive = $state(0);
	let notificationCount = $state(0);

	const isMyChatActive = $derived(activeChannelId === '' && activeRoleId === '');

	async function loadLoops() {
		try {
			const data = await getLoops() as GetLoopsResponse;
			if (data?.loops) {
				loops = data.loops;
				// Auto-expand all loops on first load
				if (expandedLoops.size === 0) {
					expandedLoops = new Set(data.loops.map((l) => l.id));
				}
			}
			if (data) {
				heartbeatActive = data.heartbeatActive ?? false;
				eventsActive = data.eventsActive ?? 0;
				desktopActive = data.desktopActive ?? false;
			}
		} catch {
			// NeboLoop not connected — empty is fine
		}
	}

	function toggleLoop(loopId: string) {
		const next = new Set(expandedLoops);
		if (next.has(loopId)) {
			next.delete(loopId);
		} else {
			next.add(loopId);
		}
		expandedLoops = next;
	}

	async function loadActiveRoles() {
		try {
			const data = await getActiveRoles();
			if (data?.roles) {
				activeRoles = data.roles;
			}
		} catch {
			// No active roles — fine
		}
	}

	function selectMyChat() {
		activeChannelId = '';
		onSelectMyChat();
	}

	function selectChannel(channel: LoopChannelEntry, loopName: string) {
		activeChannelId = channel.channelId;
		onSelectChannel(channel.channelId, channel.channelName, loopName);
	}

	function selectRole(role: ActiveRoleEntry) {
		onSelectRole(role.roleId, role.name);
	}

	onMount(() => {
		loadLoops();
		loadActiveRoles();

		const wsClient = getWebSocketClient();

		// Reload loops when agent reconnects (may have new channels)
		const unsubStatus = wsClient.onStatus((status) => {
			if (status === 'connected') {
				loadLoops();
				loadActiveRoles();
			}
		});

		// Listen for desktop activity events
		const unsubDesktop = wsClient.on<{ active: boolean }>('desktop_activity', (data) => {
			if (data) desktopActive = data.active;
		});

		// Listen for notification events
		const unsubNotify = wsClient.on<{ content: string }>('notification', (data) => {
			if (data) notificationCount++;
		});

		// Listen for lane updates to refresh activity indicators
		const unsubLane = wsClient.on('lane_update', () => {
			loadLoops();
		});

		// Reload roles when activated/deactivated
		const unsubRoleActivated = wsClient.on('role_activated', () => {
			loadActiveRoles();
		});
		const unsubRoleDeactivated = wsClient.on('role_deactivated', () => {
			loadActiveRoles();
		});

		// Periodic refresh (channels can change via NeboLoop)
		const refreshInterval = setInterval(() => {
			loadLoops();
			loadActiveRoles();
		}, 60000);

		return () => {
			unsubStatus();
			unsubDesktop();
			unsubNotify();
			unsubLane();
			unsubRoleActivated();
			unsubRoleDeactivated();
			clearInterval(refreshInterval);
		};
	});
</script>

<aside class="sidebar-container">
	<nav class="sidebar-nav">
		<!-- My Chat — always pinned at top -->
		<button
			class="sidebar-item sidebar-my-chat"
			class:sidebar-item-active={isMyChatActive}
			onclick={selectMyChat}
		>
			<svg class="sidebar-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
				<path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
			</svg>
			<span class="sidebar-label">My Chat</span>
			{#if notificationCount > 0}
				<span class="sidebar-badge">{notificationCount}</span>
			{/if}
		</button>

		<!-- Loops with channels + roles -->
		{#if loops.length > 0}
			<div class="sidebar-section-label">Loops</div>
			{#each loops as loop (loop.id)}
				<button
					class="sidebar-item sidebar-loop-header"
					onclick={() => toggleLoop(loop.id)}
				>
					<svg class="sidebar-icon-sm sidebar-chevron" class:sidebar-chevron-open={expandedLoops.has(loop.id)} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
						<polyline points="9 18 15 12 9 6" />
					</svg>
					<span class="sidebar-label">{loop.name || loop.id}</span>
				</button>

				{#if expandedLoops.has(loop.id)}
					{#if loop.channels}
						{#each loop.channels as channel (channel.channelId)}
							<button
								class="sidebar-item sidebar-channel"
								class:sidebar-item-active={activeChannelId === channel.channelId}
								onclick={() => selectChannel(channel, loop.name)}
							>
								<span class="sidebar-channel-hash">#</span>
								<span class="sidebar-label">{channel.channelName}</span>
							</button>
						{/each}
					{/if}

					<!-- Roles within this loop -->
					{#each activeRoles.filter(r => r.channelId) as role (role.roleId)}
						<button
							class="sidebar-item sidebar-role"
							class:sidebar-item-active={activeRoleId === role.roleId}
							onclick={() => selectRole(role)}
						>
							<span class="sidebar-channel-hash">#</span>
							<span class="sidebar-label">{role.name.toLowerCase()}</span>
							<span class="sidebar-role-indicator">&#10038;</span>
						</button>
					{/each}
				{/if}
			{/each}
		{/if}

		<!-- Standalone roles (not yet linked to a loop) -->
		{#if activeRoles.filter(r => !r.channelId).length > 0}
			<div class="sidebar-section-label">Roles</div>
			{#each activeRoles.filter(r => !r.channelId) as role (role.roleId)}
				<button
					class="sidebar-item sidebar-role"
					class:sidebar-item-active={activeRoleId === role.roleId}
					onclick={() => selectRole(role)}
				>
					<span class="sidebar-channel-hash">#</span>
					<span class="sidebar-label">{role.name.toLowerCase()}</span>
					<span class="sidebar-role-indicator">&#10038;</span>
				</button>
			{/each}
		{/if}

		<!-- Activity section — always visible, pulse dot shows when active -->
		<div class="sidebar-section-label">Activity</div>

		<div class="sidebar-activity-item" class:sidebar-activity-idle={!heartbeatActive}>
			<svg class="sidebar-icon-sm" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
				<polyline points="22 12 18 12 15 21 9 3 6 12 2 12" />
			</svg>
			<span class="sidebar-label-sm">Heartbeat</span>
			{#if heartbeatActive}
				<span class="sidebar-pulse"></span>
			{/if}
		</div>

		<div class="sidebar-activity-item" class:sidebar-activity-idle={eventsActive === 0}>
			<svg class="sidebar-icon-sm" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
				<circle cx="12" cy="12" r="10" />
				<polyline points="12 6 12 12 16 14" />
			</svg>
			<span class="sidebar-label-sm">Events{eventsActive > 0 ? ` (${eventsActive})` : ''}</span>
			{#if eventsActive > 0}
				<span class="sidebar-pulse"></span>
			{/if}
		</div>

		<div class="sidebar-activity-item" class:sidebar-activity-idle={!desktopActive}>
			<svg class="sidebar-icon-sm" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
				<rect x="2" y="3" width="20" height="14" rx="2" ry="2" />
				<line x1="8" y1="21" x2="16" y2="21" />
				<line x1="12" y1="17" x2="12" y2="21" />
			</svg>
			<span class="sidebar-label-sm">Desktop</span>
			{#if desktopActive}
				<span class="sidebar-pulse"></span>
			{/if}
		</div>
	</nav>
</aside>
