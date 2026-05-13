<script lang="ts">
  import { getContext } from 'svelte';
  import type { AgentPageContext } from '$lib/types/agentPage';

  const ctx = getContext<AgentPageContext>('agentPage');
  const agent = $derived(ctx.agent);
  const workflowStats = $derived(ctx.workflowStats);
  const loading = $derived(ctx.runsLoading);
</script>

<div class="flex-1 flex flex-col bg-base-100 min-w-0 min-h-0">
  <div class="h-11 px-[18px] border-b border-base-content/10 flex items-center gap-2 shrink-0">
    <span class="text-sm font-semibold">Run History</span>
    <span class="text-xs text-base-content/50 ml-auto">{agent?.name}</span>
  </div>
  <div class="flex-1 overflow-y-auto p-6">
    {#if loading}
      <div class="flex items-center justify-center pt-16 gap-2">
        <span class="loading loading-spinner loading-sm text-base-content/40"></span>
        <span class="text-xs text-base-content/50">Loading runs…</span>
      </div>
    {:else if workflowStats.totalRuns > 0}
      <div class="grid grid-cols-4 gap-2 mb-5">
        <div class="rounded-lg border border-base-300 bg-base-100 p-3 text-center">
          <div class="text-lg font-semibold">{workflowStats.totalRuns}</div>
          <div class="text-xs text-base-content/50">Total runs</div>
        </div>
        <div class="rounded-lg border border-base-300 bg-base-100 p-3 text-center">
          <div class="text-lg font-semibold text-success">{workflowStats.completed}</div>
          <div class="text-xs text-base-content/50">Completed</div>
        </div>
        <div class="rounded-lg border border-base-300 bg-base-100 p-3 text-center">
          <div class="text-lg font-semibold {workflowStats.failed > 0 ? 'text-error' : ''}">{workflowStats.failed}</div>
          <div class="text-xs text-base-content/50">Failed</div>
        </div>
        <div class="rounded-lg border border-base-300 bg-base-100 p-3 text-center">
          <div class="text-lg font-semibold font-mono">{workflowStats.avgDuration}</div>
          <div class="text-xs text-base-content/50">Avg duration</div>
        </div>
      </div>

      {#if workflowStats.running > 0}
        <div class="flex items-center gap-2 mb-4 py-2 px-3 rounded-lg border border-warning/30 bg-warning/5">
          <span class="loading loading-spinner loading-xs text-warning"></span>
          <span class="text-sm font-medium">{workflowStats.running} workflow{workflowStats.running > 1 ? 's' : ''} running now</span>
        </div>
      {/if}

      <div class="text-center pt-4">
        <div class="text-xs text-base-content/50">Select a run to see its activities.</div>
      </div>
    {:else}
      <div class="text-center pt-10">
        <div class="text-sm font-medium mb-1">No autonomous runs</div>
        <div class="text-xs text-base-content/50">Configure workflows to enable automated runs for {agent?.name}.</div>
      </div>
    {/if}
  </div>
</div>
