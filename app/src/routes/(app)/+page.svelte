<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { RotateCcw } from 'lucide-svelte';
	import { getActiveRoles as getActiveAgents, getSimpleAgentStatus, neboLoopJanusUsage, listChats, listMCPIntegrations } from '$lib/api/nebo';
	import { getWebSocketClient } from '$lib/websocket/client';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import DashboardStats from '$lib/components/dashboard/DashboardStats.svelte';
	import AgentCards from '$lib/components/dashboard/AgentCards.svelte';
	import ActivityFeed from '$lib/components/dashboard/ActivityFeed.svelte';
	import QuickActions from '$lib/components/dashboard/QuickActions.svelte';
	import type { ActiveRoleEntry as ActiveAgentEntry, SimpleAgentStatusResponse, NeboLoopJanusUsageResponse, Chat } from '$lib/api/neboComponents';

	let agents = $state<ActiveAgentEntry[]>([]);
	let agentStatus = $state<SimpleAgentStatusResponse | null>(null);
	let usage = $state<NeboLoopJanusUsageResponse | null>(null);
	let chats = $state<Chat[]>([]);
	let mcpCount = $state(0);
	let isLoading = $state(true);

	async function load() {
		const [r, s, u, c, m] = await Promise.all([
			getActiveAgents().catch(() => ({ roles: [], count: 0 })),
			getSimpleAgentStatus().catch(() => null),
			neboLoopJanusUsage().catch(() => null),
			listChats({ pageSize: 10 }).catch(() => ({ chats: [], total: 0 })),
			listMCPIntegrations().catch(() => ({ integrations: [] })),
		]);
		agents = r?.roles ?? [];
		agentStatus = s;
		usage = u;
		chats = c?.chats ?? [];
		mcpCount = (m?.integrations ?? []).length;
		isLoading = false;
	}

	const ws = getWebSocketClient();
	let unsubs: Array<() => void> = [];

	onMount(() => {
		load();
		unsubs = [
			ws.on('chat_complete', () => load()),
			ws.on('agent_activated', () => load()),
			ws.on('agent_deactivated', () => load()),
		];
	});

	onDestroy(() => unsubs.forEach((u) => u()));
</script>

<PageHeader title="Dashboard">
	{#snippet actions()}
		<button class="btn btn-ghost btn-sm gap-1.5" onclick={load}>
			<RotateCcw class="w-4 h-4" /> Refresh
		</button>
	{/snippet}
</PageHeader>

<div class="flex flex-col gap-6">
	<DashboardStats {agents} {agentStatus} {usage} {chats} {isLoading} />
	<AgentCards {agents} {agentStatus} {isLoading} />
	<div class="grid lg:grid-cols-5 gap-6">
		<div class="lg:col-span-3">
			<ActivityFeed {chats} {isLoading} />
		</div>
		<div class="lg:col-span-2">
			<QuickActions {mcpCount} />
		</div>
	</div>
</div>
