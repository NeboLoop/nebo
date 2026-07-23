<script lang="ts">
  import { getContext } from 'svelte';
  import { t } from 'svelte-i18n';
  import { page } from '$app/stores';
  import AgentTabBar from '$lib/components/AgentTabBar.svelte';
  import { mobileChatsOpen } from '$lib/stores/mobileNav';
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

<!-- Column 2: Every run in chronological order (mobile: slide-over toggled from the runs bar) -->
{#if $mobileChatsOpen}
  <div class="fixed inset-0 z-30 bg-black/40 md:hidden" onclick={() => mobileChatsOpen.set(false)} role="presentation"></div>
{/if}
<div class="md:w-[260px] md:min-w-[260px] max-md:fixed max-md:inset-y-0 max-md:left-0 max-md:z-40 max-md:w-[280px] max-md:transition-transform {$mobileChatsOpen ? 'max-md:translate-x-0 max-md:shadow-2xl' : 'max-md:-translate-x-full'} border-r border-base-content/10 shadow-[2px_0_8px_-2px_rgba(0,0,0,0.06)] relative shrink-0 flex flex-col bg-base-200/50 max-md:bg-base-200 overflow-hidden">
  <AgentTabBar agentId={agentId} agentName={agent?.name ?? ''} agentInitial={agent?.initial ?? ''} status={agentStatusVal} isApp={ctx.isApp} />

  {#if runs.length > 0}
    <div class="flex items-center gap-1.5 px-3 py-2 border-b border-base-300">
      <button
        class="py-0.5 px-2 rounded-full text-xs font-medium cursor-pointer border transition-colors {statusFilter === 'all' ? 'bg-base-content text-base-100 border-base-content' : 'bg-transparent border-base-300 hover:bg-base-200'}"
        onclick={() => statusFilter = 'all'}
      >{$t('agentActivity.filterAll', { values: { count: runs.length } })}</button>
      {#if failedCount > 0}
        <button
          class="py-0.5 px-2 rounded-full text-xs font-medium cursor-pointer border transition-colors {statusFilter === 'failed' ? 'bg-error text-error-content border-error' : 'bg-transparent border-base-300 hover:bg-base-200 text-error'}"
          onclick={() => statusFilter = 'failed'}
        >{$t('agentActivity.filterFailed', { values: { count: failedCount } })}</button>
      {/if}
      {#if runningCount > 0}
        <button
          class="py-0.5 px-2 rounded-full text-xs font-medium cursor-pointer border transition-colors {statusFilter === 'running' ? 'bg-warning text-warning-content border-warning' : 'bg-transparent border-base-300 hover:bg-base-200 text-warning'}"
          onclick={() => statusFilter = 'running'}
        >{$t('agentActivity.filterRunning', { values: { count: runningCount } })}</button>
      {/if}
    </div>
  {/if}

  <div class="flex-1 overflow-y-auto">
    {#if runs.length === 0}
      <div class="p-6 text-center text-sm text-base-content/50">{$t('agentActivity.noRuns')}</div>
    {:else if filteredRuns.length === 0}
      <div class="p-6 text-center text-sm text-base-content/50">{$t('agentActivity.noFilteredRuns', { values: { status: statusFilter } })}</div>
    {:else}
      {@const byDate = Object.groupBy(filteredRuns, (r: AgentRun) => r.dateGroup)}
      {#each Object.entries(byDate) as [date, dateRuns]}
        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 px-3.5 pt-3 pb-1">{date}</div>
        {#each dateRuns ?? [] as run}
          <a
            href="/{agentId}/runs/{run.id}"
            class="w-full flex items-center gap-2 py-2 px-3.5 border-b border-base-300/50 text-sm text-left cursor-pointer bg-transparent border-l-2 transition-colors no-underline text-base-content {selectedRunId === run.id ? 'bg-base-100 border-l-primary' : 'border-l-transparent hover:bg-base-200'}"
          >
            <div class="w-2.5 h-2.5 rounded-full shrink-0 {run.status === 'success' ? 'bg-success' : run.status === 'exited' ? 'bg-info' : run.status === 'failed' ? 'bg-error' : run.status === 'running' ? 'bg-warning animate-pulse' : run.status === 'cancelled' ? 'bg-base-content/30' : 'bg-base-300'}"></div>
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

<!-- Column 3: run detail from child page -->
<div class="flex-1 flex flex-col bg-base-100 min-w-0 min-h-0">
  <!-- Mobile runs bar: the drawer toggle (run list is a slide-over below md) -->
  <div class="md:hidden h-10 shrink-0 border-b border-base-300 flex items-center gap-2 px-2">
    <button
      class="h-8 px-2.5 rounded-md flex items-center gap-1.5 text-sm font-medium border-none bg-transparent cursor-pointer text-base-content/80"
      onclick={() => mobileChatsOpen.update((v) => !v)}
    >
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M8 6h13"/><path d="M8 12h13"/><path d="M8 18h13"/><path d="M3 6h.01"/><path d="M3 12h.01"/><path d="M3 18h.01"/></svg>
      {$t('components.agentTabBar.runs')}
    </button>
    <span class="text-sm text-base-content/60 truncate">{agent?.name ?? ''}</span>
  </div>
  {@render children()}
</div>
