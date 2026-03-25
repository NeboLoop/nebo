<script lang="ts">
	import MetricCard from '$lib/components/ui/MetricCard.svelte';
	import type { ActiveRoleEntry, SimpleAgentStatusResponse, NeboLoopJanusUsageResponse, Chat } from '$lib/api/neboComponents';

	let {
		roles = [],
		agentStatus = null,
		usage = null,
		chats = [],
		isLoading = true
	}: {
		roles: ActiveRoleEntry[];
		agentStatus: SimpleAgentStatusResponse | null;
		usage: NeboLoopJanusUsageResponse | null;
		chats: Chat[];
		isLoading: boolean;
	} = $props();

	let activeAgents = $derived(roles.length + 1);
	let tokenPercent = $derived(usage?.weekly?.percentUsed ?? 0);
	let tokenSubtitle = $derived.by(() => {
		if (!usage?.weekly) return '';
		const used = Math.round(usage.weekly.usedTokens / 1000);
		const limit = Math.round(usage.weekly.limitTokens / 1000);
		return `${used}k / ${limit}k credits this week`;
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
		title="Active Roles"
		value={roles.length}
		subtitle="Installed roles running"
		loading={isLoading}
	/>
	<MetricCard
		title="Credit Budget"
		value={tokenPercent / 100}
		format="percentage"
		precision={0}
		subtitle={tokenSubtitle}
		loading={isLoading}
	/>
	<MetricCard
		title="Recent Sessions"
		value={sessionCount}
		subtitle="Conversations"
		loading={isLoading}
	/>
</div>
