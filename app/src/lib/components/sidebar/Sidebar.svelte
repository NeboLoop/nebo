<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { goto } from '$app/navigation';
	import { getWebSocketClient } from '$lib/websocket/client';
	import { getLoops, getActiveRoles, listRoles, activateRole, deactivateRole, deleteRole, duplicateRole, updateRole } from '$lib/api/nebo';
	import type { GetLoopsResponse, LoopChannelEntry, LoopEntry } from '$lib/api/neboComponents';
	import NewBotMenu from '$lib/components/agent/NewBotMenu.svelte';
	import AlertDialog from '$lib/components/ui/AlertDialog.svelte';
	import RoleSetupModal from '$lib/components/role/RoleSetupModal.svelte';

	interface SidebarRole {
		roleId: string;
		name: string;
		description?: string;
		isActive: boolean;
		nextFireAt?: number;
	}

	let {
		activeChannelId = $bindable(''),
		activeRoleId = '',
		activeView = 'role',
		onSelectMyChat = () => {},
		onSelectChannel = (_channelId: string, _channelName: string, _loopName: string) => {},
		onSelectRole = (_roleId: string, _roleName: string) => {},
	}: {
		activeChannelId?: string;
		activeRoleId?: string;
		activeView?: string;
		onSelectMyChat?: () => void;
		onSelectChannel?: (channelId: string, channelName: string, loopName: string) => void;
		onSelectRole?: (roleId: string, roleName: string) => void;
	} = $props();

	let loops: LoopEntry[] = $state([]);
	let expandedLoops: Set<string> = $state(new Set());
	let sidebarRoles: SidebarRole[] = $state([]);
	let notificationCount = $state(0);
	let showNewBotMenu = $state(false);
	let menuPos = $state({ top: 0, left: 0 });

	// Setup wizard state
	let showSetupWizard = $state(false);
	let setupRoleId = $state('');
	let setupRoleName = $state('');
	let setupRoleDescription = $state('');

	// Context menu state
	let contextMenu = $state<{ visible: boolean; x: number; y: number; role: SidebarRole | null }>({
		visible: false, x: 0, y: 0, role: null,
	});
	let renamingRoleId = $state('');
	let renameValue = $state('');
	let renameInputEl: HTMLInputElement | undefined = $state();
	let showDeleteDialog = $state(false);
	let deleteTarget = $state<SidebarRole | null>(null);

	const isMyChatActive = $derived(activeView === 'companion');

	const activeCount = $derived(sidebarRoles.filter(r => r.isActive).length);

	// Live countdown ticker
	let nowMs = $state(Date.now());
	let runningRoles = $state<Record<string, string>>({}); // roleId → status text

	function formatCountdown(nextFireAt: number): string {
		const remaining = Math.max(0, nextFireAt * 1000 - nowMs);
		if (remaining <= 0) return '';
		const totalSecs = Math.floor(remaining / 1000);
		const hrs = Math.floor(totalSecs / 3600);
		const mins = Math.floor((totalSecs % 3600) / 60);
		const secs = totalSecs % 60;
		if (hrs > 0) return `⏱ ${hrs}h ${mins}m`;
		if (mins > 0) return `⏱ ${mins}m ${secs}s`;
		return `⏱ ${secs}s`;
	}

	function roleSubtitle(role: SidebarRole): string {
		if (!role.isActive) return 'Paused';
		const running = runningRoles[role.roleId];
		if (running) return running; // "running" or "Step 2 of 3"
		if (role.nextFireAt) {
			const cd = formatCountdown(role.nextFireAt);
			if (cd) return cd;
			// Countdown expired but no running state — show description while we wait for refresh
		}
		return role.description || '';
	}

	const BOT_COLORS = [
		{ bg: 'bg-blue-500/10', text: 'text-blue-500' },
		{ bg: 'bg-violet-500/10', text: 'text-violet-500' },
		{ bg: 'bg-emerald-500/10', text: 'text-emerald-500' },
		{ bg: 'bg-amber-500/10', text: 'text-amber-500' },
		{ bg: 'bg-rose-500/10', text: 'text-rose-500' },
		{ bg: 'bg-cyan-500/10', text: 'text-cyan-500' },
	];

	function nameHash(name: string): number {
		let hash = 0;
		for (let i = 0; i < name.length; i++) {
			hash = ((hash << 5) - hash) + name.charCodeAt(i);
			hash |= 0;
		}
		return Math.abs(hash);
	}

	function roleColor(name: string) {
		return BOT_COLORS[nameHash(name) % BOT_COLORS.length];
	}

	function roleInitial(name: string): string {
		return name.charAt(0).toUpperCase();
	}

	async function loadLoops() {
		try {
			const data = await getLoops() as GetLoopsResponse;
			if (data?.loops) {
				loops = data.loops;
				if (expandedLoops.size === 0) {
					expandedLoops = new Set(data.loops.map((l) => l.id));
				}
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

	async function loadRoles() {
		try {
			const [allRes, activeRes] = await Promise.all([
				listRoles().catch(() => null),
				getActiveRoles().catch(() => null),
			]);

			const activeRoles = activeRes?.roles ?? [];
			const activeMap = new Map(activeRoles.map(r => [r.roleId, r]));
			const roles: SidebarRole[] = [];

			// DB roles
			if (allRes?.roles) {
				for (const r of allRes.roles) {
					const active = activeMap.get(r.id);
					roles.push({
						roleId: r.id,
						name: r.name,
						description: r.description || undefined,
						isActive: !!active,
						nextFireAt: (active as any)?.nextFireAt ?? undefined,
					});
				}
			}

			// Filesystem-only roles — only show if they have an active UUID
			if (allRes?.filesystemRoles) {
				for (const r of allRes.filesystemRoles) {
					const matchedActive = activeRoles.find(a => a.name === r.name);
					if (matchedActive && !roles.some(existing => existing.name === r.name)) {
						roles.push({
							roleId: matchedActive.roleId,
							name: r.name,
							description: r.description || undefined,
							isActive: true,
							nextFireAt: (matchedActive as any)?.nextFireAt ?? undefined,
						});
					}
				}
			}

			// Sort: active first, then alphabetical
			roles.sort((a, b) => {
				if (a.isActive !== b.isActive) return a.isActive ? -1 : 1;
				return a.name.localeCompare(b.name);
			});

			sidebarRoles = roles;
		} catch {
			// Fine
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

	function selectRole(role: SidebarRole) {
		if (contextMenu.visible || Date.now() - contextMenuClosedAt < 200) return;
		onSelectRole(role.roleId, role.name);
	}

	// ── Context menu ──

	function handleContextMenu(e: MouseEvent, role: SidebarRole) {
		e.preventDefault();
		(e.currentTarget as HTMLElement)?.blur();
		contextMenu = { visible: true, x: e.clientX, y: e.clientY, role };
	}

	let contextMenuClosedAt = 0;

	function handleWindowKeydown(e: KeyboardEvent) {
		if (!contextMenu.visible) return;
		if (e.key === 'Escape') {
			e.preventDefault();
			closeContextMenu();
		} else if (e.key === ' ' || e.key === 'Enter') {
			e.preventDefault();
			e.stopPropagation();
		}
	}

	function handleWindowKeyup(e: KeyboardEvent) {
		if (!contextMenu.visible) return;
		if (e.key === ' ' || e.key === 'Enter') {
			e.preventDefault();
			e.stopPropagation();
		}
	}

	function closeContextMenu() {
		contextMenu = { visible: false, x: 0, y: 0, role: null };
		contextMenuClosedAt = Date.now();
	}

	async function handleCtxRename() {
		if (!contextMenu.role) return;
		const role = contextMenu.role;
		closeContextMenu();
		renamingRoleId = role.roleId;
		renameValue = role.name;
		await tick();
		renameInputEl?.select();
	}

	async function saveRename() {
		const trimmed = renameValue.trim();
		const role = sidebarRoles.find(r => r.roleId === renamingRoleId);
		if (!trimmed || !role || trimmed === role.name) {
			renamingRoleId = '';
			return;
		}
		try {
			await updateRole(renamingRoleId, { name: trimmed });
			// Update local list immediately
			const idx = sidebarRoles.findIndex(r => r.roleId === renamingRoleId);
			if (idx >= 0) {
				sidebarRoles[idx] = { ...sidebarRoles[idx], name: trimmed };
			}
		} catch {
			// revert
		}
		renamingRoleId = '';
	}

	function cancelRename() {
		renamingRoleId = '';
	}

	function handleRenameKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter') {
			e.preventDefault();
			saveRename();
		} else if (e.key === 'Escape') {
			e.preventDefault();
			cancelRename();
		}
	}

	async function handleCtxDuplicate() {
		if (!contextMenu.role) return;
		const roleId = contextMenu.role.roleId;
		closeContextMenu();
		try {
			const res = await duplicateRole(roleId);
			if (res?.role) {
				await loadRoles();
				onSelectRole(res.role.id, res.role.name);
				goto(`/agent/role/${res.role.id}/chat`);
			}
		} catch {
			// ignore
		}
	}

	async function handleCtxToggle() {
		if (!contextMenu.role) return;
		const role = contextMenu.role;
		closeContextMenu();
		try {
			if (role.isActive) {
				await deactivateRole(role.roleId);
			} else {
				await activateRole(role.roleId);
			}
			await loadRoles();
		} catch {
			// ignore
		}
	}

	function handleCtxDelete() {
		if (!contextMenu.role) return;
		deleteTarget = contextMenu.role;
		closeContextMenu();
		showDeleteDialog = true;
	}

	async function confirmDelete() {
		if (!deleteTarget) return;
		showDeleteDialog = false;
		try {
			if (deleteTarget.isActive) await deactivateRole(deleteTarget.roleId);
			await deleteRole(deleteTarget.roleId);
			if (activeRoleId === deleteTarget.roleId) {
				goto('/agents');
			}
			await loadRoles();
		} catch {
			// ignore
		}
		deleteTarget = null;
	}

	onMount(() => {
		loadLoops();
		loadRoles();

		const wsClient = getWebSocketClient();

		const unsubStatus = wsClient.onStatus((status) => {
			if (status === 'connected') {
				loadLoops();
				loadRoles();
			}
		});

		const unsubNotify = wsClient.on<{ content: string }>('notification', (data) => {
			if (data) notificationCount++;
		});

		const unsubLane = wsClient.on('lane_update', () => {
			loadLoops();
		});

		const unsubRoleActivated = wsClient.on('role_activated', () => {
			loadRoles();
		});
		const unsubRoleDeactivated = wsClient.on('role_deactivated', () => {
			loadRoles();
		});
		const unsubRoleInstalled = wsClient.on('role_installed', () => {
			loadRoles();
		});
		const unsubRoleUninstalled = wsClient.on('role_uninstalled', () => {
			loadRoles();
		});
		const unsubRoleUpdated = wsClient.on('role_updated', () => {
			loadRoles();
		});
		const unsubRoleSetup = wsClient.on('role_setup', (data: { roleId: string; roleName: string; roleDescription: string }) => {
			setupRoleId = data.roleId;
			setupRoleName = data.roleName;
			setupRoleDescription = data.roleDescription || '';
			showSetupWizard = true;
		});
		const unsubRunStarted = wsClient.on('workflow_run_started', (data: { roleId: string }) => {
			if (data.roleId) {
				runningRoles = { ...runningRoles, [data.roleId]: 'running' };
			}
		});
		const unsubActivityUpdate = wsClient.on('workflow_activity_update', (data: { roleId: string; step: number; totalSteps: number }) => {
			if (data.roleId) {
				runningRoles = { ...runningRoles, [data.roleId]: `Step ${data.step} of ${data.totalSteps}` };
			}
		});
		const unsubRunCompleted = wsClient.on('workflow_run_completed', (data: { roleId: string }) => {
			if (data.roleId) {
				const { [data.roleId]: _, ...rest } = runningRoles;
				runningRoles = rest;
			}
			loadRoles();
		});
		const unsubRunFailed = wsClient.on('workflow_run_failed', (data: { roleId: string }) => {
			if (data.roleId) {
				const { [data.roleId]: _, ...rest } = runningRoles;
				runningRoles = rest;
			}
			loadRoles();
		});

		const refreshInterval = setInterval(() => {
			loadLoops();
			loadRoles();
		}, 60000);

		// 1-second ticker for countdown display + wake detection
		let lastTick = Date.now();
		const tickInterval = setInterval(() => {
			const now = Date.now();
			const gap = now - lastTick;
			lastTick = now;
			nowMs = now;

			// If gap > 5s, computer likely woke from sleep — refresh everything
			if (gap > 5000) {
				runningRoles = {}; // clear stale running states
				loadRoles();
				loadLoops();
			}
		}, 1000);

		// Also refresh on visibility change (tab/window refocus)
		function handleVisibility() {
			if (document.visibilityState === 'visible') {
				nowMs = Date.now();
				runningRoles = {};
				loadRoles();
			}
		}
		document.addEventListener('visibilitychange', handleVisibility);

		return () => {
			unsubStatus();
			unsubNotify();
			unsubLane();
			unsubRoleActivated();
			unsubRoleDeactivated();
			unsubRoleInstalled();
			unsubRoleUninstalled();
			unsubRoleUpdated();
			unsubRoleSetup();
			unsubRunStarted();
			unsubActivityUpdate();
			unsubRunCompleted();
			unsubRunFailed();
			clearInterval(refreshInterval);
			clearInterval(tickInterval);
			document.removeEventListener('visibilitychange', handleVisibility);
		};
	});
</script>

<svelte:window onkeydown={handleWindowKeydown} onkeyup={handleWindowKeyup} />

<aside class="sidebar-container">
	<nav class="sidebar-nav">
		<!-- Header with + New button -->
		<div class="sidebar-header">
			<div>
				<div class="sidebar-header-title">Agents</div>
				{#if sidebarRoles.length > 0}
					<div class="sidebar-header-subtitle">{activeCount} of {sidebarRoles.length} active</div>
				{/if}
			</div>
			<div class="flex items-center gap-1">
				<button
					class="sidebar-header-btn"
					onclick={() => goto('/commander')}
					title="Commander — visual agent coordination"
				>
					<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
						<rect x="1" y="1" width="8" height="8" rx="1" /><rect x="15" y="1" width="8" height="8" rx="1" /><rect x="8" y="15" width="8" height="8" rx="1" /><path d="M5 9v2a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V9" /><path d="M12 13v2" />
					</svg>
				</button>
			<div class="relative">
				<button
					class="sidebar-header-btn"
					onclick={(e) => { e.stopPropagation(); const rect = (e.currentTarget as HTMLElement).getBoundingClientRect(); menuPos = { top: rect.bottom + 4, left: rect.left }; showNewBotMenu = !showNewBotMenu; }}
					title="Add new role"
				>
					<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
						<line x1="12" y1="5" x2="12" y2="19" />
						<line x1="5" y1="12" x2="19" y2="12" />
					</svg>
				</button>
				{#if showNewBotMenu}
					<NewBotMenu onClose={() => showNewBotMenu = false} />
				{/if}
			</div>
			</div>
		</div>

		<!-- Assistant — always pinned at top -->
		<button
			class="sidebar-bot-card"
			class:sidebar-item-active={isMyChatActive}
			onclick={selectMyChat}
		>
			<div class="sidebar-bot-icon bg-primary/10">
				<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-primary">
					<path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
				</svg>
			</div>
			<div class="sidebar-bot-info">
				<span class="sidebar-bot-name font-medium">Assistant</span>
				<span class="sidebar-bot-role">Personal AI</span>
			</div>
			{#if notificationCount > 0}
				<span class="sidebar-badge">{notificationCount}</span>
			{/if}
		</button>

		<!-- Roles -->
		{#if sidebarRoles.length > 0}
			<div class="sidebar-divider"></div>
			{#each sidebarRoles as role (role.roleId)}
				{@const c = roleColor(role.name)}
				<!-- svelte-ignore a11y_no_static_element_interactions -->
				<div
					class="sidebar-bot-card"
					class:sidebar-item-active={activeRoleId === role.roleId}
					class:sidebar-bot-paused={!role.isActive}
					onclick={() => selectRole(role)}
					oncontextmenu={(e) => handleContextMenu(e, role)}
				>
					<div class="sidebar-bot-icon {c.bg}">
						<span class="{c.text} font-semibold text-base">{roleInitial(role.name)}</span>
					</div>
					<div class="sidebar-bot-info">
						{#if renamingRoleId === role.roleId}
							<!-- svelte-ignore a11y_autofocus -->
							<input
								bind:this={renameInputEl}
								bind:value={renameValue}
								class="sidebar-rename-input"
								onkeydown={handleRenameKeydown}
								onblur={saveRename}
								onclick={(e) => e.stopPropagation()}
							/>
						{:else}
							<span class="sidebar-bot-name">{role.name}</span>
							{@const subtitle = roleSubtitle(role)}
							{#if subtitle === 'running' || subtitle?.startsWith('Step ')}
								<span class="sidebar-bot-role flex items-center gap-1">
									<span class="loading loading-spinner loading-xs"></span>
									{subtitle === 'running' ? 'Running...' : subtitle}
								</span>
							{:else if subtitle}
								<span class="sidebar-bot-role">{subtitle}</span>
							{/if}
						{/if}
					</div>
					{#if renamingRoleId !== role.roleId}
						{#if role.isActive}
							<span class="sidebar-bot-status sidebar-bot-status-online"></span>
						{:else}
							<svg class="sidebar-bot-paused-icon" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
								<rect x="6" y="4" width="4" height="16" /><rect x="14" y="4" width="4" height="16" />
							</svg>
						{/if}
					{/if}
				</div>
			{/each}
		{/if}

		<!-- Loops with channels -->
		{#if loops.length > 0}
			<div class="sidebar-divider"></div>
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
				{/if}
			{/each}
		{/if}
	</nav>
</aside>

<!-- Context menu -->
{#if contextMenu.visible}
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div class="sidebar-context-backdrop" onclick={closeContextMenu} oncontextmenu={(e) => { e.preventDefault(); closeContextMenu(); }}></div>
	<div class="sidebar-context-menu" style="left: {contextMenu.x}px; top: {contextMenu.y}px;">
		<button onclick={handleCtxRename}>
			<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
				<path d="M17 3a2.83 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z" />
			</svg>
			Rename
		</button>
		<button onclick={handleCtxDuplicate}>
			<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
				<rect width="14" height="14" x="8" y="8" rx="2" ry="2" />
				<path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2" />
			</svg>
			Duplicate
		</button>
		<div class="context-menu-divider"></div>
		<button onclick={handleCtxToggle}>
			{#if contextMenu.role?.isActive}
				<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
					<rect x="6" y="4" width="4" height="16" /><rect x="14" y="4" width="4" height="16" />
				</svg>
				Pause
			{:else}
				<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
					<polygon points="5 3 19 12 5 21 5 3" />
				</svg>
				Resume
			{/if}
		</button>
		<div class="context-menu-divider"></div>
		<button class="context-menu-danger" onclick={handleCtxDelete}>
			<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
				<polyline points="3 6 5 6 21 6" />
				<path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
			</svg>
			Delete
		</button>
	</div>
{/if}

<AlertDialog
	bind:open={showDeleteDialog}
	title="Delete Agent"
	description="Are you sure you want to delete &quot;{deleteTarget?.name}&quot;? This will remove the agent and all its data permanently."
	actionLabel="Delete"
	actionType="danger"
	onAction={confirmDelete}
/>

{#if showSetupWizard}
	<RoleSetupModal
		appId={setupRoleId}
		roleName={setupRoleName}
		roleDescription={setupRoleDescription}
		inputs={{}}
		onComplete={(roleId) => {
			showSetupWizard = false;
			loadRoles();
			onSelectRole(roleId, setupRoleName);
		}}
		onCancel={() => { showSetupWizard = false; }}
	/>
{/if}
