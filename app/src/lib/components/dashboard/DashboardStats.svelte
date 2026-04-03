<script lang="ts">
	import MetricCard from '$lib/components/ui/MetricCard.svelte';
	import type { ActiveAgentEntry, SimpleAgentStatusResponse, NeboLoopJanusUsageResponse, Chat } from '$lib/api/neboComponents';

	let {
		agents = [],
		agentStatus = null,
		usage = null,
		chats = [],
		isLoading = true
	}: {
		agents: ActiveAgentEntry[];
		agentStatus: SimpleAgentStatusResponse | null;
		usage: NeboLoopJanusUsageResponse | null;
		chats: Chat[];
		isLoading: boolean;
	} = $props();

	let activeAgents = $derived(agents.length + 1);
	let tokenPercent = $derived(usage?.weekly?.percentUsed ?? 0);
	let tokenSubtitle = $derived.by(() => {
		if (!usage?.weekly) return '';
		return `${usage.weekly.percentUsed}% used this week`;
	});
	let sessionCount = $derived(chats.length);
</script>

<div class="grid sm:grid-cols-2 lg:grid-cols-4 gap-3">
	<MetricCard
		title="Active Agents"
		value={activeAgents}
		subtitle={agentStatus?.connected ? 'Online' : 'Offline'}
		loading={isLoading}
	/>
	<MetricCard
		title="Active Agents"
		value={agents.length}
		subtitle="Installed agents running"
		loading={isLoading}
	/>
	<a href="/settings/usage" class="contents">
		<MetricCard
			title="Credit Budget"
			value={tokenPercent / 100}
			format="percentage"
			precision={0}
			subtitle={tokenSubtitle}
			loading={isLoading}
		/>
	</a>
	<MetricCard
		title="Recent Sessions"
		value={sessionCount}
		subtitle="Conversations"
		loading={isLoading}
	/>
</div>
