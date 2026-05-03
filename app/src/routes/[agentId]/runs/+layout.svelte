<script lang="ts">
  import { getContext } from 'svelte';
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import AgentTabBar from '$lib/components/AgentTabBar.svelte';
  import type { AgentPageContext, AgentRun } from '$lib/types/agentPage';

  let { children } = $props();

  const ctx = getContext<AgentPageContext>('agentPage');
  const agentId = $derived(ctx.agentId);
  const agent = $derived(ctx.agent);
  const runs = $derived(ctx.runs);
  const agentStatusVal = $derived(ctx.agentStatus(ctx.agentId));

  // State
  let statusFilter = $state<'all' | 'failed' | 'running'>('all');

  // Derived: filtered runs
  const filteredRuns = $derived.by(() => {
    if (statusFilter === 'all') return runs;
    return runs.filter((r: AgentRun) => r.status === statusFilter);
  });

  // Derived: counts for filter pills
  const failedCount = $derived(runs.filter((r: AgentRun) => r.status === 'failed').length);
  const runningCount = $derived(runs.filter((r: AgentRun) => r.status === 'running').length);

  // Selected run ID from URL
  const selectedRunId = $derived($page.params.runId ?? null);

  // Trigger icon helper
  function triggerIcon(type: string): string {
    if (type === 'schedule') return '↻';
    if (type === 'event') return '⚡';
    return '▶';
  }
</script>

<!-- Column 2: Run list -->
<div class="w-[260px] min-w-[260px] border-r border-base-content/10 shadow-[2px_0_8px_-2px_rgba(0,0,0,0.06)] relative shrink-0 flex flex-col bg-base-200/50">
  <AgentTabBar agentId={agentId} agentName={agent?.name ?? ''} agentInitial={agent?.initial ?? ''} status={agentStatusVal} />

  <!-- Filter pills -->
  {#if runs.length > 0}
    <div class="flex items-center gap-1.5 px-3 py-2 border-b border-base-300">
      <button
        class="py-0.5 px-2 rounded-full text-xs font-medium cursor-pointer border transition-colors {statusFilter === 'all' ? 'bg-base-content text-base-100 border-base-content' : 'bg-transparent border-base-300 hover:bg-base-200'}"
        onclick={() => statusFilter = 'all'}
      >All {runs.length}</button>
      {#if failedCount > 0}
        <button
          class="py-0.5 px-2 rounded-full text-xs font-medium cursor-pointer border transition-colors {statusFilter === 'failed' ? 'bg-error text-error-content border-error' : 'bg-transparent border-base-300 hover:bg-base-200 text-error'}"
          onclick={() => statusFilter = 'failed'}
        >Failed {failedCount}</button>
      {/if}
      {#if runningCount > 0}
        <button
          class="py-0.5 px-2 rounded-full text-xs font-medium cursor-pointer border transition-colors {statusFilter === 'running' ? 'bg-warning text-warning-content border-warning' : 'bg-transparent border-base-300 hover:bg-base-200 text-warning'}"
          onclick={() => statusFilter = 'running'}
        >Running {runningCount}</button>
      {/if}
    </div>
  {/if}

  <div class="flex-1 overflow-y-auto">
    {#if runs.length === 0}
      <div class="p-6 text-center text-sm">No autonomous runs for this agent.</div>
    {:else if filteredRuns.length === 0}
      <div class="p-6 text-center text-sm text-base-content/50">No {statusFilter} runs.</div>
    {:else}
      {@const grouped = Object.groupBy(filteredRuns, (r: AgentRun) => r.date)}
      {#each Object.entries(grouped) as [date, dateRuns]}
        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 px-3.5 pt-3 pb-1">{date}</div>
        {#each dateRuns as run}
          <a
            href="/{agentId}/runs/{run.id}"
            class="w-full flex items-center gap-2 py-2 px-3.5 border-b border-base-300 text-sm text-left cursor-pointer bg-transparent border-l-2 transition-colors no-underline text-base-content {selectedRunId === run.id ? 'bg-base-100 border-l-primary' : 'border-l-transparent hover:bg-base-200'}"
          >
            <!-- Trigger icon -->
            <span class="text-xs shrink-0 w-4 text-center text-base-content/50" title="{run.trigger}">{triggerIcon(run.trigger ?? '')}</span>
            <div class="flex-1 min-w-0">
              <div class="text-sm font-medium truncate">{run.name}</div>
              <div class="text-xs text-base-content/50 font-mono">{run.date}</div>
            </div>
            <div class="w-[18px] h-[18px] rounded flex items-center justify-center text-sm font-semibold shrink-0 {run.status === 'success' ? 'bg-success/10 text-success' : run.status === 'failed' ? 'bg-error/10 text-error' : run.status === 'running' ? 'bg-warning/10 text-warning' : 'bg-base-200 text-base-content/50'}">
              {#if run.status === 'success'}&#10003;{:else if run.status === 'failed'}&#10007;{:else if run.status === 'running'}<span class="loading loading-spinner loading-xs"></span>{:else}&#8212;{/if}
            </div>
            <div class="text-xs text-base-content/50 font-mono shrink-0 w-[52px] text-right">{run.duration}</div>
          </a>
        {/each}
      {/each}
    {/if}
  </div>
</div>

<!-- Column 3: rendered by child page -->
{@render children()}
