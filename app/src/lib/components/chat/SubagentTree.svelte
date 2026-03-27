<script lang="ts">
	import { Loader2, Check, X, ChevronDown, ChevronRight } from 'lucide-svelte';

	export interface SubagentState {
		taskId: string;
		description: string;
		agentType: string;
		status: 'pending' | 'running' | 'complete' | 'error';
		toolCount: number;
		tokenCount: number;
		currentOperation: string;
	}

	interface Props {
		agents: SubagentState[];
		expanded?: boolean;
	}

	let { agents, expanded = $bindable(false) }: Props = $props();

	const totalCount = $derived(agents.length);
	const runningCount = $derived(agents.filter((a) => a.status === 'running').length);
	const completedCount = $derived(agents.filter((a) => a.status === 'complete').length);
	const failedCount = $derived(agents.filter((a) => a.status === 'error').length);
	const allDone = $derived(runningCount === 0 && totalCount > 0);

	const agentTypeLabel = $derived(() => {
		const types = new Set(agents.map((a) => a.agentType));
		if (types.size === 1) {
			const t = [...types][0];
			return t.charAt(0).toUpperCase() + t.slice(1);
		}
		return '';
	});

	function formatTokens(count: number): string {
		if (count >= 1000000) return `${(count / 1000000).toFixed(1)}M`;
		if (count >= 1000) return `${(count / 1000).toFixed(1)}k`;
		return `${count}`;
	}
</script>

<div class="subagent-tree">
	<button
		class="subagent-tree-header"
		onclick={() => (expanded = !expanded)}
		type="button"
	>
		{#if expanded}
			<ChevronDown size={14} />
		{:else}
			<ChevronRight size={14} />
		{/if}

		{#if allDone}
			<Check size={14} class="subagent-tree-icon-done" />
		{:else}
			<Loader2 size={14} class="subagent-tree-spinner" />
		{/if}

		<span class="subagent-tree-title">
			{#if allDone}
				{totalCount} {agentTypeLabel()} agents completed
				{#if failedCount > 0}
					({failedCount} failed)
				{/if}
			{:else}
				Running {totalCount} {agentTypeLabel()} agents\u2026
			{/if}
		</span>
	</button>

	{#if expanded}
		<div class="subagent-tree-list">
			{#each agents as agent, i (agent.taskId)}
				{@const isLast = i === agents.length - 1}
				<div class="subagent-tree-node">
					<span class="subagent-tree-connector">
						{isLast ? '\u2514\u2500' : '\u251C\u2500'}
					</span>

					{#if agent.status === 'running'}
						<Loader2 size={12} class="subagent-tree-spinner" />
					{:else if agent.status === 'complete'}
						<Check size={12} class="subagent-tree-icon-done" />
					{:else if agent.status === 'error'}
						<X size={12} class="subagent-tree-icon-error" />
					{/if}

					<span class="subagent-tree-desc">{agent.description}</span>

					{#if agent.toolCount > 0 || agent.tokenCount > 0}
						<span class="subagent-tree-metrics">
							{#if agent.toolCount > 0}
								\u00B7 {agent.toolCount} tool uses
							{/if}
							{#if agent.tokenCount > 0}
								\u00B7 {formatTokens(agent.tokenCount)} tokens
							{/if}
						</span>
					{/if}
				</div>

				{#if agent.status === 'running' && agent.currentOperation}
					<div class="subagent-tree-progress">
						<span class="subagent-tree-connector-cont">
							{isLast ? '\u00A0\u00A0' : '\u2502\u00A0'}
						</span>
						<span class="subagent-tree-progress-text">
							\u23BF {agent.currentOperation}
						</span>
					</div>
				{/if}

				{#if agent.status === 'complete'}
					<div class="subagent-tree-progress">
						<span class="subagent-tree-connector-cont">
							{isLast ? '\u00A0\u00A0' : '\u2502\u00A0'}
						</span>
						<span class="subagent-tree-done-text">\u23BF Done</span>
					</div>
				{/if}
			{/each}
		</div>
	{/if}
</div>
