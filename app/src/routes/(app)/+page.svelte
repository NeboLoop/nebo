<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { goto } from '$app/navigation';
	import { Card, Badge, Button, Spinner } from '$lib/components/ui';
	import {
		Bot,
		MessageSquare,
		Wrench,
		Server,
		Activity,
		ArrowRight,
		Terminal,
		Zap,
		Wifi,
		WifiOff
	} from 'lucide-svelte';
	import { auth } from '$lib/stores/auth';
	import { getWebSocketClient, type ConnectionStatus } from '$lib/websocket/client';

	let currentUser = $derived($auth.user);

	// Dashboard state
	let isLoading = $state(true);
	let wsConnected = $state(false);
	let stats = $state({
		agents_online: 0,
		total_sessions: 0,
		tools_available: 9,
		channels_connected: 0
	});
	let recentActivity = $state<
		Array<{
			id: string;
			type: 'chat' | 'tool' | 'agent';
			message: string;
			time: string;
		}>
	>([]);

	let unsubscribers: (() => void)[] = [];

	onMount(async () => {
		const client = getWebSocketClient();

		// Track connection status
		unsubscribers.push(
			client.onStatus((status: ConnectionStatus) => {
				wsConnected = status === 'connected';
			})
		);

		// Listen for real-time updates
		unsubscribers.push(
			client.on('status_update', handleStatusUpdate),
			client.on('activity', handleActivity),
			client.on('agent_connected', () => {
				stats.agents_online++;
			}),
			client.on('agent_disconnected', () => {
				stats.agents_online = Math.max(0, stats.agents_online - 1);
			})
		);

		await loadDashboardData();
	});

	onDestroy(() => {
		unsubscribers.forEach((unsub) => unsub());
	});

	function handleStatusUpdate(data: Record<string, unknown>) {
		if (data) {
			if (typeof data.agents_online === 'number') {
				stats.agents_online = data.agents_online as number;
			}
			if (typeof data.total_sessions === 'number') {
				stats.total_sessions = data.total_sessions as number;
			}
			if (typeof data.channels_connected === 'number') {
				stats.channels_connected = data.channels_connected as number;
			}
		}
	}

	function handleActivity(data: Record<string, unknown>) {
		if (data) {
			const activity = {
				id: crypto.randomUUID(),
				type: (data.type as 'chat' | 'tool' | 'agent') || 'chat',
				message: (data.message as string) || '',
				time: 'just now'
			};
			recentActivity = [activity, ...recentActivity.slice(0, 4)];
		}
	}

	async function loadDashboardData() {
		isLoading = true;
		try {
			// Load stats from API
			const [statusRes, sessionsRes] = await Promise.all([
				fetch('/api/v1/agent/status').catch(() => null),
				fetch('/api/v1/agent/sessions').catch(() => null)
			]);

			if (statusRes?.ok) {
				const data = await statusRes.json();
				stats.agents_online = data.agents_online || 0;
			}

			if (sessionsRes?.ok) {
				const data = await sessionsRes.json();
				stats.total_sessions = data.sessions?.length || 0;
			}

			// Mock recent activity for now
			recentActivity = [
				{ id: '1', type: 'chat', message: 'New conversation started', time: '2 min ago' },
				{ id: '2', type: 'tool', message: 'bash tool executed', time: '5 min ago' },
				{ id: '3', type: 'agent', message: 'Agent connected', time: '10 min ago' }
			];
		} catch (error) {
			console.error('Failed to load dashboard data:', error);
		} finally {
			isLoading = false;
		}
	}

	const quickActions = [
		{
			label: 'Open Agent Console',
			description: 'Chat with your AI agent',
			href: '/agent',
			icon: Bot,
			color: 'primary'
		},
		{
			label: 'View Sessions',
			description: 'Browse conversation history',
			href: '/sessions',
			icon: MessageSquare,
			color: 'secondary'
		},
		{
			label: 'Check Status',
			description: 'Monitor system health',
			href: '/status',
			icon: Activity,
			color: 'accent'
		}
	];
</script>

<svelte:head>
	<title>Dashboard - GoBot</title>
</svelte:head>

