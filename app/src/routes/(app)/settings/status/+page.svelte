<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import {
		Activity,
		Server,
		Cpu,
		Clock,
		RefreshCw,
		Wifi,
		WifiOff,
		Database,
		Users,
		Layers,
		ArrowUpCircle
	} from 'lucide-svelte';
	import { getWebSocketClient, type ConnectionStatus } from '$lib/websocket/client';
	import { auth } from '$lib/stores/auth';
	import * as api from '$lib/api/nebo';

	let wsConnected = $state(false);
	let wsReconnecting = $state(false);

	interface Agent {
		id: string;
		name: string;
		status: 'online' | 'offline' | 'busy';
		connected_at?: string;
		last_activity?: string;
		current_task?: string;
	}

	interface SystemStatus {
		mcpServer: 'online' | 'offline';
		database: 'online' | 'offline';
		websocket: 'online' | 'offline';
		uptime: string;
		memoryUsage: string;
		activeSessions: number;
		connectedClients: number;
	}

	let agents = $state<Agent[]>([]);
	let systemStatus = $state<SystemStatus>({
		mcpServer: 'offline',
		database: 'offline',
		websocket: 'offline',
		uptime: '0s',
		memoryUsage: '0MB',
		activeSessions: 0,
		connectedClients: 0
	});
	interface LaneTaskInfo {
		id: string;
		description: string;
		enqueuedAt: number;
		startedAt?: number;
	}

	interface LaneStats {
		lane: string;
		queued: number;
		active: number;
		maxConcurrent: number;
		activeTasks?: LaneTaskInfo[];
		queuedTasks?: LaneTaskInfo[];
	}

	import type { AgentSettings, UpdateCheckResponse } from '$lib/api/neboComponents';

	let isLoading = $state(true);
	let lanes = $state<Record<string, LaneStats>>({});

	// Update settings
	let autoUpdate = $state(true);
	let updateCheckResult = $state<UpdateCheckResponse | null>(null);
	let currentSettings = $state<AgentSettings | null>(null);
	let isCheckingUpdate = $state(false);
	let refreshInterval: ReturnType<typeof setInterval>;
	let unsubscribers: (() => void)[] = [];

	const laneOrder = ['main', 'events', 'subagent', 'heartbeat', 'comm', 'nested'];
	const laneLabels: Record<string, string> = {
		main: 'Main',
		events: 'Events',
		subagent: 'Sub-agents',
		heartbeat: 'Heartbeat',
		comm: 'Communication',
		nested: 'Nested'
	};

	const sortedLanes = $derived(
		laneOrder
			.filter((l) => lanes[l])
			.map((l) => lanes[l])
	);

	onMount(async () => {
		const client = getWebSocketClient();

		unsubscribers.push(
			client.onStatus((status: ConnectionStatus) => {
				wsConnected = status === 'connected';
				wsReconnecting = status === 'connecting';
			})
		);

		unsubscribers.push(
			client.on('status_update', handleStatusUpdate),
			client.on('agent_connected', handleAgentConnected),
			client.on('agent_disconnected', handleAgentDisconnected),
			client.on('lane_update', handleLaneUpdate),
			client.on('pong', () => {})
		);

		await Promise.all([loadStatus(), loadLanes(), loadUpdateSettings()]);
		refreshInterval = setInterval(loadStatus, 10000);
	});

	onDestroy(() => {
		if (refreshInterval) clearInterval(refreshInterval);
		unsubscribers.forEach((unsub) => unsub());
	});

	function handleStatusUpdate(data: Record<string, unknown>) {
		if (data) {
			systemStatus = {
				...systemStatus,
				...(data as Partial<SystemStatus>)
			};
		}
	}

	function handleAgentConnected(data: Record<string, unknown>) {
		if (data && typeof data.id === 'string') {
			const agent: Agent = {
				id: data.id as string,
				name: (data.name as string) || '',
				status: (data.status as 'online' | 'offline' | 'busy') || 'online',
				connected_at: data.connected_at as string | undefined,
				last_activity: data.last_activity as string | undefined,
				current_task: data.current_task as string | undefined
			};
			if (!agents.find((a) => a.id === agent.id)) {
				agents = [...agents, agent];
			}
		}
	}

	function handleAgentDisconnected(data: Record<string, unknown>) {
		const agentId = data?.id as string;
		if (agentId) {
			agents = agents.filter((a) => a.id !== agentId);
		}
	}

	async function loadStatus() {
		try {
			const [agentsData, statusData] = await Promise.all([
				api.listAgents().catch(() => null),
				api.getSimpleAgentStatus().catch(() => null)
			]);

			if (agentsData) {
				agents = (agentsData.agents || []).map((a) => ({
					id: a.agentId,
					name: a.agentId,
					status: a.connected ? 'online' : 'offline' as const,
					connected_at: a.createdAt
				}));
			}

			if (statusData) {
				const uptimeSeconds = statusData.uptime || 0;
				const uptimeStr = uptimeSeconds > 3600
					? `${Math.floor(uptimeSeconds / 3600)}h ${Math.floor((uptimeSeconds % 3600) / 60)}m`
					: uptimeSeconds > 60
						? `${Math.floor(uptimeSeconds / 60)}m ${uptimeSeconds % 60}s`
						: `${uptimeSeconds}s`;
				systemStatus = {
					mcpServer: statusData.connected ? 'online' : 'offline',
					database: 'online',
					websocket: wsConnected ? 'online' : 'offline',
					uptime: uptimeStr,
					memoryUsage: '0MB',
					activeSessions: 0,
					connectedClients: wsConnected ? 1 : 0
				};
			}
		} catch (error) {
			console.error('Failed to load status:', error);
		} finally {
			isLoading = false;
		}
	}

	async function loadLanes() {
		try {
			const data = await api.getLanes();
			if (data && typeof data === 'object') {
				lanes = data as unknown as Record<string, LaneStats>;
			}
		} catch {
			// Agent not connected — ignore
		}
	}

	function handleLaneUpdate(data: Record<string, unknown>) {
		// Lane events trigger a refresh of lane stats
		loadLanes();
	}

	function elapsedSince(ms: number): string {
		const elapsed = Date.now() - ms;
		if (elapsed < 1000) return '<1s';
		if (elapsed < 60000) return `${Math.floor(elapsed / 1000)}s`;
		return `${Math.floor(elapsed / 60000)}m ${Math.floor((elapsed % 60000) / 1000)}s`;
	}

	function formatTime(dateStr?: string): string {
		if (!dateStr) return 'N/A';
		return new Date(dateStr).toLocaleString();
	}

	async function loadUpdateSettings() {
		try {
			const [settingsData, checkData] = await Promise.all([
				api.getAgentSettings().catch(() => null),
				api.updateCheck().catch(() => null)
			]);
			if (settingsData?.settings) {
				currentSettings = settingsData.settings;
				autoUpdate = settingsData.settings.autoUpdate;
			}
			if (checkData) {
				updateCheckResult = checkData;
			}
		} catch {
			// Non-critical
		}
	}

	async function toggleAutoUpdate() {
		autoUpdate = !autoUpdate;
		if (currentSettings) {
			try {
				await api.updateAgentSettings({ ...currentSettings, autoUpdate });
				currentSettings = { ...currentSettings, autoUpdate };
			} catch {
				autoUpdate = !autoUpdate; // revert on failure
			}
		}
	}

	async function handleCheckNow() {
		isCheckingUpdate = true;
		try {
			const data = await api.updateCheck();
			if (data) updateCheckResult = data;
		} catch {
			// ignore
		} finally {
			isCheckingUpdate = false;
		}
	}

	$effect(() => {
		systemStatus.websocket = wsConnected ? 'online' : 'offline';
	});
