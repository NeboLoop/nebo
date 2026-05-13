<script lang="ts">
  import { getContext } from 'svelte';
  import { page } from '$app/stores';
  import AgentTabBar from '$lib/components/AgentTabBar.svelte';
  import type { AgentPageContext, AgentRun } from '$lib/types/agentPage';

  let { children } = $props();

  const ctx = getContext<AgentPageContext>('agentPage');
  const agentId = $derived(ctx.agentId);
  const agent = $derived(ctx.agent);
  const runs = $derived(ctx.runs);
  const hasMoreRuns = $derived(ctx.hasMoreRuns);
  const runsLoading = $derived(ctx.runsLoading);
  const agentStatusVal = $derived(ctx.agentStatus(ctx.agentId));

  let statusFilter = $state<'all' | 'failed' | 'running'>('all');

  const filteredRuns = $derived.by(() => {
    if (statusFilter === 'all') return runs;
    return runs.filter((r: AgentRun) => r.status === statusFilter);
  });

  const failedCount = $derived(runs.filter((r: AgentRun) => r.status === 'failed').length);
  const runningCount = $derived(runs.filter((r: AgentRun) => r.status === 'running').length);
  const selectedRunId = $derived($page.params.runId ?? null);

  // Infinite scroll via IntersectionObserver
  let sentinelEl = $state<HTMLElement | null>(null);

  $effect(() => {
    const el = sentinelEl;
    if (!el) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[0]?.isIntersecting && ctx.hasMoreRuns && !ctx.runsLoading && statusFilter === 'all') {
          ctx.loadMoreRuns();
        }
      },
      { rootMargin: '200px' }
    );
    observer.observe(el);
    return () => observer.disconnect();
  });
</script>

<!-- Column 2: Every run in chronological order -->
<div class="w-[260px] min-w-[260px] border-r border-base-content/10 shadow-[2px_0_8px_-2px_rgba(0,0,0,0.06)] relative shrink-0 flex flex-col bg-base-200/50 overflow-hidden">
  <AgentTabBar agentId={agentId} agentName={agent?.name ?? ''} agentInitial={agent?.initial ?? ''} status={agentStatusVal} />

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
      <div class="p-6 text-center text-sm text-base-content/50">No runs yet.</div>
    {:else if filteredRuns.length === 0}
      <div class="p-6 text-center text-sm text-base-content/50">No {statusFilter} runs.</div>
    {:else}
      {@const byDate = Object.groupBy(filteredRuns, (r: AgentRun) => r.dateGroup)}
      {#each Object.entries(byDate) as [date, dateRuns]}
        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 px-3.5 pt-3 pb-1">{date}</div>
        {#each dateRuns ?? [] as run}
          <a
            href="/{agentId}/runs/{run.id}"
            class="w-full flex items-center gap-2 py-2 px-3.5 border-b border-base-300/50 text-sm text-left cursor-pointer bg-transparent border-l-2 transition-colors no-underline text-base-content {selectedRunId === run.id ? 'bg-base-100 border-l-primary' : 'border-l-transparent hover:bg-base-200'}"
          >
            <div class="w-2.5 h-2.5 rounded-full shrink-0 {run.status === 'success' ? 'bg-success' : run.status === 'skipped' ? 'bg-base-content/30' : run.status === 'failed' ? 'bg-error' : run.status === 'running' ? 'bg-warning animate-pulse' : 'bg-base-300'}"></div>
            <div class="flex-1 min-w-0">
              <div class="text-sm font-medium truncate">{run.workflowName}</div>
              <div class="text-xs text-base-content/50 font-mono">{run.time}</div>
            </div>
            <div class="text-xs text-base-content/50 font-mono shrink-0">{run.duration}</div>
          </a>
        {/each}
      {/each}
      {#if runsLoading}
        <div class="flex justify-center py-3">
          <span class="loading loading-spinner loading-xs"></span>
        </div>
      {/if}
      <div bind:this={sentinelEl} class="h-1"></div>
    {/if}
  </div>
</div>

{@render children()}
