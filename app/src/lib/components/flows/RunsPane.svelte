<!--
  Runs — the audit trail, in the work pane. Every autonomous run this employee
  has done, newest first, with the status filter and infinite scroll the runs
  page had. A run opens in full for inspection.
-->
<script lang="ts">
  import { getContext } from 'svelte';
  import { t } from 'svelte-i18n';
  import { page } from '$app/stores';
  import type { AgentPageContext } from '$lib/types/agentPage';

  let { onopen }: { onopen: (id: string) => void } = $props();

  const ctx = getContext<AgentPageContext>('agentPage');
  // The ?runs param's value carries the entry filter ("failed"/"running") so
  // the stat tiles deep-link to what they count; the chips own it after that.
  const entryFilter = (v: string | null): 'all' | 'failed' | 'running' =>
    v === 'failed' || v === 'running' ? v : 'all';
  let statusFilter = $state<'all' | 'failed' | 'running'>(
    entryFilter($page.url.searchParams.get('runs'))
  );
  // Clicking a tile while the modal is already open re-aims the filter.
  let prevRunsParam = $page.url.searchParams.get('runs');
  $effect(() => {
    const v = $page.url.searchParams.get('runs');
    if (v !== prevRunsParam) {
      prevRunsParam = v;
      if (v !== null) statusFilter = entryFilter(v);
    }
  });

  const runs = $derived(ctx.runs);
  const shown = $derived(
    statusFilter === 'all' ? runs : runs.filter((r) => r.status === statusFilter)
  );
  const failedCount = $derived(runs.filter((r) => r.status === 'failed').length);
  const runningCount = $derived(runs.filter((r) => r.status === 'running').length);

  const filters = $derived([
    { id: 'all' as const, label: $t('common.all'), count: runs.length },
    { id: 'failed' as const, label: $t('common.failed'), count: failedCount },
    { id: 'running' as const, label: $t('agent.running'), count: runningCount }
  ]);

  // Infinite scroll: filtering is client-side over the loaded page, so paging
  // only makes sense on the unfiltered list.
  let sentinel = $state<HTMLDivElement | null>(null);
  $effect(() => {
    if (!sentinel || statusFilter !== 'all' || !ctx.hasMoreRuns) return;
    const io = new IntersectionObserver(
      (es) => {
        if (es[0]?.isIntersecting && !ctx.runsLoading) ctx.loadMoreRuns();
      },
      { rootMargin: '200px' }
    );
    io.observe(sentinel);
    return () => io.disconnect();
  });

  function dotClass(status: string): string {
    if (status === 'success') return 'bg-success';
    if (status === 'running') return 'bg-warning animate-pulse';
    if (status === 'failed') return 'bg-error';
    if (status === 'interrupted') return 'bg-warning';
    return 'bg-base-content/30';
  }
</script>

<div class="flex-1 min-w-0 flex flex-col h-full min-h-0">
  {#if runs.length > 0}
    <div class="px-3 py-2 border-b border-base-content/8 flex items-center gap-1 shrink-0">
      {#each filters as f (f.id)}
        <button
          class="btn btn-xs normal-case {statusFilter === f.id ? 'btn-neutral' : 'btn-ghost text-base-content/60'}"
          onclick={() => (statusFilter = f.id)}
        >{f.label} {f.count}</button>
      {/each}
    </div>
  {/if}

  <div class="flex-1 min-h-0 overflow-y-auto">
    {#if shown.length === 0}
      <p class="p-6 text-center text-sm text-base-content/50">{$t('agent.noRunsYet')}</p>
    {:else}
      {#each shown as r (r.id)}
        <button
          type="button"
          class="w-full text-left flex items-center gap-2.5 py-2.5 px-4 border-b border-base-content/8 bg-transparent cursor-pointer hover:bg-base-200/60 transition-colors"
          onclick={() => onopen(r.id)}
        >
          <span class="w-2 h-2 rounded-full shrink-0 {dotClass(r.status)}"></span>
          <span class="flex-1 min-w-0">
            <span class="block text-sm truncate">{r.workflowName}</span>
            <span class="block text-xs text-base-content/50 font-mono">{r.dateGroup} &middot; {r.time}</span>
          </span>
          <span class="text-xs text-base-content/50 font-mono shrink-0">{r.duration}</span>
        </button>
      {/each}
      {#if statusFilter === 'all' && ctx.hasMoreRuns}
        <div bind:this={sentinel} class="py-4 flex justify-center">
          <span class="loading loading-spinner loading-sm"></span>
        </div>
      {/if}
    {/if}
  </div>
</div>
