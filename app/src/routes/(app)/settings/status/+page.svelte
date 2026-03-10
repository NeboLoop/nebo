<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import Card from '$lib/components/ui/Card.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import Badge from '$lib/components/ui/Badge.svelte';
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
		<p class="text-sm text-base-content/60">Monitor connected agents and system health</p>
	</div>
	<div class="flex items-center gap-3">
		{#if wsConnected}
			<Badge type="success" class="flex items-center gap-1">
				<Wifi class="w-3 h-3" />
				Live Updates
			</Badge>
		{:else if wsReconnecting}
			<Badge type="warning" class="flex items-center gap-1">
				<RefreshCw class="w-3 h-3 animate-spin" />
				Reconnecting
			</Badge>
		{:else}
			<Badge type="error" class="flex items-center gap-1">
				<WifiOff class="w-3 h-3" />
				Disconnected
			</Badge>
		{/if}
		<Button type="ghost" onclick={loadStatus}>
			<RefreshCw class="w-4 h-4 mr-2" />
			Refresh
		</Button>
	</div>
</div>

<!-- System Status Grid -->
<div class="grid sm:grid-cols-2 lg:grid-cols-4 gap-4 mb-8">
	<Card>
		<div class="flex items-center gap-3">
			<div
				class="w-10 h-10 rounded-xl {systemStatus.mcpServer === 'online'
					? 'bg-success/10'
					: 'bg-error/10'} flex items-center justify-center"
			>
				<Server
					class="w-5 h-5 {systemStatus.mcpServer === 'online' ? 'text-success' : 'text-error'}"
				/>
			</div>
			<div>
				<p class="text-sm text-base-content/60">MCP Server</p>
				<p
					class="font-bold {systemStatus.mcpServer === 'online' ? 'text-success' : 'text-error'}"
				>
					{systemStatus.mcpServer === 'online' ? 'Online' : 'Offline'}
				</p>
			</div>
		</div>
	</Card>

	<Card>
		<div class="flex items-center gap-3">
			<div
				class="w-10 h-10 rounded-xl {systemStatus.database === 'online'
					? 'bg-success/10'
					: 'bg-error/10'} flex items-center justify-center"
			>
				<Database
					class="w-5 h-5 {systemStatus.database === 'online' ? 'text-success' : 'text-error'}"
				/>
			</div>
			<div>
				<p class="text-sm text-base-content/60">Database</p>
				<p class="font-bold {systemStatus.database === 'online' ? 'text-success' : 'text-error'}">
					{systemStatus.database === 'online' ? 'Online' : 'Offline'}
				</p>
			</div>
		</div>
	</Card>

	<Card>
		<div class="flex items-center gap-3">
			<div
				class="w-10 h-10 rounded-xl {systemStatus.websocket === 'online'
					? 'bg-success/10'
					: 'bg-error/10'} flex items-center justify-center"
			>
				<Wifi
					class="w-5 h-5 {systemStatus.websocket === 'online' ? 'text-success' : 'text-error'}"
				/>
			</div>
			<div>
				<p class="text-sm text-base-content/60">WebSocket</p>
				<p
					class="font-bold {systemStatus.websocket === 'online' ? 'text-success' : 'text-error'}"
				>
					{systemStatus.websocket === 'online' ? 'Connected' : 'Disconnected'}
				</p>
			</div>
		</div>
	</Card>

	<Card>
		<div class="flex items-center gap-3">
			<div class="w-10 h-10 rounded-xl bg-primary/10 flex items-center justify-center">
				<Clock class="w-5 h-5 text-primary" />
			</div>
			<div>
				<p class="text-sm text-base-content/60">Uptime</p>
				<p class="font-bold text-base-content">{systemStatus.uptime}</p>
			</div>
		</div>
	</Card>
</div>