</script>

<div class="mb-6 flex items-center justify-between">
	<div>
		<h2 class="font-display text-xl font-bold text-base-content mb-1">Agent Status</h2>
		<p class="text-sm text-base-content/70">Monitor connected agents and system health</p>
	</div>
	<div class="flex items-center gap-3">
		{#if wsConnected}
			<span class="inline-flex items-center gap-1 text-[11px] font-semibold px-2 py-0.5 rounded-full bg-success/10 text-success">
				<Wifi class="w-3 h-3" />
				Live
			</span>
		{:else if wsReconnecting}
			<span class="inline-flex items-center gap-1 text-[11px] font-semibold px-2 py-0.5 rounded-full bg-warning/10 text-warning">
				<RefreshCw class="w-3 h-3 animate-spin" />
				Reconnecting
			</span>
		{:else}
			<span class="inline-flex items-center gap-1 text-[11px] font-semibold px-2 py-0.5 rounded-full bg-error/10 text-error">
				<WifiOff class="w-3 h-3" />
				Disconnected
			</span>
		{/if}
		<button
			type="button"
			class="h-8 px-3 rounded-lg bg-base-content/5 border border-base-content/10 text-[13px] font-medium text-base-content/70 hover:border-base-content/20 hover:text-base-content transition-colors flex items-center gap-1.5"
			onclick={loadStatus}
		>
			<RefreshCw class="w-3.5 h-3.5" />
		</button>
	</div>
</div>

<!-- System Status Grid -->
<div class="grid sm:grid-cols-2 lg:grid-cols-4 gap-3 mb-6">
	<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-4">
		<div class="flex items-center gap-3">
			<div class="w-9 h-9 rounded-xl {systemStatus.mcpServer === 'online' ? 'bg-success/10' : 'bg-error/10'} flex items-center justify-center shrink-0">
				<Server class="w-4.5 h-4.5 {systemStatus.mcpServer === 'online' ? 'text-success' : 'text-error'}" />
			</div>
			<div>
				<p class="text-sm text-base-content/70">MCP Server</p>
				<p class="text-sm font-bold {systemStatus.mcpServer === 'online' ? 'text-success' : 'text-error'}">
					{systemStatus.mcpServer === 'online' ? 'Online' : 'Offline'}
				</p>
			</div>
		</div>
	</div>

	<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-4">
		<div class="flex items-center gap-3">
			<div class="w-9 h-9 rounded-xl {systemStatus.database === 'online' ? 'bg-success/10' : 'bg-error/10'} flex items-center justify-center shrink-0">
				<Database class="w-4.5 h-4.5 {systemStatus.database === 'online' ? 'text-success' : 'text-error'}" />
			</div>
			<div>
				<p class="text-sm text-base-content/70">Database</p>
				<p class="text-sm font-bold {systemStatus.database === 'online' ? 'text-success' : 'text-error'}">
					{systemStatus.database === 'online' ? 'Online' : 'Offline'}
				</p>
			</div>
		</div>
	</div>

	<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-4">
		<div class="flex items-center gap-3">
			<div class="w-9 h-9 rounded-xl {systemStatus.websocket === 'online' ? 'bg-success/10' : 'bg-error/10'} flex items-center justify-center shrink-0">
				<Wifi class="w-4.5 h-4.5 {systemStatus.websocket === 'online' ? 'text-success' : 'text-error'}" />
			</div>
			<div>
				<p class="text-sm text-base-content/70">WebSocket</p>
				<p class="text-sm font-bold {systemStatus.websocket === 'online' ? 'text-success' : 'text-error'}">
					{systemStatus.websocket === 'online' ? 'Connected' : 'Disconnected'}
				</p>
			</div>
		</div>
	</div>

	<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-4">
		<div class="flex items-center gap-3">
			<div class="w-9 h-9 rounded-xl bg-primary/10 flex items-center justify-center shrink-0">
				<Clock class="w-4.5 h-4.5 text-primary" />
			</div>
			<div>
				<p class="text-sm text-base-content/70">Uptime</p>
				<p class="text-sm font-bold text-base-content">{systemStatus.uptime}</p>
			</div>
		</div>
	</div>
</div>

<!-- Updates -->
<section class="mb-6">
	<h3 class="text-sm font-semibold text-base-content/70 uppercase tracking-wider mb-3">Updates</h3>
	<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
		<div class="flex items-center justify-between">
			<div>
				<p class="text-sm text-base-content/70">Current Version</p>
				<p class="text-sm font-bold text-base-content">{updateCheckResult?.currentVersion ?? 'Loading...'}</p>
			</div>
			<div>
				<p class="text-sm text-base-content/70">Install Method</p>
				<p class="text-sm font-bold text-base-content capitalize">{updateCheckResult?.installMethod ?? '—'}</p>
			</div>
			<div>
				{#if updateCheckResult?.available}
					<span class="inline-flex items-center text-[11px] font-semibold px-2 py-0.5 rounded-full bg-warning/10 text-warning">{updateCheckResult.latestVersion} available</span>
				{:else if updateCheckResult}
					<span class="inline-flex items-center text-[11px] font-semibold px-2 py-0.5 rounded-full bg-success/10 text-success">Up to date</span>
				{/if}
			</div>
			<button
				type="button"
				class="h-8 px-3 rounded-lg bg-base-content/5 border border-base-content/10 text-[13px] font-medium text-base-content/70 hover:border-base-content/20 hover:text-base-content transition-colors flex items-center gap-1.5 disabled:opacity-30"
				onclick={handleCheckNow}
				disabled={isCheckingUpdate}
			>
				<RefreshCw class="w-3.5 h-3.5 {isCheckingUpdate ? 'animate-spin' : ''}" />
				Check Now
			</button>
		</div>
		{#if updateCheckResult?.installMethod === 'direct'}
			<div class="flex items-center justify-between pt-4 mt-4 border-t border-base-content/10">
				<div>
					<p class="text-sm font-medium text-base-content">Auto-update</p>
					<p class="text-sm text-base-content/70">Automatically download and apply updates</p>
				</div>
				<input
					type="checkbox"
					class="toggle toggle-primary"
					checked={autoUpdate}
					onchange={toggleAutoUpdate}
				/>
			</div>
		{:else if updateCheckResult?.installMethod === 'homebrew'}
			<div class="pt-4 mt-4 border-t border-base-content/10">
				<p class="text-sm text-base-content/70">Managed by Homebrew — run <code class="bg-base-content/5 px-1.5 py-0.5 rounded text-sm">brew upgrade nebo</code> to update</p>
			</div>
		{:else if updateCheckResult?.installMethod === 'package_manager'}
			<div class="pt-4 mt-4 border-t border-base-content/10">
				<p class="text-sm text-base-content/70">Managed by package manager</p>
			</div>
		{/if}
	</div>
</section>

<!-- Connected Agents -->
<section class="mb-6">
	<div class="flex items-center justify-between mb-3">
		<h3 class="text-sm font-semibold text-base-content/70 uppercase tracking-wider">Connected Agents</h3>
		<span class="text-sm text-base-content/50">{agents.filter((a) => a.status === 'online').length} online</span>
	</div>

	{#if isLoading}
		<div class="flex items-center justify-center gap-3 py-16">
			<Spinner size={20} />
			<span class="text-sm text-base-content/70">Loading agents...</span>
		</div>
	{:else if agents.length === 0}
		<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-12 text-center">
			<Activity class="w-10 h-10 mx-auto mb-3 text-base-content/30" />
			<h3 class="font-display font-bold text-base-content mb-1">No agents connected</h3>
			<p class="text-sm text-base-content/70">
				Run <code class="bg-base-content/5 px-1.5 py-0.5 rounded text-sm">nebo agent --org your-org</code> to connect
			</p>
		</div>
	{:else}
		<div class="rounded-2xl bg-base-200/50 border border-base-content/10 overflow-hidden">
			<table class="w-full">
				<thead>
					<tr class="text-left text-[13px] text-base-content/50 border-b border-base-content/10">
						<th class="px-4 py-3 font-medium">Agent</th>
						<th class="px-4 py-3 font-medium">Status</th>
						<th class="px-4 py-3 font-medium">Connected</th>
						<th class="px-4 py-3 font-medium">Last Activity</th>
						<th class="px-4 py-3 font-medium">Current Task</th>
					</tr>
				</thead>
				<tbody class="divide-y divide-base-content/10">
					{#each agents as agent}
						<tr>
							<td class="px-4 py-3">
								<div class="flex items-center gap-2">
									<div class="w-2 h-2 rounded-full {agent.status === 'online' ? 'bg-success' : agent.status === 'busy' ? 'bg-warning' : 'bg-error'}"></div>
									<span class="text-sm font-medium text-base-content">{agent.name || agent.id}</span>
								</div>
							</td>
							<td class="px-4 py-3">
								<span class="text-[11px] font-semibold uppercase px-1.5 py-0.5 rounded {agent.status === 'online' ? 'bg-success/10 text-success' : agent.status === 'busy' ? 'bg-warning/10 text-warning' : 'bg-error/10 text-error'}">
									{agent.status}
								</span>
							</td>
							<td class="px-4 py-3 text-sm text-base-content/70">{formatTime(agent.connected_at)}</td>
							<td class="px-4 py-3 text-sm text-base-content/70">{formatTime(agent.last_activity)}</td>
							<td class="px-4 py-3 text-sm text-base-content/70">{agent.current_task || '-'}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</section>

<!-- Lane Monitor -->
<section>
	<div class="flex items-center justify-between mb-3">
		<h3 class="text-sm font-semibold text-base-content/70 uppercase tracking-wider">Lane Monitor</h3>
		<button
			type="button"
			class="h-7 w-7 rounded-lg bg-base-content/5 border border-base-content/10 flex items-center justify-center hover:border-base-content/20 transition-colors"
			onclick={loadLanes}
		>
			<RefreshCw class="w-3 h-3 text-base-content/70" />
		</button>
	</div>

	{#if sortedLanes.length === 0}
		<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-8 text-center">
			<p class="text-sm text-base-content/50">No lane data available</p>
		</div>
	{:else}
		<div class="rounded-2xl bg-base-200/50 border border-base-content/10 divide-y divide-base-content/10">
			{#each sortedLanes as lane}
				{@const isActive = lane.active > 0}
				{@const hasQueued = lane.queued > 0}
				{@const capacity = lane.maxConcurrent === 0 ? 10 : lane.maxConcurrent}
				{@const pct = Math.min((lane.active / capacity) * 100, 100)}
				<div class="p-4">
					<div class="flex items-center justify-between mb-2">
						<div class="flex items-center gap-2">
							<div class="w-2 h-2 rounded-full {isActive ? 'bg-success animate-pulse' : 'bg-base-content/20'}"></div>
							<span class="text-sm font-medium text-base-content">{laneLabels[lane.lane] || lane.lane}</span>
						</div>
						<div class="flex items-center gap-3 text-[13px] text-base-content/50">
							<span>{lane.active} active</span>
							{#if hasQueued}
								<span class="text-warning">{lane.queued} queued</span>
							{/if}
							<span>max {lane.maxConcurrent === 0 ? '∞' : lane.maxConcurrent}</span>
						</div>
					</div>

					<div class="h-1.5 rounded-full bg-base-content/10 overflow-hidden">
						<div
							class="h-full rounded-full transition-all duration-300 {pct > 80 ? 'bg-warning' : 'bg-success'}"
							style="width: {pct}%"
						></div>
					</div>

					{#if lane.activeTasks && lane.activeTasks.length > 0}
						<div class="mt-2 space-y-1">
							{#each lane.activeTasks as task}
								<div class="flex items-center justify-between text-[13px] pl-4">
									<span class="text-base-content/70 truncate">{task.description || task.id}</span>
									{#if task.startedAt}
										<span class="text-base-content/50 ml-2 shrink-0">{elapsedSince(task.startedAt)}</span>
									{/if}
								</div>
							{/each}
						</div>
					{/if}

					{#if lane.queuedTasks && lane.queuedTasks.length > 0}
						<div class="mt-1 space-y-1">
							{#each lane.queuedTasks as task}
								<div class="flex items-center justify-between text-[13px] pl-4">
									<span class="text-base-content/50 truncate">{task.description || task.id}</span>
								</div>
							{/each}
						</div>
					{/if}
				</div>
			{/each}
		</div>
	{/if}
</section>
