<script lang="ts">
	import { onMount, tick, getContext } from 'svelte';
	import { goto } from '$app/navigation';
	import { t } from 'svelte-i18n';
	import { getWebSocketClient } from '$lib/websocket/client';
	import { getLoops, getActiveAgents, listAgents, listChats, activateAgent, deactivateAgent, deleteAgent, duplicateAgent, updateAgent, listAgentChats, getEntityConfig, updateEntityConfig } from '$lib/api/nebo';
	import type { GetLoopsResponse, LoopChannelEntry, LoopEntry, Chat } from '$lib/api/neboComponents';
	import NewBotMenu from '$lib/components/agent/NewBotMenu.svelte';
	import AlertDialog from '$lib/components/ui/AlertDialog.svelte';
	import AgentSetupModal from '$lib/components/agent-setup/AgentSetupModal.svelte';
	import AvatarMenu from './AvatarMenu.svelte';

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
		userName = '',
		onSelectMyChat = () => {},
		onSelectChannel = (_channelId: string, _channelName: string, _loopName: string) => {},
		onSelectAgent = (_agentId: string, _agentName: string) => {},
	}: {
		activeChannelId?: string;
		activeAgentId?: string;
		activeView?: string;
		userName?: string;
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

	// Chat-centric sidebar: all chats grouped by time bucket
	interface SidebarChatItem {
		id: string;
		title: string;
		agentId?: string;
		agentName: string;
		updatedAt?: string;
		starred?: boolean;
		unread?: number;
	}

	let allChats = $state<SidebarChatItem[]>([]);
	let agentsExpanded = $state(false);

	// Multi-chat: kept for context menu / management operations.
	let agentChats = $state<Chat[]>([]);
	let agentChatsLoading = $state(false);
	let expandedAgentId = $state('');

	let searchQuery = $state('');
	let railMode = $state(typeof localStorage !== 'undefined' && localStorage.getItem('nebo:sidebar-rail') === '1');

	function toggleRailMode() {
		railMode = !railMode;
		localStorage.setItem('nebo:sidebar-rail', railMode ? '1' : '0');
	}

	const BUCKET_ORDER = ['starred', 'today', 'yesterday', 'previous7', 'older'] as const;
	const BUCKET_LABELS: Record<string, string> = {
		starred: 'Starred',
		today: 'Today',
		yesterday: 'Yesterday',
		previous7: 'Previous 7 days',
		older: 'Older',
	};

	function getBucket(dateVal?: string | number): string {
		if (dateVal == null) return 'older';
		// Backend sends Unix epoch seconds (i64); handle both numbers and ISO strings
		const date = typeof dateVal === 'number'
			? new Date(dateVal * 1000)
			: new Date(dateVal);
		if (isNaN(date.getTime())) return 'older';
		const now = new Date();
		const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
		const yesterday = new Date(today.getTime() - 86400000);
		const weekAgo = new Date(today.getTime() - 7 * 86400000);
		if (date >= today) return 'today';
		if (date >= yesterday) return 'yesterday';
		if (date >= weekAgo) return 'previous7';
		return 'older';
	}

	// Group chats by time bucket, filtered by search
	const groupedChats = $derived.by(() => {
		const q = searchQuery.trim().toLowerCase();
		const filtered = q
			? allChats.filter(c => c.title.toLowerCase().includes(q) || c.agentName.toLowerCase().includes(q))
			: allChats;
		const groups: Record<string, SidebarChatItem[]> = {};
		for (const chat of filtered) {
			const key = chat.starred ? 'starred' : getBucket(chat.updatedAt);
			(groups[key] ||= []).push(chat);
		}
		return groups;
	});

	// Agents preview: sorted by pinned + name, limited to 4 unless expanded
	const topAgents = $derived.by(() => {
		const sorted = [...sidebarAgents].sort((a, b) => {
			if (a.pinned !== b.pinned) return a.pinned ? -1 : 1;
			return a.name.localeCompare(b.name);
		});
		return agentsExpanded ? sorted : sorted.slice(0, 4);
	});

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

	async function loadAllChats() {
		try {
			// Load global chat list + per-agent chats in parallel for proper attribution
			const [globalRes, ...agentResults] = await Promise.all([
				listChats({ pageSize: 100 }).catch(() => ({ chats: [] })),
				...sidebarAgents.map(a =>
					listAgentChats(a.agentId).catch(() => ({ chats: [] }))
				),
			]);

			// Build chatId → agent lookup from per-agent results
			const chatAgentMap = new Map<string, { agentId: string; agentName: string }>();
			for (let i = 0; i < sidebarAgents.length; i++) {
				const agent = sidebarAgents[i];
				for (const c of ((agentResults[i] as any)?.chats ?? [])) {
					chatAgentMap.set(c.id, { agentId: agent.agentId, agentName: agent.name });
				}
			}

			// Process global chat list with agent enrichment
			const raw = (globalRes as any)?.chats ?? [];
			allChats = raw
				.filter((c: any) => {
					const sn: string = c.sessionName || '';
					const id: string = c.id || '';
					// Skip internal subagent chats
					if (sn.includes('subagent:') || id.includes('subagent:')) return false;
					// Skip legacy chats where ID is a session key (contains ':')
					if (id.includes(':')) return false;
					return true;
				})
				.map((c: any) => {
					const agentInfo = chatAgentMap.get(c.id);
					return {
						id: c.id,
						title: c.title || 'Untitled',
						agentId: agentInfo?.agentId,
						agentName: agentInfo?.agentName || 'Assistant',
						updatedAt: c.updatedAt,
					};
				});
		} catch {
			// Fine
		}
	}

	let showAgentPicker = $state(false);

	function handleNewChat() {
		if (channelState?.onNewChat) {
			channelState.onNewChat();
			setTimeout(() => loadAllChats(), 500);
		} else {
			selectMyChat();
		}
	}

	function handleNewChatForAgent(agentId: string, agentName: string) {
		showAgentPicker = false;
		onSelectAgent(agentId, agentName);
		// Give navigation time to mount, then trigger new chat
		setTimeout(() => {
			if (channelState?.onNewChat) {
				channelState.onNewChat();
				setTimeout(() => loadAllChats(), 500);
			}
		}, 300);
	}

	function handleChatSelect(chat: SidebarChatItem) {
		// Set the target chat ID so Chat component loads this specific conversation
		if (channelState) {
			channelState.activeChatId = chat.id;
		}

		if (chat.agentId) {
			// If already viewing this agent, use the switch callback directly
			if (activeAgentId === chat.agentId && activeView === 'agent' && channelState?.onSwitchChat) {
				channelState.onSwitchChat(chat.id);
			} else {
				onSelectAgent(chat.agentId, chat.agentName);
			}
		} else {
			// Assistant chat — use switch if already on assistant, otherwise navigate
			if (activeView === 'companion' && channelState?.onSwitchChat) {
				channelState.onSwitchChat(chat.id);
			} else {
				selectMyChat();
			}
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
		Promise.all([loadLoops(), loadAgents()]).then(() => { loadAllChats(); initialLoadDone = true; });

		const wsClient = getWebSocketClient();

		const unsubStatus = wsClient.onStatus((status) => {
			if (status === 'connected' && initialLoadDone) {
				loadLoops();
				loadAgents();
				loadAllChats();
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
			setTimeout(() => { loadAgents(); loadAllChats(); }, 300);
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
		const unsubChatComplete = wsClient.on('chat_complete', () => {
			loadAllChats();
		});
		const unsubTitleUpdated = wsClient.on('chat_title_updated', (data: { chatId?: string; title?: string }) => {
			if (data?.chatId && data?.title) {
				allChats = allChats.map(c => c.id === data.chatId ? { ...c, title: data.title! } : c);
			}
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
			loadAllChats();
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
				loadAllChats();
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
			unsubChatComplete();
			unsubTitleUpdated();
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

<aside class="border-r border-base-300 bg-base-200 flex flex-col h-full min-h-0 overflow-hidden transition-all duration-200 {railMode ? 'w-[58px] min-w-[58px]' : 'w-[260px] min-w-[260px]'}">
	{#if railMode}
		<!-- ═══ Rail mode ═══ -->
		<div class="flex flex-col items-center py-2.5 gap-1">
			<button class="w-[38px] h-[38px] rounded-[9px] grid place-items-center text-base-content/60 hover:bg-base-300 cursor-pointer" onclick={toggleRailMode} title="Expand sidebar">
				<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="M9 4v16"/></svg>
			</button>
			<button class="w-[38px] h-[38px] rounded-[9px] grid place-items-center text-base-content/60 hover:bg-base-300 cursor-pointer" onclick={handleNewChat} title="New chat">
				<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg>
			</button>
			<button class="w-[38px] h-[38px] rounded-[9px] grid place-items-center text-base-content/60 hover:bg-base-300 cursor-pointer" onclick={() => goto('/commander')} title="Search (⌘K)">
				<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/></svg>
			</button>
			<div class="w-[22px] h-px bg-base-300 my-1.5"></div>
			<button class="w-[38px] h-[38px] rounded-[9px] grid place-items-center cursor-pointer {isMyChatActive ? 'bg-primary/10 text-primary' : 'text-base-content/60 hover:bg-base-300'}" onclick={selectMyChat} title="Chats">
				<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M21 11.5a8.38 8.38 0 0 1-.9 3.8 8.5 8.5 0 0 1-7.6 4.7 8.38 8.38 0 0 1-3.8-.9L3 21l1.9-5.7a8.38 8.38 0 0 1-.9-3.8 8.5 8.5 0 0 1 4.7-7.6 8.38 8.38 0 0 1 3.8-.9h.5a8.48 8.48 0 0 1 8 8v.5z"/></svg>
			</button>
			<button class="w-[38px] h-[38px] rounded-[9px] grid place-items-center text-base-content/60 hover:bg-base-300 cursor-pointer" onclick={() => goto('/marketplace')} title="Marketplace">
				<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M6 2 3 6v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2V6l-3-4Z"/><path d="M3 6h18"/><path d="M16 10a4 4 0 0 1-8 0"/></svg>
			</button>
			<div class="w-[22px] h-px bg-base-300 my-1.5"></div>
			{#each sidebarAgents.slice(0, 6) as agent (agent.agentId)}
				{@const colorName = agentColorName(agent.name)}
				{@const isActiveAgent = activeAgentId === agent.agentId && activeView === 'agent'}
				<!-- svelte-ignore a11y_no_static_element_interactions -->
				<div
					class="w-7 h-7 rounded-[7px] grid place-items-center text-xs font-bold relative cursor-pointer {avatarClasses(colorName)}"
					onclick={() => selectRole(agent)}
					oncontextmenu={(e) => handleContextMenu(e, agent)}
					title={agent.name}
				>
					{agentInitial(agent.name)}
					{#if isActiveAgent}
						<span class="absolute -top-0.5 -right-0.5 w-2 h-2 rounded-full bg-primary border-2 border-base-200"></span>
					{/if}
				</div>
			{/each}
		</div>
	{:else}
		<!-- ═══ Expanded mode ═══ -->
		<div class="px-2.5 pt-2.5 pb-1.5 flex flex-col gap-0.5">
			<!-- Toggle row -->
			<div class="flex items-center px-1 pb-1">
				<button class="w-[30px] h-[30px] rounded-[7px] grid place-items-center text-base-content/60 hover:bg-base-300 cursor-pointer" onclick={toggleRailMode} title="Collapse sidebar">
					<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="M9 4v16"/></svg>
				</button>
				<div class="flex-1"></div>
				<div class="relative">
					<button
						class="w-[30px] h-[30px] rounded-[7px] grid place-items-center text-base-content/60 hover:bg-base-300 cursor-pointer"
						onclick={(e) => { e.stopPropagation(); const rect = (e.currentTarget as HTMLElement).getBoundingClientRect(); menuPos = { top: rect.bottom + 4, left: rect.left }; showNewBotMenu = !showNewBotMenu; }}
						title="New agent"
					>
						<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="9" cy="7" r="4"/><path d="M3 21v-1a6 6 0 0 1 6-6h2"/><line x1="19" y1="11" x2="19" y2="17"/><line x1="16" y1="14" x2="22" y2="14"/></svg>
					</button>
					{#if showNewBotMenu}
						<NewBotMenu onClose={() => showNewBotMenu = false} />
					{/if}
				</div>
			</div>
			<!-- + New chat with agent picker -->
			<div class="relative">
				<!-- svelte-ignore a11y_no_static_element_interactions -->
				<div
					class="flex items-center gap-2.5 px-2.5 py-2 rounded-lg text-[13.5px] text-base-content cursor-pointer font-medium hover:bg-base-300"
					onclick={() => { if (sidebarAgents.length > 0) showAgentPicker = !showAgentPicker; else handleNewChat(); }}
				>
					<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg>
					New chat
					{#if sidebarAgents.length > 0}
						<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" class="ml-auto text-base-content/40"><polyline points="6 9 12 15 18 9"/></svg>
					{/if}
				</div>
				{#if showAgentPicker}
					<!-- svelte-ignore a11y_no_static_element_interactions -->
					<div class="fixed inset-0 z-[29]" onclick={() => showAgentPicker = false}></div>
					<div class="absolute left-0 top-full mt-1 w-[220px] bg-base-100 border border-base-300 rounded-xl shadow-lg z-30 p-1.5">
						<div class="text-[10.5px] font-semibold tracking-[0.8px] text-base-content/40 uppercase px-2.5 pt-1.5 pb-1">New chat with</div>
						<button
							class="w-full flex items-center gap-2.5 px-2.5 py-[7px] rounded-lg text-[13.5px] text-base-content hover:bg-base-300 transition-colors text-left cursor-pointer"
							onclick={() => { showAgentPicker = false; handleNewChat(); }}
						>
							<div class="w-4 h-4 rounded grid place-items-center text-[9.5px] font-bold bg-primary/10 text-primary">N</div>
							Assistant
						</button>
						{#each sidebarAgents as agent (agent.agentId)}
							{@const colorName = agentColorName(agent.name)}
							<button
								class="w-full flex items-center gap-2.5 px-2.5 py-[7px] rounded-lg text-[13.5px] text-base-content hover:bg-base-300 transition-colors text-left cursor-pointer"
								onclick={() => handleNewChatForAgent(agent.agentId, agent.name)}
							>
								<div class="w-4 h-4 rounded grid place-items-center text-[9.5px] font-bold {avatarClasses(colorName)}">{agentInitial(agent.name)}</div>
								{agent.name}
							</button>
						{/each}
					</div>
				{/if}
			</div>
			<!-- Search chats -->
			<div class="flex items-center gap-2 px-2.5 py-2 rounded-lg">
				<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-base-content/40 shrink-0"><circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/></svg>
				<input
					type="text"
					bind:value={searchQuery}
					placeholder="Search chats…"
					class="border-0 outline-none bg-transparent flex-1 text-[13.5px] text-base-content placeholder:text-base-content/40"
				/>
			</div>
			<!-- Marketplace link -->
			<a
				href="/marketplace"
				class="flex items-center gap-2.5 px-2.5 py-2 rounded-lg text-[13.5px] text-base-content/60 no-underline hover:bg-base-300 hover:text-base-content transition-colors"
			>
				<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M6 2 3 6v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2V6l-3-4Z"/><path d="M3 6h18"/><path d="M16 10a4 4 0 0 1-8 0"/></svg>
				Marketplace
			</a>
		</div>

		<!-- Body -->
		<div class="flex-1 overflow-auto px-2.5 pt-2 pb-4">
			<!-- Agents preview (hidden when searching) -->
			{#if !searchQuery.trim()}
				<div class="mb-1">
					<div class="flex items-center text-[10.5px] font-semibold tracking-[0.8px] text-base-content/40 uppercase px-2.5 pt-3.5 pb-1">
						<span>Agents</span>
						<a href="/marketplace" class="ml-auto text-[10.5px] text-base-content/40 cursor-pointer font-medium tracking-normal normal-case hover:text-base-content/60">Browse →</a>
					</div>
					{#each topAgents as agent (agent.agentId)}
						{@const colorName = agentColorName(agent.name)}
						{@const isActiveAgent = activeAgentId === agent.agentId && activeView === 'agent'}
						<!-- svelte-ignore a11y_no_static_element_interactions -->
						<div
							class="grid grid-cols-[18px_1fr_auto] items-center gap-2.5 py-[7px] px-2.5 rounded-lg cursor-pointer {isActiveAgent ? 'bg-primary/10' : 'hover:bg-base-300'}"
							onclick={() => selectRole(agent)}
							oncontextmenu={(e) => handleContextMenu(e, agent)}
						>
							<div class="w-4 h-4 rounded grid place-items-center text-[9.5px] font-bold {avatarClasses(colorName)}">
								{agentInitial(agent.name)}
							</div>
							<div class="text-[13.5px] truncate {isActiveAgent ? 'text-primary font-medium' : 'text-base-content'}">
								{#if renamingAgentId === agent.agentId}
									<!-- svelte-ignore a11y_autofocus -->
									<input
										bind:this={renameInputEl}
										bind:value={renameValue}
										class="bg-transparent border border-base-300 rounded px-1 py-0.5 text-[13px] w-full outline-none"
										onkeydown={handleRenameKeydown}
										onblur={saveRename}
										onclick={(e) => e.stopPropagation()}
									/>
								{:else}
									{agent.name}
								{/if}
							</div>
							<div>
								{#if !agent.isActive}
									<span class="text-[10px] text-base-content/40">paused</span>
								{/if}
							</div>
						</div>
					{/each}
					{#if sidebarAgents.length > 4}
						<!-- svelte-ignore a11y_no_static_element_interactions -->
						<div
							class="grid grid-cols-[18px_1fr] items-center gap-2.5 py-[7px] px-2.5 rounded-lg cursor-pointer text-base-content/40 hover:bg-base-300"
							onclick={() => agentsExpanded = !agentsExpanded}
						>
							<div class="grid place-items-center">
								<svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="transition-transform duration-150 {agentsExpanded ? 'rotate-180' : ''}"><polyline points="6 9 12 15 18 9"/></svg>
							</div>
							<div class="text-[12.5px]">{agentsExpanded ? 'Show fewer' : `${sidebarAgents.length - 4} more`}</div>
						</div>
					{/if}
				</div>
			{/if}

			<!-- Chats grouped by time -->
			{#each BUCKET_ORDER as bucket}
				{@const bucketChats = groupedChats[bucket]}
				{#if bucketChats && bucketChats.length > 0}
					<div class="text-[10.5px] font-semibold tracking-[0.8px] text-base-content/40 uppercase px-2.5 pt-3.5 pb-1">{BUCKET_LABELS[bucket]}</div>
					{#each bucketChats as chat (chat.id)}
						{@const colorName = agentColorName(chat.agentName)}
						{@const isActiveChat = channelState?.activeChatId === chat.id}
						<!-- svelte-ignore a11y_no_static_element_interactions -->
						<div
							class="grid grid-cols-[18px_1fr_auto] items-center gap-2.5 py-[7px] px-2.5 rounded-lg cursor-pointer {isActiveChat ? 'bg-primary/10' : 'hover:bg-base-300'}"
							onclick={() => handleChatSelect(chat)}
						>
							<div class="w-4 h-4 rounded grid place-items-center text-[9.5px] font-bold {avatarClasses(colorName)}" title={chat.agentName}>
								{chat.agentName.charAt(0).toUpperCase()}
							</div>
							<div class="text-[13.5px] truncate {isActiveChat ? 'text-primary font-medium' : 'text-base-content'}">
								{chat.title}
							</div>
							<div class="flex items-center gap-1.5">
								{#if chat.starred}
									<span class="text-[11px] text-warning">★</span>
								{/if}
								{#if chat.unread}
									<span class="text-[10px] font-semibold bg-primary text-white rounded-full px-1.5 py-px">{chat.unread}</span>
								{/if}
							</div>
						</div>
					{/each}
				{/if}
			{/each}

			<!-- Empty state -->
			{#if allChats.length === 0 && !searchQuery.trim()}
				<div class="text-center text-[13px] text-base-content/40 py-8">
					No chats yet. Start a conversation!
				</div>
			{/if}

			<!-- Loops with channels -->
			{#if loops.length > 0}
				<div class="text-[10.5px] font-semibold tracking-[0.8px] text-base-content/40 uppercase px-2.5 pt-3.5 pb-1">
					Channels
				</div>
				{#each loops as loop (loop.id)}
					<!-- svelte-ignore a11y_no_static_element_interactions -->
					<div
						class="grid grid-cols-[18px_1fr] items-center gap-2.5 py-[7px] px-2.5 rounded-lg cursor-pointer text-base-content hover:bg-base-300"
						onclick={() => toggleLoop(loop.id)}
					>
						<svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="transition-transform duration-150 {expandedLoops.has(loop.id) ? 'rotate-90' : ''}"><polyline points="9 18 15 12 9 6"/></svg>
						<span class="text-[13px] font-medium">{loop.name || loop.id}</span>
					</div>
					{#if expandedLoops.has(loop.id) && loop.channels}
						{#each loop.channels as channel (channel.channelId)}
							<!-- svelte-ignore a11y_no_static_element_interactions -->
							<div
								class="grid grid-cols-[18px_1fr] items-center gap-2.5 py-[7px] px-2.5 pl-5 rounded-lg cursor-pointer {activeChannelId === channel.channelId ? 'bg-primary/10 text-primary' : 'text-base-content/60 hover:bg-base-300'}"
								onclick={() => selectChannel(channel, loop.name)}
							>
								<span class="text-[13px] font-medium">#</span>
								<span class="text-[13px]">{channel.channelName}</span>
							</div>
						{/each}
					{/if}
				{/each}
			{/if}
		</div>
		<!-- Avatar menu (bottom) -->
		<div class="px-2.5 pb-2.5 pt-1 border-t border-base-300">
			<AvatarMenu {userName} />
		</div>
	{/if}
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
