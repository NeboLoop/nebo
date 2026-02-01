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
		Users
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
		mcp_server: 'online' | 'offline';
		database: 'online' | 'offline';
		websocket: 'online' | 'offline';
		uptime: string;
		memory_usage: string;
		active_sessions: number;
		connected_clients: number;
	}

	let agents = $state<Agent[]>([]);
	let systemStatus = $state<SystemStatus>({
		mcp_server: 'offline',
		database: 'offline',
		websocket: 'offline',
		uptime: '0s',
		memory_usage: '0MB',
		active_sessions: 0,
		connected_clients: 0
	});
	let isLoading = $state(true);
	let refreshInterval: ReturnType<typeof setInterval>;
	let unsubscribers: (() => void)[] = [];

	onMount(async () => {
		const client = getWebSocketClient();

		// Track connection status
		unsubscribers.push(
			client.onStatus((status: ConnectionStatus) => {
				wsConnected = status === 'connected';
				wsReconnecting = status === 'connecting';
			})
		);

		// Listen for status updates via websocket
		unsubscribers.push(
			client.on('status_update', handleStatusUpdate),
			client.on('agent_connected', handleAgentConnected),
			client.on('agent_disconnected', handleAgentDisconnected),
			client.on('pong', () => {
				// Pong received, connection is healthy
			})
		);

		await loadStatus();
		// Auto-refresh every 10 seconds (websocket provides real-time updates too)
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
				agents = agentsData.agents || [];
			}

			if (statusData) {
				const data = statusData as Record<string, unknown>;
				systemStatus = {
					mcp_server: (data.mcp_server as 'online' | 'offline') || 'online',
					database: (data.database as 'online' | 'offline') || 'online',
					websocket: wsConnected ? 'online' : 'offline',
					uptime: (data.uptime as string) || '0s',
					memory_usage: (data.memory_usage as string) || '0MB',
					active_sessions: (data.active_sessions as number) || 0,
					connected_clients: (data.connected_clients as number) || 0
				};
			}
		} catch (error) {
			console.error('Failed to load status:', error);
		} finally {
			isLoading = false;
		}
	}

	function formatTime(dateStr?: string): string {
		if (!dateStr) return 'N/A';
		return new Date(dateStr).toLocaleString();
	}

	// Reactive websocket status
	$effect(() => {
		systemStatus.websocket = wsConnected ? 'online' : 'offline';
	});
</script>

<svelte:head>
	<title>Agent Status - Nebo</title>
</svelte:head>

<div class="mb-6 flex items-center justify-between">
	<div>
		<h1 class="font-display text-2xl font-bold text-base-content mb-1">Agent Status</h1>
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
				class="w-10 h-10 rounded-xl {systemStatus.mcp_server === 'online'
					? 'bg-success/10'
					: 'bg-error/10'} flex items-center justify-center"
			>
				<Server
					class="w-5 h-5 {systemStatus.mcp_server === 'online' ? 'text-success' : 'text-error'}"
				/>
			</div>
			<div>
				<p class="text-sm text-base-content/60">MCP Server</p>
				<p
					class="font-bold {systemStatus.mcp_server === 'online' ? 'text-success' : 'text-error'}"
				>
					{systemStatus.mcp_server === 'online' ? 'Online' : 'Offline'}
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

<!-- Quick Stats -->
<div class="grid sm:grid-cols-4 gap-4 mt-6">
	<Card>
		<div class="flex items-center gap-3">
			<div class="w-10 h-10 rounded-xl bg-primary/10 flex items-center justify-center">
				<Activity class="w-5 h-5 text-primary" />
			</div>
			<div>
				<p class="text-sm text-base-content/60">Active Sessions</p>
				<p class="font-display text-2xl font-bold text-base-content">
					{systemStatus.active_sessions}
				</p>
			</div>
		</div>
	</Card>
	<Card>
		<div class="flex items-center gap-3">
			<div class="w-10 h-10 rounded-xl bg-secondary/10 flex items-center justify-center">
				<Users class="w-5 h-5 text-secondary" />
			</div>
			<div>
				<p class="text-sm text-base-content/60">Connected Clients</p>
				<p class="font-display text-2xl font-bold text-base-content">
					{systemStatus.connected_clients}
				</p>
			</div>
		</div>
	</Card>
	<Card>
		<div class="flex items-center gap-3">
			<div class="w-10 h-10 rounded-xl bg-accent/10 flex items-center justify-center">
				<Cpu class="w-5 h-5 text-accent" />
			</div>
			<div>
				<p class="text-sm text-base-content/60">Memory Usage</p>
				<p class="font-display text-2xl font-bold text-base-content">
					{systemStatus.memory_usage}
				</p>
			</div>
		</div>
	</Card>
	<Card>
		<div class="flex items-center gap-3">
			<div class="w-10 h-10 rounded-xl bg-info/10 flex items-center justify-center">
				<Server class="w-5 h-5 text-info" />
			</div>
			<div>
				<p class="text-sm text-base-content/60">Total Agents</p>
				<p class="font-display text-2xl font-bold text-base-content">{agents.length}</p>
			</div>
		</div>
	</Card>
</div>
