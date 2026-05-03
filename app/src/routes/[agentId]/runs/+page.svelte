<script lang="ts">
  import { getContext } from 'svelte';
  import type { AgentPageContext } from '$lib/types/agentPage';

  const ctx = getContext<AgentPageContext>('agentPage');
  const agentId = $derived(ctx.agentId);
  const agent = $derived(ctx.agent);
  const runs = $derived(ctx.runs);
  const workflowStats = $derived(ctx.workflowStats);
</script>

<!-- Column 3: Overview (no run selected) -->
<div class="flex-1 flex flex-col bg-base-100 min-w-0 min-h-0">
  <div class="h-11 px-[18px] border-b border-base-content/10 flex items-center gap-2 shrink-0">
    <span class="text-sm font-semibold">Run History</span>
    <span class="text-xs text-base-content/50 ml-auto">{agent?.name}</span>
  </div>
  <div class="flex-1 overflow-y-auto p-6">
    {#if workflowStats.totalRuns > 0}
      <!-- Stats cards -->
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

      <!-- Dot timeline of recent runs -->
      <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-3">Recent Runs</div>
      <div class="flex flex-col">
        {#each runs.slice(0, 10) as run, idx}
          <div class="flex gap-3">
            <!-- Dot + line -->
            <div class="flex flex-col items-center shrink-0">
              <div class="w-3 h-3 rounded-full shrink-0 {run.status === 'success' ? 'bg-success' : run.status === 'failed' ? 'bg-error' : run.status === 'running' ? 'bg-warning animate-pulse' : 'bg-base-300'}"></div>
              {#if idx < Math.min(runs.length, 10) - 1}
                <div class="w-px flex-1 min-h-6 bg-base-300"></div>
              {/if}
            </div>

            <!-- Run info -->
            <a
              href="/{agentId}/runs/{run.id}"
              class="flex-1 min-w-0 pb-3 text-left cursor-pointer hover:opacity-70 transition-opacity no-underline text-base-content"
            >
              <div class="flex items-center gap-2">
                <span class="text-sm font-medium">{run.name}</span>
                <span class="text-xs text-base-content/50 font-mono">{run.duration}</span>
              </div>
              <div class="text-xs text-base-content/50">{run.date} &middot; <span class="capitalize">{run.trigger}</span></div>
            </a>
          </div>
        {/each}
      </div>
    {:else}
      <div class="text-center pt-10">
        <div class="text-sm font-medium mb-1">No autonomous runs</div>
        <div class="text-xs text-base-content/50">Configure workflows to enable automated runs for {agent?.name}.</div>
      </div>
    {/if}
  </div>
</div>
