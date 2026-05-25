<script lang="ts">
  import { AGENT_COLORS } from '$lib/tokens.js';
  import { AGENTS, CAL_DAYS } from '$lib/data.js';
  import { onMount } from 'svelte';
  import { packLanes } from '$lib/utils.js';
  import { flattenForDate, attachRunData, snapTo15, userScheduleItems } from '$lib/stores/schedule.js';
  import type { CalendarItem } from '$lib/stores/schedule.js';
  import DayDetailPane from './DayDetailPane.svelte';

  let { enabled, selectedDate, onopencanvas, showHeartbeats = false }: { enabled: Record<string, boolean>; selectedDate: Date; onopencanvas?: (agentFull: string) => void; showHeartbeats?: boolean } = $props();

  const HOUR_PX = 80;

  let scrollEl = $state<HTMLDivElement | null>(null);
  let selected = $state<string | null>(null);
  let createData = $state<{ hour: number; date: Date } | null>(null);
  let preview = $state<{ agent: string; hour: number; dur: number; label: string } | null>(null);
  let now = $state(new Date());
  const nowHour = $derived(now.getHours() + now.getMinutes() / 60);
  const nowLabel = $derived(`${now.getHours() % 12 || 12}:${String(now.getMinutes()).padStart(2, '0')}`);

  onMount(() => {
    if (scrollEl) scrollEl.scrollTop = 7 * HOUR_PX;
  });

  $effect(() => {
    const interval = setInterval(() => { now = new Date(); }, 60_000);
    return () => clearInterval(interval);
  });

  const rulerMarks = Array.from({ length: 24 }, (_, i) => i);

  const weekStart = $derived.by(() => {
    const d = new Date(selectedDate);
    const dow = d.getDay();
    const diff = (dow === 0 ? -6 : 1) - dow;
    d.setDate(d.getDate() + diff);
    return d;
  });

  const weekDays = $derived(
    Array.from({ length: 7 }, (_, i) => {
      const d = new Date(weekStart);
      d.setDate(d.getDate() + i);
      return d;
    })
  );

  function isToday(date: Date) {
    return date.getFullYear() === now.getFullYear() &&
      date.getMonth() === now.getMonth() &&
      date.getDate() === now.getDate();
  }

  function dateToWd(date: Date) {
    const d = date.getDay();
    return d === 0 ? 7 : d;
  }

  function expandBand(band: CalendarItem & { _id: string }): Array<CalendarItem & { _id: string }> {
    const intervalStr = band.interval || '30m';
    const hMatch = intervalStr.match(/(\d+)h/);
    const mMatch = intervalStr.match(/(\d+)m/);
    const minutes = (hMatch ? parseInt(hMatch[1]) * 60 : 0) + (mMatch ? parseInt(mMatch[1]) : 0);
    if (minutes <= 0) return [band];
    const step = minutes / 60;
    const items: Array<CalendarItem & { _id: string }> = [];
    let h = band.hour;
    let idx = 0;
    while (h < band.end) {
      items.push({ ...band, _id: `${band._id}:${idx}`, hour: h, dur: 0.25, end: h + 0.25 });
      h += step;
      idx++;
    }
    return items;
  }

  function dayItems(date: Date) {
    const wd = dateToWd(date);
    const items = attachRunData(flattenForDate(wd, enabled, $userScheduleItems));
    const regular = items.filter(i => i.triggerType !== 'heartbeat');
    if (!showHeartbeats) return packLanes(regular);
    const expanded = items.filter(i => i.triggerType === 'heartbeat').flatMap(expandBand);
    return packLanes([...regular, ...expanded]);
  }

  const allPacked = $derived(weekDays.flatMap(d => dayItems(d)));
  const selectedItem = $derived(selected ? allPacked.find(p => p._id === selected) ?? null : null);

  function handleColumnClick(e: MouseEvent, date: Date) {
    if (e.target !== e.currentTarget) return;
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const y = e.clientY - rect.top;
    const hour = snapTo15(y / HOUR_PX);
    selected = null;
    createData = { hour, date: new Date(date) };
  }
</script>

