<script>
  import { AGENTS, SCHEDULE, SCHED_AGENTS, agentColor, runsPerWeek } from './data.js';
  import ScheduleMiniMonth from './ScheduleMiniMonth.svelte';
  import ScheduleDayView from './ScheduleDayView.svelte';
  import ScheduleWeekView from './ScheduleWeekView.svelte';
  import ScheduleMonthView from './ScheduleMonthView.svelte';

  let { defaultView = 'week', dateLabel = '', dateSub = '' } = $props();

  let view = $state(defaultView);
  let enabled = $state(Object.fromEntries(SCHED_AGENTS.map(id => [id, true])));

  const enabledCount = $derived(Object.values(enabled).filter(Boolean).length);

  function toggle(id) { enabled[id] = !enabled[id]; }
</script>

<div class="flex-1 flex min-h-0 overflow-hidden">
  <!-- Left rail -->
  <div class="w-56 border-r border-base-content/10 flex flex-col bg-base-100 shrink-0">
    <div class="px-4 pt-3.5 pb-2 flex items-center justify-between">
      <span class="font-mono text-[10px] font-medium uppercase tracking-wider text-base-content/40">Agents</span>
      <span class="font-mono text-[10px] text-base-content/50">{enabledCount} of {SCHED_AGENTS.length}</span>
    </div>

    <div class="px-2 flex flex-col gap-px">
      {#each SCHED_AGENTS as id}
        {@const a = AGENTS.find(x => x.id === id)}
        {@const c = agentColor(id)}
        {@const on = enabled[id]}
        <label class="flex items-center gap-2.5 px-2 py-1.5 rounded-md cursor-pointer text-[13px] hover:bg-base-content/5 transition-colors {on ? '' : 'opacity-50'}">
          <span
            class="w-3.5 h-3.5 rounded-sm flex items-center justify-center shrink-0 text-[10px] leading-none"
            style:background={on ? c.ink : 'transparent'}
            style:border="1.5px solid {c.ink}"
            style:color={on ? 'white' : 'transparent'}
          >{#if on}✓{/if}</span>
          <span class="flex-1 font-medium">{a.name}</span>
          <span class="font-mono text-[10px] text-base-content/40">{runsPerWeek(id)}/wk</span>
          <input type="checkbox" checked={on} onchange={() => toggle(id)} hidden />
        </label>
      {/each}
    </div>

    <div class="h-4"></div>

    <div class="px-4 pb-1 font-mono text-[10px] font-medium uppercase tracking-wider text-base-content/40">Trigger types</div>
    <div class="px-4 flex flex-col gap-2 text-xs text-base-content/70">
      <span class="flex items-center gap-2">
        <span class="w-4.5 h-4.5 rounded bg-base-200 border border-base-content/10 font-mono text-[11px] text-base-content/60 inline-flex items-center justify-center">↻</span>
        Scheduled
      </span>
      <span class="flex items-center gap-2">
        <span class="w-4.5 h-4.5 rounded bg-base-200 border border-base-content/10 font-mono text-[11px] text-base-content/60 inline-flex items-center justify-center">⚡</span>
        Event
      </span>
      <span class="flex items-center gap-2">
        <span class="w-4.5 h-4.5 rounded bg-base-200 border border-base-content/10 font-mono text-[11px] text-base-content/60 inline-flex items-center justify-center">›</span>
        You
      </span>
    </div>

    <div class="flex-1"></div>
    <ScheduleMiniMonth />
  </div>

  <!-- Main pane -->
  <div class="flex-1 flex flex-col min-h-0">
    <!-- Header -->
    <div class="px-5 py-3.5 flex items-center gap-3.5 border-b border-base-content/10">
      <h1 class="text-xl font-bold tracking-tight">{dateLabel}</h1>
      {#if dateSub}
        <span class="text-sm text-base-content/50">{dateSub}</span>
      {/if}
      <div class="flex-1"></div>

      <!-- View toggle -->
      <div class="inline-flex bg-base-200/80 rounded-lg p-0.5 border border-base-content/5">
        {#each ['day', 'week', 'month'] as v}
          <button
            class="px-3.5 py-1 rounded-md text-xs cursor-pointer transition-all {view === v ? 'bg-base-100 font-medium text-base-content shadow-sm' : 'text-base-content/50'}"
            onclick={() => view = v}
          >{v[0].toUpperCase() + v.slice(1)}</button>
        {/each}
      </div>

      <button class="btn btn-ghost btn-sm text-xs">Today</button>
    </div>

    <!-- Calendar content -->
    <div class="flex-1 min-h-0 flex">
      {#if view === 'day'}
        <ScheduleDayView {enabled} />
      {:else if view === 'week'}
        <ScheduleWeekView {enabled} />
      {:else}
        <ScheduleMonthView {enabled} />
      {/if}
    </div>
  </div>
</div>
