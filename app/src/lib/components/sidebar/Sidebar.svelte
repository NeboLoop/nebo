<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { goto } from '$app/navigation';
	import { t } from 'svelte-i18n';
	import { getWebSocketClient } from '$lib/websocket/client';
	import { getLoops, getActiveAgents, listAgents, activateAgent, deactivateAgent, deleteAgent, duplicateAgent, updateAgent } from '$lib/api/nebo';
	import type { GetLoopsResponse, LoopChannelEntry, LoopEntry } from '$lib/api/neboComponents';
	import NewBotMenu from '$lib/components/agent/NewBotMenu.svelte';
	import AlertDialog from '$lib/components/ui/AlertDialog.svelte';
	import AgentSetupModal from '$lib/components/agent-setup/AgentSetupModal.svelte';

	interface SidebarAgent {
		agentId: string;
		name: string;
		description?: string;
		isActive: boolean;
		nextFireAt?: number;
	}

	let {
		activeChannelId = $bindable(''),
		activeAgentId = '',
		activeView = 'agent',
		onSelectMyChat = () => {},
		onSelectChannel = (_channelId: string, _channelName: string, _loopName: string) => {},
		onSelectAgent = (_agentId: string, _agentName: string) => {},
	}: {
		activeChannelId?: string;
		activeAgentId?: string;
		activeView?: string;
		onSelectMyChat?: () => void;
		onSelectChannel?: (channelId: string, channelName: string, loopName: string) => void;
		onSelectAgent?: (agentId: string, agentName: string) => void;
	} = $props();

	let loops: LoopEntry[] = $state([]);
	let expandedLoops: Set<string> = $state(new Set());
	let sidebarAgents: SidebarAgent[] = $state([]);
	let notificationCount = $state(0);
	let showNewBotMenu = $state(false);
	let menuPos = $state({ top: 0, left: 0 });

	// Setup wizard state
	let showSetupWizard = $state(false);
	let setupAgentId = $state('');
	let setupAgentName = $state('');
	let setupAgentDescription = $state('');

	// Context menu state
	let contextMenu = $state<{ visible: boolean; x: number; y: number; agent: SidebarAgent | null }>({
		visible: false, x: 0, y: 0, agent: null,
	});
	let renamingAgentId = $state('');
	let renameValue = $state('');
	let renameInputEl: HTMLInputElement | undefined = $state();
	let showDeleteDialog = $state(false);
	let deleteTarget = $state<SidebarAgent | null>(null);

	const isMyChatActive = $derived(activeView === 'companion');

	const activeCount = $derived(sidebarAgents.filter(r => r.isActive).length);

	// Live countdown ticker
	let nowMs = $state(Date.now());
	let runningAgents = $state<Record<string, string>>({}); // agentId → status text

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

	function agentSubtitle(agent: SidebarAgent): string {
		if (!agent.isActive) return $t('common.paused');
		const running = runningAgents[agent.agentId];
		if (running) return running; // "running" or "Step 2 of 3"
		if (agent.nextFireAt) {
			const cd = formatCountdown(agent.nextFireAt);
			if (cd) return cd;
			// Countdown expired but no running state — show description while we wait for refresh
		}
		return agent.description || '';
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

	function agentColor(name: string) {
		return BOT_COLORS[nameHash(name) % BOT_COLORS.length];
	}

	function agentInitial(name: string): string {
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

	async function loadAgents() {
		try {
			const [allRes, activeRes] = await Promise.all([
				listAgents().catch(() => null),
				getActiveAgents().catch(() => null),
			]);

			const activeAgentList = activeRes?.agents ?? [];
			const activeMap = new Map(activeAgentList.map(r => [r.agentId, r]));
			const agents: SidebarAgent[] = [];

			// DB agents
			if (allRes?.agents) {
				for (const r of allRes.agents) {
					const active = activeMap.get(r.id);
					agents.push({
						agentId: r.id,
						name: r.name,
						description: r.description || undefined,
						isActive: !!active,
						nextFireAt: (active as any)?.nextFireAt ?? undefined,
					});
				}
			}

			// Filesystem-only agents — only show if they have an active UUID
			if (allRes?.filesystemAgents) {
				for (const r of allRes.filesystemAgents) {
					const matchedActive = activeAgentList.find(a => a.name === r.name);
					if (matchedActive && !agents.some(existing => existing.name === r.name)) {
						agents.push({
							agentId: matchedActive.agentId,
							name: r.name,
							description: r.description || undefined,
							isActive: true,
							nextFireAt: (matchedActive as any)?.nextFireAt ?? undefined,
						});
					}
				}
			}

			// Sort: active first, then alphabetical
			agents.sort((a, b) => {
				if (a.isActive !== b.isActive) return a.isActive ? -1 : 1;
				return a.name.localeCompare(b.name);
			});

			sidebarAgents = agents;
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

	function selectRole(agent: SidebarAgent) {
		if (contextMenu.visible || Date.now() - contextMenuClosedAt < 200) return;
		onSelectAgent(agent.agentId, agent.name);
	}

	// ── Context menu ──

	function handleContextMenu(e: MouseEvent, agent: SidebarAgent) {
		e.preventDefault();
		(e.currentTarget as HTMLElement)?.blur();
		contextMenu = { visible: true, x: e.clientX, y: e.clientY, agent };
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
		contextMenu = { visible: false, x: 0, y: 0, agent: null };
		contextMenuClosedAt = Date.now();
	}

	async function handleCtxRename() {
		if (!contextMenu.agent) return;
		const agent = contextMenu.agent;
		closeContextMenu();
		renamingAgentId = agent.agentId;
		renameValue = agent.name;
		await tick();
		renameInputEl?.select();
	}

	async function saveRename() {
		const trimmed = renameValue.trim();
		const agent = sidebarAgents.find(r => r.agentId === renamingAgentId);
		if (!trimmed || !agent || trimmed === agent.name) {
			renamingAgentId = '';
			return;
		}
		try {
			await updateAgent(renamingAgentId, { name: trimmed });
			// Update local list immediately
			const idx = sidebarAgents.findIndex(r => r.agentId === renamingAgentId);
			if (idx >= 0) {
				sidebarAgents[idx] = { ...sidebarAgents[idx], name: trimmed };
			}
		} catch {
			// revert
		}
		renamingAgentId = '';
	}

	function cancelRename() {
		renamingAgentId = '';
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
		if (!contextMenu.agent) return;
		const agentId = contextMenu.agent.agentId;
		closeContextMenu();
		try {
			const res = await duplicateAgent(agentId);
			if (res?.agent) {
				await loadAgents();
				onSelectAgent(res.agent.id, res.agent.name);
				goto(`/agent/persona/${res.agent.id}/chat`);
			}
		} catch {
			// ignore
		}
	}

	async function handleCtxToggle() {
		if (!contextMenu.agent) return;
		const agent = contextMenu.agent;
		closeContextMenu();
		try {
			if (agent.isActive) {
				await deactivateAgent(agent.agentId);
			} else {
				await activateAgent(agent.agentId);
			}
			await loadAgents();
		} catch {
			// ignore
		}
	}

	function handleCtxDelete() {
		if (!contextMenu.agent) return;
		deleteTarget = contextMenu.agent;
		closeContextMenu();
		showDeleteDialog = true;
	}

	async function confirmDelete() {
		if (!deleteTarget) return;
		showDeleteDialog = false;
		try {
			if (deleteTarget.isActive) await deactivateAgent(deleteTarget.agentId);
			await deleteAgent(deleteTarget.agentId);
			if (activeAgentId === deleteTarget.agentId) {
				goto('/agents');
			}
			await loadAgents();
		} catch {
			// ignore
		}
		deleteTarget = null;
	}

	onMount(() => {
		let initialLoadDone = false;
		Promise.all([loadLoops(), loadAgents()]).then(() => { initialLoadDone = true; });

		const wsClient = getWebSocketClient();

		const unsubStatus = wsClient.onStatus((status) => {
			if (status === 'connected' && initialLoadDone) {
				loadLoops();
				loadAgents();
			}
		});

		const unsubNotify = wsClient.on<{ content: string }>('notification', (data) => {
			if (data) notificationCount++;
		});

		const unsubLane = wsClient.on('lane_update', () => {
			loadLoops();
		});

		const unsubAgentActivated = wsClient.on('agent_activated', () => {
			loadAgents();
		});
		const unsubAgentDeactivated = wsClient.on('agent_deactivated', () => {
			loadAgents();
		});
		const unsubAgentInstalled = wsClient.on('agent_installed', () => {
			loadAgents();
		});
		const unsubAgentUninstalled = wsClient.on('agent_uninstalled', () => {
			loadAgents();
		});
		const unsubAgentUpdated = wsClient.on('agent_updated', () => {
			loadAgents();
		});
		const unsubAgentSetup = wsClient.on('agent_setup', (data: { agentId: string; agentName: string; agentDescription: string }) => {
			setupAgentId = data.agentId;
			setupAgentName = data.agentName;
			setupAgentDescription = data.agentDescription || '';
			showSetupWizard = true;
		});
		const unsubRunStarted = wsClient.on('workflow_run_started', (data: { agentId: string }) => {
			if (data.agentId) {
				runningAgents = { ...runningAgents, [data.agentId]: 'running' };
			}
		});
		const unsubActivityUpdate = wsClient.on('workflow_activity_update', (data: { agentId: string; step: number; totalSteps: number }) => {
			if (data.agentId) {
				runningAgents = { ...runningAgents, [data.agentId]: $t('sidebar.stepProgress', { values: { step: data.step, total: data.totalSteps } }) };
			}
		});
		const unsubRunCompleted = wsClient.on('workflow_run_completed', (data: { agentId: string }) => {
			if (data.agentId) {
				const { [data.agentId]: _, ...rest } = runningAgents;
				runningAgents = rest;
			}
			loadAgents();
		});
		const unsubRunFailed = wsClient.on('workflow_run_failed', (data: { agentId: string }) => {
			if (data.agentId) {
				const { [data.agentId]: _, ...rest } = runningAgents;
				runningAgents = rest;
			}
			loadAgents();
		});

		const refreshInterval = setInterval(() => {
			loadLoops();
			loadAgents();
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
				runningAgents = {}; // clear stale running states
				loadAgents();
				loadLoops();
			}
		}, 1000);

		// Also refresh on visibility change (tab/window refocus)
		function handleVisibility() {
			if (document.visibilityState === 'visible') {
				nowMs = Date.now();
				runningAgents = {};
				loadAgents();
			}
		}
		document.addEventListener('visibilitychange', handleVisibility);

		return () => {
			unsubStatus();
			unsubNotify();
			unsubLane();
			unsubAgentActivated();
			unsubAgentDeactivated();
			unsubAgentInstalled();
			unsubAgentUninstalled();
			unsubAgentUpdated();
			unsubAgentSetup();
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
	<!-- Expand button — visible only in rail mode -->
	<button class="sidebar-expand-btn" onclick={() => window.dispatchEvent(new CustomEvent('nebo:focus-mode', { detail: false }))} title="Expand sidebar">
		<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
			<line x1="3" y1="12" x2="21" y2="12" /><line x1="3" y1="6" x2="21" y2="6" /><line x1="3" y1="18" x2="21" y2="18" />
		</svg>
	</button>
	<nav class="sidebar-nav">
		<!-- Header with + New button -->
		<div class="sidebar-header">
			<div>
				<div class="sidebar-header-title">{$t('sidebar.agents')}</div>
				{#if sidebarAgents.length > 0}
					<div class="sidebar-header-subtitle">{$t('sidebar.activeCount', { values: { active: activeCount, total: sidebarAgents.length } })}</div>
				{/if}
			</div>
			<div class="flex items-center gap-1">
				<button
					class="sidebar-header-btn"
					onclick={() => goto('/commander')}
					title={$t('sidebar.commander')}
				>
					<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
						<rect x="1" y="1" width="8" height="8" rx="1" /><rect x="15" y="1" width="8" height="8" rx="1" /><rect x="8" y="15" width="8" height="8" rx="1" /><path d="M5 9v2a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V9" /><path d="M12 13v2" />
					</svg>
				</button>
			<div class="relative">
				<button
					class="sidebar-header-btn"
					onclick={(e) => { e.stopPropagation(); const rect = (e.currentTarget as HTMLElement).getBoundingClientRect(); menuPos = { top: rect.bottom + 4, left: rect.left }; showNewBotMenu = !showNewBotMenu; }}
					title={$t('sidebar.addNewRole')}
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
				<span class="sidebar-bot-name font-medium">{$t('sidebar.assistant')}</span>
				<span class="sidebar-bot-agent">{$t('sidebar.personalAI')}</span>
			</div>
			{#if notificationCount > 0}
				<span class="sidebar-badge">{notificationCount}</span>
			{/if}
		</button>

		<!-- Agents -->
		{#if sidebarAgents.length > 0}
			<div class="sidebar-divider"></div>
			{#each sidebarAgents as agent (agent.agentId)}
				{@const c = agentColor(agent.name)}
				<!-- svelte-ignore a11y_no_static_element_interactions -->
				<div
					class="sidebar-bot-card"
					class:sidebar-item-active={activeAgentId === agent.agentId}
					class:sidebar-bot-paused={!agent.isActive}
					onclick={() => selectRole(agent)}
					oncontextmenu={(e) => handleContextMenu(e, agent)}
				>
					<div class="sidebar-bot-icon {c.bg}">
						<span class="{c.text} font-semibold text-base">{agentInitial(agent.name)}</span>
					</div>
					<div class="sidebar-bot-info">
						{#if renamingAgentId === agent.agentId}
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
							<span class="sidebar-bot-name">{agent.name}</span>
							{@const subtitle = agentSubtitle(agent)}
							{#if runningAgents[agent.agentId]}
								<span class="sidebar-bot-agent flex items-center gap-1">
									<span class="loading loading-spinner loading-xs"></span>
									{runningAgents[agent.agentId] === 'running' ? $t('common.running') : subtitle}
								</span>
							{:else if subtitle}
								<span class="sidebar-bot-agent">{subtitle}</span>
							{/if}
						{/if}
					</div>
					{#if renamingAgentId !== agent.agentId}
						{#if agent.isActive}
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
			{$t('sidebar.rename')}
		</button>
		<button onclick={handleCtxDuplicate}>
			<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
				<rect width="14" height="14" x="8" y="8" rx="2" ry="2" />
				<path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2" />
			</svg>
			{$t('sidebar.duplicate')}
		</button>
		<div class="context-menu-divider"></div>
		<button onclick={handleCtxToggle}>
			{#if contextMenu.agent?.isActive}
				<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
					<rect x="6" y="4" width="4" height="16" /><rect x="14" y="4" width="4" height="16" />
				</svg>
				{$t('sidebar.pause')}
			{:else}
				<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
					<polygon points="5 3 19 12 5 21 5 3" />
				</svg>
				{$t('sidebar.resume')}
			{/if}
		</button>
		<div class="context-menu-divider"></div>
		<button class="context-menu-danger" onclick={handleCtxDelete}>
			<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
				<polyline points="3 6 5 6 21 6" />
				<path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
			</svg>
			{$t('common.delete')}
		</button>
	</div>
{/if}

<AlertDialog
	bind:open={showDeleteDialog}
	title={$t('sidebar.deleteAgent')}
	description={$t('sidebar.deleteAgentConfirm', { values: { name: deleteTarget?.name ?? '' } })}
	actionLabel={$t('common.delete')}
	actionType="danger"
	onAction={confirmDelete}
/>

{#if showSetupWizard}
	<AgentSetupModal
		appId={setupAgentId}
		agentName={setupAgentName}
		agentDescription={setupAgentDescription}
		inputs={{}}
		onComplete={(agentId) => {
			showSetupWizard = false;
			loadAgents();
			onSelectAgent(agentId, setupAgentName);
		}}
		onCancel={() => { showSetupWizard = false; }}
	/>
{/if}