<div class="flex-1 flex bg-base-100 min-h-0 overflow-hidden">
  <div class="flex-1 flex flex-col min-h-0 min-w-0">
  <!-- day strip -->
  <div class="flex border-b border-base-content/10 bg-base-200/50 shrink-0">
    <div class="w-18 border-r border-base-content/10"></div>
    {#each weekDays as day, i}
      {@const today = isToday(day)}
      <div class="flex-1 px-3 py-2 {today ? 'bg-base-100' : ''}" style:border-right={i < 6 ? '1px solid var(--color-base-300)' : 'none'}>
        <div class="text-sm uppercase tracking-wider text-base-content/70">{CAL_DAYS[i]}</div>
        <div class="mt-0.5">
          {#if today}
            <span class="inline-flex items-center justify-center w-6 h-6 rounded-full bg-primary text-primary-content text-sm font-semibold">{day.getDate()}</span>
          {:else}
            <span class="text-base">{day.getDate()}</span>
          {/if}
        </div>
      </div>
    {/each}
  </div>

  <!-- body -->
  <div class="flex-1 flex overflow-y-auto min-h-0" bind:this={scrollEl}>
    <!-- ruler -->
    <div class="w-18 border-r border-base-content/10 relative shrink-0 bg-base-200/50" style="height:{24 * HOUR_PX}px">
      {#each rulerMarks as h, i}
        <div class="absolute right-3 -translate-y-1/2 text-xs text-base-content/70" style="top:{i * HOUR_PX}px">
          {#if h === 12}
            <span class="font-medium">Noon</span>
          {:else}
            <span class="font-medium">{h === 0 ? 12 : h <= 12 ? h : h - 12}</span>
            <span class="text-base-content/60 ml-0.5">{h < 12 ? 'AM' : 'PM'}</span>
          {/if}
        </div>
      {/each}

      {#if weekDays.some(d => isToday(d))}
        <div class="absolute left-0 right-0 z-20" style="top:{nowHour * HOUR_PX}px">
          <span class="absolute right-1 -translate-y-1/2 px-1.5 py-0.5 rounded-field bg-error text-error-content text-sm font-semibold">{nowLabel}</span>
        </div>
      {/if}
    </div>

    <!-- day columns -->
    {#each weekDays as day, dayIdx}
      {@const today = isToday(day)}
      {@const dayPacked = dayItems(day)}
      <div
        class="flex-1 relative cursor-pointer {today ? 'bg-primary/5' : ''}"
        style="height:{24 * HOUR_PX}px; border-right:{dayIdx < 6 ? '1px solid var(--color-base-300)' : 'none'}; background-image:linear-gradient(to bottom, var(--color-base-300) 1px, transparent 1px); background-size:100% {HOUR_PX}px"
        ondblclick={(e) => handleColumnClick(e, day)}
        role="button"
        tabindex="-1"
      >
        {#if today}
          <div class="absolute left-0 right-0 h-0.5 bg-error z-4" style="top:{nowHour * HOUR_PX}px"></div>
        {/if}

        <!-- Event items -->
        {#each dayPacked as item}
          {@const top = item.hour * HOUR_PX}
          {@const height = Math.max(32, item.dur * HOUR_PX)}
          {@const c = AGENT_COLORS[item.agent]}
          {@const a = AGENTS.find(x => x.id === item.agent)}
          {@const indent = item.totalLanes > 1 ? Math.min(20, 80 / item.totalLanes) : 0}
          {@const leftPct = item.lane * indent}
          {@const widthPct = item.totalLanes > 1 ? 100 - leftPct - (item.lane < item.totalLanes - 1 ? 5 : 0) : 100}
          {@const zBase = 2 + item.lane}
          {@const isHeartbeat = item.triggerType === 'heartbeat'}
          <div
            class="absolute rounded-sm overflow-hidden cursor-pointer flex items-start border-l-[2.5px] px-1 pt-0.5 min-h-[18px] transition-shadow {c.fillClass} {c.edgeClass} {c.textClass} {selected === item._id ? 'ring-2' : ''}"
            style="left:calc({leftPct}% + 2px); width:calc({widthPct}% - 4px); top:{top}px; height:{height}px; {isHeartbeat ? 'border-left-style:dashed; ' : ''}{selected === item._id ? `--tw-ring-color:${c.edgeVar}; z-index:20` : `z-index:${zBase}`}"
            title="{a?.name ?? item.agent}: {item.label}{isHeartbeat && item.interval ? ` (every ${item.interval})` : ''}"
            onclick={(e) => { e.stopPropagation(); selected = item._id; createData = null; }}
            onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); selected = item._id; createData = null; } }}
            role="button"
            tabindex="0"
          >
            <span class="text-xs font-semibold overflow-hidden text-ellipsis whitespace-nowrap flex-1">{isHeartbeat ? '↻ ' : ''}{a?.name ?? item.agent}: {item.label}</span>
            {#if item.run}
              <span class="w-1.5 h-1.5 rounded-full shrink-0 ml-0.5 mt-0.5 {item.run.status === 'success' ? 'bg-success' : item.run.status === 'failed' ? 'bg-error' : item.run.status === 'skipped' ? 'bg-warning' : 'bg-base-content/30'}"></span>
              {/if}
            </div>
        {/each}

        <!-- Live preview block while creating -->
        {#if preview && createData}
          {@const pv = preview}
          {@const previewDayIdx = (() => { const cd = createData.date; return weekDays.findIndex(wd => wd.getFullYear() === cd.getFullYear() && wd.getMonth() === cd.getMonth() && wd.getDate() === cd.getDate()); })()}
          {#if previewDayIdx === dayIdx}
            {@const pc = AGENT_COLORS[pv.agent]}
            {@const pa = AGENTS.find(x => x.id === pv.agent)}
            {@const pTop = pv.hour * HOUR_PX}
            {@const pHeight = Math.max(20, pv.dur * HOUR_PX)}
            {#if pc}
              <div
                class="absolute rounded-sm overflow-hidden flex items-center border-l-[2.5px] border-dashed px-1 py-0.5 opacity-70 animate-pulse {pc.fillClass} {pc.edgeClass} {pc.textClass}"
                style="left:2px; right:2px; top:{pTop}px; height:{pHeight}px; z-index:15"
              >
                <span class="text-xs font-semibold overflow-hidden text-ellipsis whitespace-nowrap flex-1">{pa?.name ?? pv.agent}: {pv.label}</span>
              </div>
            {/if}
          {/if}
        {/if}
      </div>
    {/each}
  </div>
  </div>
  <DayDetailPane item={selectedItem} {createData} bind:preview {onopencanvas} onclose={() => { selected = null; createData = null; }} />
</div>