<!-- Updates -->
<Card class="mb-8">
	<h2 class="font-display font-bold text-base-content mb-4 flex items-center gap-2">
		<ArrowUpCircle class="w-5 h-5" />
		Updates
	</h2>
	<div class="space-y-4">
		<div class="flex items-center justify-between">
			<div>
				<p class="text-sm text-base-content/60">Current Version</p>
				<p class="font-bold text-base-content">{updateCheckResult?.currentVersion ?? 'Loading...'}</p>
			</div>
			<div>
				<p class="text-sm text-base-content/60">Install Method</p>
				<p class="font-bold text-base-content capitalize">{updateCheckResult?.installMethod ?? '—'}</p>
			</div>
			<div>
				{#if updateCheckResult?.available}
					<Badge type="warning">{updateCheckResult.latestVersion} available</Badge>
				{:else if updateCheckResult}
					<Badge type="success">Up to date</Badge>
				{/if}
			</div>
			<Button type="ghost" size="sm" onclick={handleCheckNow} disabled={isCheckingUpdate}>
				{#if isCheckingUpdate}
					<RefreshCw class="w-4 h-4 mr-1 animate-spin" />
				{:else}
					<RefreshCw class="w-4 h-4 mr-1" />
				{/if}
				Check Now
			</Button>
		</div>
		{#if updateCheckResult?.installMethod === 'direct'}
			<div class="flex items-center justify-between pt-3 border-t border-base-300">
				<div>
					<p class="font-medium text-sm">Auto-update</p>
					<p class="text-xs text-base-content/50">Automatically download and apply updates</p>
				</div>
				<input
					type="checkbox"
					class="toggle toggle-primary"
					checked={autoUpdate}
					onchange={toggleAutoUpdate}
				/>
			</div>
		{:else if updateCheckResult?.installMethod === 'homebrew'}
			<div class="pt-3 border-t border-base-300">
				<p class="text-sm text-base-content/50">Managed by Homebrew — run <code class="bg-base-300 px-1.5 py-0.5 rounded text-xs">brew upgrade nebo</code> to update</p>
			</div>
		{:else if updateCheckResult?.installMethod === 'package_manager'}
			<div class="pt-3 border-t border-base-300">
				<p class="text-sm text-base-content/50">Managed by package manager</p>
			</div>
		{/if}
	</div>
</Card>

<!-- Connected Agents -->
<Card>
	<h2 class="font-display font-bold text-base-content mb-4 flex items-center gap-2">
		<Activity class="w-5 h-5" />
		Connected Agents
		<span class="ml-auto text-sm font-normal text-base-content/50">
			{agents.filter((a) => a.status === 'online').length} online
		</span>
	</h2>

	{#if isLoading}
		<div class="py-8 text-center text-base-content/60">Loading agents...</div>
	{:else if agents.length === 0}
		<div class="py-12 text-center">
			<Activity class="w-12 h-12 mx-auto mb-4 text-base-content/30" />
			<h3 class="font-display font-bold text-base-content mb-2">No agents connected</h3>
			<p class="text-base-content/60">
				Run <code class="bg-base-300 px-2 py-1 rounded text-sm">nebo agent --org your-org</code> to
				connect an agent
			</p>
		</div>
	{:else}
		<div class="overflow-x-auto">
			<table class="w-full">
				<thead>
					<tr class="text-left text-sm text-base-content/50 border-b border-base-300">
						<th class="pb-3 font-medium">Agent</th>
						<th class="pb-3 font-medium">Status</th>
						<th class="pb-3 font-medium">Connected</th>
						<th class="pb-3 font-medium">Last Activity</th>
						<th class="pb-3 font-medium">Current Task</th>
					</tr>
				</thead>
				<tbody class="divide-y divide-base-300">
					{#each agents as agent}
						<tr>
							<td class="py-3">
								<div class="flex items-center gap-2">
									<div
										class="w-2 h-2 rounded-full {agent.status === 'online'
											? 'bg-success'
											: agent.status === 'busy'
												? 'bg-warning'
												: 'bg-error'}"
									></div>
									<span class="font-medium">{agent.name || agent.id}</span>
								</div>
							</td>
							<td class="py-3">
								<span
									class="px-2 py-1 rounded text-xs font-medium {agent.status === 'online'
										? 'bg-success/20 text-success'
										: agent.status === 'busy'
											? 'bg-warning/20 text-warning'
											: 'bg-error/20 text-error'}"
								>
									{agent.status}
								</span>
							</td>
							<td class="py-3 text-sm text-base-content/60">
								{formatTime(agent.connected_at)}
							</td>
							<td class="py-3 text-sm text-base-content/60">
								{formatTime(agent.last_activity)}
							</td>
							<td class="py-3 text-sm text-base-content/60">
								{agent.current_task || '-'}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</Card>

<!-- Lane Monitor -->
<Card class="mt-6">
	<h2 class="font-display font-bold text-base-content mb-4 flex items-center gap-2">
		<Layers class="w-5 h-5" />
		Lane Monitor
		<Button type="ghost" size="sm" class="ml-auto" onclick={loadLanes}>
			<RefreshCw class="w-3 h-3" />
		</Button>
	</h2>

	{#if sortedLanes.length === 0}
		<p class="text-base-content/50 text-sm py-4 text-center">No lane data available</p>
	{:else}
		<div class="space-y-3">
			{#each sortedLanes as lane}
				{@const isActive = lane.active > 0}
				{@const hasQueued = lane.queued > 0}
				{@const capacity = lane.maxConcurrent === 0 ? 10 : lane.maxConcurrent}
				{@const pct = Math.min((lane.active / capacity) * 100, 100)}
				<div class="p-3 rounded-lg bg-base-200">
					<div class="flex items-center justify-between mb-2">
						<div class="flex items-center gap-2">
							<div class="w-2 h-2 rounded-full {isActive ? 'bg-success animate-pulse' : 'bg-base-content/20'}"></div>
							<span class="font-medium text-sm">{laneLabels[lane.lane] || lane.lane}</span>
						</div>
						<div class="flex items-center gap-3 text-xs text-base-content/50">
							<span>{lane.active} active</span>
							{#if hasQueued}
								<span class="text-warning">{lane.queued} queued</span>
							{/if}
							<span>max {lane.maxConcurrent === 0 ? '∞' : lane.maxConcurrent}</span>
						</div>
					</div>

					<!-- Capacity bar -->
					<div class="h-1.5 rounded-full bg-base-300 overflow-hidden">
						<div
							class="h-full rounded-full transition-all duration-300 {pct > 80 ? 'bg-warning' : 'bg-success'}"
							style="width: {pct}%"
						></div>
					</div>

					<!-- Active tasks -->
					{#if lane.activeTasks && lane.activeTasks.length > 0}
						<div class="mt-2 space-y-1">
							{#each lane.activeTasks as task}
								<div class="flex items-center justify-between text-xs pl-4">
									<span class="text-base-content/70 truncate">{task.description || task.id}</span>
									{#if task.startedAt}
										<span class="text-base-content/40 ml-2 flex-shrink-0">{elapsedSince(task.startedAt)}</span>
									{/if}
								</div>
							{/each}
						</div>
					{/if}

					<!-- Queued tasks -->
					{#if lane.queuedTasks && lane.queuedTasks.length > 0}
						<div class="mt-1 space-y-1">
							{#each lane.queuedTasks as task}
								<div class="flex items-center justify-between text-xs pl-4">
									<span class="text-base-content/40 truncate">⏳ {task.description || task.id}</span>
								</div>
							{/each}
						</div>
					{/if}
				</div>
			{/each}
		</div>
	{/if}
</Card>