<div class="space-y-6">
	<!-- Header -->
	<div class="flex items-center justify-between">
		<div>
			<h1 class="text-2xl font-bold">Welcome back{currentUser?.name ? `, ${currentUser.name}` : ''}!</h1>
			<p class="text-base-content/60">Here's what's happening with your AI agent</p>
		</div>
		<div class="flex items-center gap-3">
			{#if wsConnected}
				<Badge type="success" class="flex items-center gap-1">
					<Wifi class="w-3 h-3" />
					Live
				</Badge>
			{:else}
				<Badge type="warning" class="flex items-center gap-1">
					<WifiOff class="w-3 h-3" />
					Offline
				</Badge>
			{/if}
			<Button onclick={() => goto('/agent')}>
				<Bot class="w-4 h-4 mr-2" />
				Open Console
			</Button>
		</div>
	</div>

	<!-- Stats Grid -->
	<div class="grid grid-cols-2 lg:grid-cols-4 gap-4">
		<Card class="bg-base-200">
			<div class="flex items-center gap-3">
				<div class="w-12 h-12 rounded-xl bg-success/20 flex items-center justify-center">
					<Activity class="w-6 h-6 text-success" />
				</div>
				<div>
					<p class="text-2xl font-bold">{stats.agents_online}</p>
					<p class="text-sm text-base-content/60">Agents Online</p>
				</div>
			</div>
		</Card>

		<Card class="bg-base-200">
			<div class="flex items-center gap-3">
				<div class="w-12 h-12 rounded-xl bg-primary/20 flex items-center justify-center">
					<MessageSquare class="w-6 h-6 text-primary" />
				</div>
				<div>
					<p class="text-2xl font-bold">{stats.total_sessions}</p>
					<p class="text-sm text-base-content/60">Sessions</p>
				</div>
			</div>
		</Card>

		<Card class="bg-base-200">
			<div class="flex items-center gap-3">
				<div class="w-12 h-12 rounded-xl bg-secondary/20 flex items-center justify-center">
					<Wrench class="w-6 h-6 text-secondary" />
				</div>
				<div>
					<p class="text-2xl font-bold">{stats.tools_available}</p>
					<p class="text-sm text-base-content/60">Tools Available</p>
				</div>
			</div>
		</Card>

		<Card class="bg-base-200">
			<div class="flex items-center gap-3">
				<div class="w-12 h-12 rounded-xl bg-accent/20 flex items-center justify-center">
					<Server class="w-6 h-6 text-accent" />
				</div>
				<div>
					<p class="text-2xl font-bold">{stats.channels_connected}</p>
					<p class="text-sm text-base-content/60">Channels</p>
				</div>
			</div>
		</Card>
	</div>

	<!-- Main Content Grid -->
	<div class="grid lg:grid-cols-3 gap-6">
		<!-- Quick Actions -->
		<div class="lg:col-span-2">
			<Card>
				<h2 class="text-lg font-bold mb-4">Quick Actions</h2>
				<div class="grid sm:grid-cols-3 gap-4">
					{#each quickActions as action}
						<button
							onclick={() => goto(action.href)}
							class="p-4 rounded-xl bg-base-200 hover:bg-base-300 transition-all text-left group"
						>
							<div
								class="w-10 h-10 rounded-lg bg-{action.color}/20 flex items-center justify-center mb-3 group-hover:scale-110 transition-transform"
							>
								<action.icon class="w-5 h-5 text-{action.color}" />
							</div>
							<h3 class="font-semibold mb-1">{action.label}</h3>
							<p class="text-sm text-base-content/60">{action.description}</p>
						</button>
					{/each}
				</div>
			</Card>

			<!-- Try It Now -->
			<Card class="mt-6 bg-gradient-to-br from-primary/10 to-secondary/10 border-primary/20">
				<div class="flex items-center gap-4">
					<div class="w-14 h-14 rounded-xl bg-primary/20 flex items-center justify-center shrink-0">
						<Terminal class="w-7 h-7 text-primary" />
					</div>
					<div class="flex-1">
						<h3 class="font-bold text-lg">Try the CLI Agent</h3>
						<p class="text-sm text-base-content/70 mb-2">
							Run commands directly from your terminal
						</p>
						<code class="text-xs bg-base-300 px-2 py-1 rounded"
							>gobot chat "list files in current directory"</code
						>
					</div>
					<Button type="primary" onclick={() => goto('/agent')}>
						Try Now
						<ArrowRight class="w-4 h-4 ml-2" />
					</Button>
				</div>
			</Card>
		</div>

		<!-- Recent Activity -->
		<div>
			<Card>
				<div class="flex items-center justify-between mb-4">
					<h2 class="text-lg font-bold">Recent Activity</h2>
					<Badge type="info">{recentActivity.length}</Badge>
				</div>

				{#if isLoading}
					<div class="flex justify-center py-8">
						<Spinner />
					</div>
				{:else if recentActivity.length === 0}
					<div class="text-center py-8 text-base-content/60">
						<Zap class="w-8 h-8 mx-auto mb-2 opacity-50" />
						<p>No recent activity</p>
					</div>
				{:else}
					<div class="space-y-3">
						{#each recentActivity as activity}
							<div class="flex items-center gap-3 p-3 rounded-lg bg-base-200">
								<div
									class="w-8 h-8 rounded-lg flex items-center justify-center {activity.type ===
									'chat'
										? 'bg-primary/20'
										: activity.type === 'tool'
											? 'bg-secondary/20'
											: 'bg-success/20'}"
								>
									{#if activity.type === 'chat'}
										<MessageSquare class="w-4 h-4 text-primary" />
									{:else if activity.type === 'tool'}
										<Wrench class="w-4 h-4 text-secondary" />
									{:else}
										<Bot class="w-4 h-4 text-success" />
									{/if}
								</div>
								<div class="flex-1 min-w-0">
									<p class="text-sm truncate">{activity.message}</p>
									<p class="text-xs text-base-content/50">{activity.time}</p>
								</div>
							</div>
						{/each}
					</div>
				{/if}

				<button
					onclick={() => goto('/sessions')}
					class="w-full mt-4 py-2 text-sm text-primary hover:underline"
				>
					View all activity
				</button>
			</Card>

			<!-- System Status -->
			<Card class="mt-6">
				<h2 class="text-lg font-bold mb-4">System Status</h2>
				<div class="space-y-3">
					<div class="flex items-center justify-between">
						<span class="text-sm">MCP Server</span>
						<Badge type="success">Online</Badge>
					</div>
					<div class="flex items-center justify-between">
						<span class="text-sm">Database</span>
						<Badge type="success">Online</Badge>
					</div>
					<div class="flex items-center justify-between">
						<span class="text-sm">WebSocket</span>
						{#if wsConnected}
							<Badge type="success">Connected</Badge>
						{:else}
							<Badge type="warning">Disconnected</Badge>
						{/if}
					</div>
				</div>
				<button
					onclick={() => goto('/status')}
					class="w-full mt-4 py-2 text-sm text-primary hover:underline"
				>
					View full status
				</button>
			</Card>
		</div>
	</div>
</div>
