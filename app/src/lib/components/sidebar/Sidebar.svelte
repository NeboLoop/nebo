<script lang="ts">
	import { onMount, tick, getContext } from 'svelte';
	import { goto } from '$app/navigation';
	import { t } from 'svelte-i18n';
	import { getWebSocketClient } from '$lib/websocket/client';
	import { getLoops, getActiveAgents, listAgents, activateAgent, deactivateAgent, deleteAgent, duplicateAgent, updateAgent, listAgentChats, getEntityConfig, updateEntityConfig } from '$lib/api/nebo';
	import type { GetLoopsResponse, LoopChannelEntry, LoopEntry, Chat } from '$lib/api/neboComponents';
	import NewBotMenu from '$lib/components/agent/NewBotMenu.svelte';
	import AlertDialog from '$lib/components/ui/AlertDialog.svelte';
	import AgentSetupModal from '$lib/components/agent-setup/AgentSetupModal.svelte';

	interface SidebarAgent {
		agentId: string;
		name: string;
		description?: string;
		isActive: boolean;
		nextFireAt?: number;
		pinned?: boolean;
		multiChat?: boolean;
	}

	// Access channelState for multi-chat communication with Chat.svelte.
	const channelState = getContext<{
		activeChatId: string;
		onSwitchChat: ((chatId: string) => void) | null;
		onNewChat: (() => void) | null;
	}>('channelState');

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

	// V2 NavC: split agents into workstreams (multi-chat) and single-chat
	const workstreamAgents = $derived(sidebarAgents.filter(a => a.multiChat));
	const singleAgents = $derived(sidebarAgents.filter(a => !a.multiChat));

	// Multi-chat: chat list for the expanded agent.
	let agentChats = $state<Chat[]>([]);
	let agentChatsLoading = $state(false);
	let expandedAgentId = $state('');

	let showAllAgentsDropdown = $state(false);

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

	// V2 agent color palette (matches CSS custom properties --agent-{color}-bg/ink)
	const V2_AGENT_COLORS = [
		'violet', 'green', 'sky', 'amber', 'rose', 'mint', 'slate', 'peach', 'lilac'
	];

	function nameHash(name: string): number {
		let hash = 0;
		for (let i = 0; i < name.length; i++) {
			hash = ((hash << 5) - hash) + name.charCodeAt(i);
			hash |= 0;
		}
		return Math.abs(hash);
	}

	function agentColorName(name: string): string {
		return V2_AGENT_COLORS[nameHash(name) % V2_AGENT_COLORS.length];
	}

	function agentInitial(name: string): string {
		return name.charAt(0).toUpperCase();
	}

	// Pre-defined class strings so Tailwind JIT detects them at build time.
	const AVATAR_BG_CLASSES: Record<string, string> = {
		violet: 'bg-[var(--agent-violet-bg)] text-[var(--agent-violet-ink)]',
		green: 'bg-[var(--agent-green-bg)] text-[var(--agent-green-ink)]',
		sky: 'bg-[var(--agent-sky-bg)] text-[var(--agent-sky-ink)]',
		amber: 'bg-[var(--agent-amber-bg)] text-[var(--agent-amber-ink)]',
		rose: 'bg-[var(--agent-rose-bg)] text-[var(--agent-rose-ink)]',
		mint: 'bg-[var(--agent-mint-bg)] text-[var(--agent-mint-ink)]',
		slate: 'bg-[var(--agent-slate-bg)] text-[var(--agent-slate-ink)]',
		peach: 'bg-[var(--agent-peach-bg)] text-[var(--agent-peach-ink)]',
		lilac: 'bg-[var(--agent-lilac-bg)] text-[var(--agent-lilac-ink)]',
	};

	function avatarClasses(colorName: string): string {
		return AVATAR_BG_CLASSES[colorName] ?? AVATAR_BG_CLASSES.violet;
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

			// Load pin state for all agents.
			const pinResults = await Promise.all(
				agents.map(a => getEntityConfig('agent', a.agentId).catch(() => null))
			);
			for (let i = 0; i < agents.length; i++) {
				const res = pinResults[i] as { config?: { pinned?: boolean; multiChat?: boolean } } | null;
				agents[i].pinned = res?.config?.pinned ?? false;
				agents[i].multiChat = res?.config?.multiChat ?? false;
			}

			// Sort: pinned first, then alphabetical.
			agents.sort((a, b) => {
				if (a.pinned !== b.pinned) return a.pinned ? -1 : 1;
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
		// Auto-expand if multi-chat is enabled.
		if (agent.multiChat) {
			expandedAgentId = agent.agentId;
			loadAgentChatList(agent.agentId);
		}
	}

	function toggleExpand(agent: SidebarAgent) {
		if (expandedAgentId === agent.agentId) {
			expandedAgentId = '';
			agentChats = [];
		} else {
			expandedAgentId = agent.agentId;
			loadAgentChatList(agent.agentId);
		}
	}

	async function loadAgentChatList(agentId: string) {
		agentChatsLoading = true;
		try {
			const res = await listAgentChats(agentId);
			agentChats = res.chats || [];
		} catch {
			agentChats = [];
		}
		agentChatsLoading = false;
	}

	async function togglePin(agent: SidebarAgent) {
		const newPinned = !agent.pinned;
		try {
			await updateEntityConfig('agent', agent.agentId, { pinned: newPinned });
			const idx = sidebarAgents.findIndex(a => a.agentId === agent.agentId);
			if (idx >= 0) {
				sidebarAgents[idx] = { ...sidebarAgents[idx], pinned: newPinned };
			}
		} catch {
			// ignore
		}
	}

	function handleSidebarChatClick(chatId: string) {
		if (channelState?.onSwitchChat) {
			channelState.onSwitchChat(chatId);
		}
	}

	function handleSidebarNewChat() {
		if (channelState?.onNewChat) {
			channelState.onNewChat();
			// Reload chat list after a short delay to get the new chat.
			setTimeout(() => {
				if (expandedAgentId) loadAgentChatList(expandedAgentId);
			}, 500);
		}
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
		const target = deleteTarget;
		showDeleteDialog = false;
		deleteTarget = null;
		// Optimistically remove from sidebar immediately
		sidebarAgents = sidebarAgents.filter(a => a.agentId !== target.agentId);
		try {
			if (target.isActive) await deactivateAgent(target.agentId);
			await deleteAgent(target.agentId);
			if (activeAgentId === target.agentId) {
				goto('/agent/assistant/chat');
			}
		} catch {
			// Restore on failure
			await loadAgents();
		}
	}

	async function toggleMultiChat(agent: SidebarAgent) {
		const newVal = !agent.multiChat;
		try {
			await updateEntityConfig('agent', agent.agentId, { multiChat: newVal });
			const idx = sidebarAgents.findIndex(a => a.agentId === agent.agentId);
			if (idx >= 0) {
				sidebarAgents[idx] = { ...sidebarAgents[idx], multiChat: newVal };
			}
			if (!newVal && expandedAgentId === agent.agentId) {
				expandedAgentId = '';
				agentChats = [];
			}
		} catch {
			// ignore
		}
	}

	// Load chat list when expanded agent changes.
	$effect(() => {
		if (expandedAgentId) {
			loadAgentChatList(expandedAgentId);
		} else {
			agentChats = [];
		}
	});

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
		const unsubAgentUninstalled = wsClient.on('agent_uninstalled', (data: { agentId?: string }) => {
			if (data?.agentId) {
				sidebarAgents = sidebarAgents.filter(a => a.agentId !== data.agentId);
			}
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

<aside class="border-r border-base-300 bg-base-200 flex flex-col min-h-0 flex-1 overflow-hidden">
	<!-- Sidebar Header -->
	<div class="px-4 pt-4 pb-2 flex items-center gap-2.5">
		<div class="text-[15px] font-semibold">{$t('sidebar.agents')}</div>
		<div class="flex-1"></div>
		<button
			class="sidebar-header-btn"
			onclick={() => goto('/commander')}
			title={$t('sidebar.commander')}
		>
			<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
				<circle cx="11" cy="11" r="8" /><line x1="21" y1="21" x2="16.65" y2="16.65" />
			</svg>
		</button>
		<div class="relative">
			<button
				class="sidebar-header-btn"
				onclick={(e) => { e.stopPropagation(); const rect = (e.currentTarget as HTMLElement).getBoundingClientRect(); menuPos = { top: rect.bottom + 4, left: rect.left }; showNewBotMenu = !showNewBotMenu; }}
				title={$t('sidebar.addNewRole')}
			>
				<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
					<line x1="12" y1="5" x2="12" y2="19" /><line x1="5" y1="12" x2="19" y2="12" />
				</svg>
			</button>
			{#if showNewBotMenu}
				<NewBotMenu onClose={() => showNewBotMenu = false} />
			{/if}
		</div>
	</div>

	<div class="flex-1 overflow-auto px-2.5 pt-1 pb-4">
		<!-- Assistant — always at top as a simple row -->
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<div
			class="sidebar-simple-row {isMyChatActive ? 'sidebar-simple-row-active' : ''}"
			onclick={selectMyChat}
		>
			<div class="sidebar-agent-avatar w-7 h-7 rounded-[7px] text-xs {avatarClasses('violet')}">
				A
			</div>
			<div class="sidebar-simple-row-name {isMyChatActive ? 'sidebar-simple-row-name-active' : ''}">
				{$t('sidebar.assistant')}
			</div>
			{#if notificationCount > 0}
				<span class="sidebar-thread-unread">{notificationCount}</span>
			{:else}
				<span class="sidebar-status-dot"></span>
			{/if}
		</div>

		<!-- Workstreams (multi-chat agents) -->
		{#if workstreamAgents.length > 0}
			<div class="sidebar-group-label">
				<svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
					<rect x="2" y="2" width="20" height="8" rx="2" /><rect x="2" y="14" width="20" height="8" rx="2" />
				</svg>
				Workstreams · {workstreamAgents.length}
			</div>
			{#each workstreamAgents as agent (agent.agentId)}
				{@const colorName = agentColorName(agent.name)}
				{@const isActiveAgent = activeAgentId === agent.agentId && activeView === 'agent'}
				{@const isExpanded = expandedAgentId === agent.agentId}
				<div class="sidebar-workstream">
					<!-- Workstream header -->
					<!-- svelte-ignore a11y_no_static_element_interactions -->
					<div
						class="sidebar-ws-head {isActiveAgent && !channelState?.activeChatId ? 'sidebar-ws-head-active' : ''}"
						onclick={() => selectRole(agent)}
						oncontextmenu={(e) => handleContextMenu(e, agent)}
					>
						<div class="sidebar-agent-avatar w-8 h-8 rounded-[9px] text-[13px] {avatarClasses(colorName)}">
							{agentInitial(agent.name)}
						</div>
						<div>
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
								<div class="sidebar-ws-name">{agent.name}</div>
							{/if}
							<div class="sidebar-ws-meta">
								<span class="sidebar-multi-flag">MULTI</span>
								{agentChats.length > 0 && isExpanded ? agentChats.length : ''} chats
								{#if runningAgents[agent.agentId]}
									<span class="loading loading-spinner loading-xs"></span>
								{/if}
							</div>
						</div>
						<button class="sidebar-ws-new-btn" onclick={(e) => { e.stopPropagation(); expandedAgentId = agent.agentId; onSelectAgent(agent.agentId, agent.name); handleSidebarNewChat(); }} title="New chat">
							<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
								<line x1="12" y1="5" x2="12" y2="19" /><line x1="5" y1="12" x2="19" y2="12" />
							</svg>
						</button>
					</div>
					<!-- Thread list -->
					{#if isExpanded || isActiveAgent}
						<div class="sidebar-ws-threads">
							{#if agentChatsLoading && expandedAgentId === agent.agentId}
								<div class="sidebar-thread justify-center">
									<span class="loading loading-spinner loading-xs col-span-full"></span>
								</div>
							{:else if expandedAgentId === agent.agentId}
								{#each agentChats as chat, i (chat.id)}
									{@const chatActive = channelState?.activeChatId === chat.id}
									<!-- svelte-ignore a11y_no_static_element_interactions -->
									<div
										class="sidebar-thread {chatActive ? 'sidebar-thread-active' : ''}"
										onclick={(e) => { e.stopPropagation(); handleSidebarChatClick(chat.id); }}
									>
										<div class="sidebar-thread-dot {chatActive ? 'sidebar-thread-dot-active' : ''}"></div>
										<div class="sidebar-thread-title {chatActive ? 'sidebar-thread-title-active' : ''}">
											{chat.title || `Chat ${i + 1}`}
										</div>
										<div class="sidebar-thread-meta">
											{#if chat.updatedAt}
												{new Date(chat.updatedAt).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}
											{/if}
										</div>
									</div>
								{/each}
							{/if}
						</div>
					{/if}
				</div>
			{/each}
		{/if}

		<!-- Single-chat agents -->
		{#if singleAgents.length > 0}
			<div class="sidebar-group-label">
				<svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
					<path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
				</svg>
				Single-chat · {singleAgents.length}
			</div>
			{#each singleAgents as agent (agent.agentId)}
				{@const colorName = agentColorName(agent.name)}
				{@const isActiveAgent = activeAgentId === agent.agentId && activeView === 'agent'}
				<!-- svelte-ignore a11y_no_static_element_interactions -->
				<div
					class="sidebar-simple-row {isActiveAgent ? 'sidebar-simple-row-active' : ''}"
					onclick={() => selectRole(agent)}
					oncontextmenu={(e) => handleContextMenu(e, agent)}
				>
					<div class="sidebar-agent-avatar w-[26px] h-[26px] rounded-[7px] text-[11px] {avatarClasses(colorName)}">
						{agentInitial(agent.name)}
					</div>
					<div class="sidebar-simple-row-name {isActiveAgent ? 'sidebar-simple-row-name-active' : ''}">
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
							{agent.name}
						{/if}
					</div>
					{#if agent.isActive}
						<span class="sidebar-status-dot {agent.isActive ? '' : 'sidebar-status-dot-paused'}"></span>
					{:else}
						<span class="sidebar-status-dot sidebar-status-dot-paused"></span>
					{/if}
				</div>
			{/each}
		{/if}

		<!-- Loops with channels -->
		{#if loops.length > 0}
			<div class="sidebar-group-label">
				<svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
					<circle cx="12" cy="12" r="10" /><path d="M12 6v6l4 2" />
				</svg>
				Channels
			</div>
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
	</div>
</aside>

<!-- Context menu -->
{#if contextMenu.visible}
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div class="sidebar-context-backdrop" onclick={closeContextMenu} oncontextmenu={(e) => { e.preventDefault(); closeContextMenu(); }}></div>
	<div class="sidebar-context-menu" style:left="{contextMenu.x}px" style:top="{contextMenu.y}px">
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
		<button onclick={() => { if (contextMenu.agent) { togglePin(contextMenu.agent); closeContextMenu(); } }}>
			<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
				<path d="M12 17v5" /><path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16h14v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z" />
			</svg>
			{contextMenu.agent?.pinned ? 'Unpin' : 'Pin to sidebar'}
		</button>
		<button onclick={() => { if (contextMenu.agent) { toggleMultiChat(contextMenu.agent); closeContextMenu(); } }}>
			<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
				<path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
				{#if !contextMenu.agent?.multiChat}
					<line x1="12" y1="8" x2="12" y2="14" /><line x1="9" y1="11" x2="15" y2="11" />
				{/if}
			</svg>
			{contextMenu.agent?.multiChat ? 'Disable multi-chat' : 'Enable multi-chat'}
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
