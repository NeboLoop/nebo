<script lang="ts">
  import { onMount } from 'svelte';
  import { AGENT_COLORS } from '$lib/tokens.js';
  import { AGENTS } from '$lib/data.js';
  import { packLanes } from '$lib/utils.js';
  import { flattenForDate, attachRunData, snapTo15, userScheduleItems } from '$lib/stores/schedule.js';
  import type { CalendarItem } from '$lib/stores/schedule.js';
  import DayDetailPane from './DayDetailPane.svelte';

  let { enabled, selectedDate, onopencanvas, showHeartbeats = false }: { enabled: Record<string, boolean>; selectedDate: Date; onopencanvas?: (agentFull: string) => void; showHeartbeats?: boolean } = $props();

  const HOUR_PX = 80;

  let now = $state(new Date());
  const nowHour = $derived(now.getHours() + now.getMinutes() / 60);
  const nowLabel = $derived(`${now.getHours() % 12 || 12}:${String(now.getMinutes()).padStart(2, '0')}`);

  const isToday = $derived(
    selectedDate.getFullYear() === now.getFullYear() &&
    selectedDate.getMonth() === now.getMonth() &&
    selectedDate.getDate() === now.getDate()
  );

  const weekday = $derived(selectedDate.getDay() === 0 ? 7 : selectedDate.getDay());

  let selected = $state<string | null>(null);
  let createData = $state<{ hour: number; date: Date } | null>(null);
  let preview = $state<{ agent: string; hour: number; dur: number; label: string } | null>(null);
  let scrollEl = $state<HTMLDivElement | null>(null);

  $effect(() => {
    const interval = setInterval(() => { now = new Date(); }, 60_000);
    return () => clearInterval(interval);
  });

  onMount(() => {
    if (scrollEl) scrollEl.scrollTop = 7 * HOUR_PX;
  });

  const allItems = $derived(
    attachRunData(flattenForDate(weekday, enabled, $userScheduleItems))
  );

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

  const packed = $derived.by(() => {
    const regular = allItems.filter(i => i.triggerType !== 'heartbeat');
    if (!showHeartbeats) return packLanes(regular);
    const expanded = allItems.filter(i => i.triggerType === 'heartbeat').flatMap(expandBand);
    return packLanes([...regular, ...expanded]);
  });
  const rulerMarks = Array.from({ length: 24 }, (_, i) => i);
  const selectedItem = $derived(selected ? packed.find(p => p._id === selected) ?? null : null);

  function handleWellClick(e: MouseEvent) {
    if (e.target !== e.currentTarget) return;
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const y = e.clientY - rect.top;
    const hour = snapTo15(y / HOUR_PX);
    selected = null;
    createData = { hour, date: new Date(selectedDate) };
  }
</script>

<div class="flex-1 flex flex-col bg-base-100 min-h-0 overflow-hidden">
  <div class="flex-1 flex min-h-0">
    <div class="flex-1 overflow-y-auto" bind:this={scrollEl}>
      <div class="flex relative" style="height:{24 * HOUR_PX}px">
        <!-- hour ruler -->
        <div class="w-18 border-r border-base-content/10 relative shrink-0 bg-base-200/50">
          {#each rulerMarks as h, i}
            <div
              class="absolute right-3 -translate-y-1/2 text-xs text-base-content/70"
              style="top:{i * HOUR_PX}px"
            >
              {#if h === 12}
                <span class="font-medium">Noon</span>
              {:else}
                <span class="font-medium">{h === 0 ? 12 : h <= 12 ? h : h - 12}</span>
                <span class="text-base-content/60 ml-0.5">{h < 12 ? 'AM' : 'PM'}</span>
              {/if}
            </div>
          {/each}

          {#if isToday}
            <div class="absolute left-0 right-0 z-20" style="top:{nowHour * HOUR_PX}px">
              <span class="absolute right-1 -translate-y-1/2 px-1.5 py-0.5 rounded-field bg-error text-error-content text-sm font-semibold">{nowLabel}</span>
            </div>
          {/if}
        </div>

        <!-- event well -->
        <div
          class="flex-1 relative cursor-pointer"
          style="height:{24 * HOUR_PX}px; background-image:repeating-linear-gradient(to bottom, var(--color-base-300) 0px, var(--color-base-300) 1px, transparent 1px, transparent {HOUR_PX / 4}px, var(--color-base-content-10, transparent) {HOUR_PX / 4}px, var(--color-base-content-10, transparent) {HOUR_PX / 4}px, transparent {HOUR_PX / 4}px, transparent {HOUR_PX / 2}px, var(--color-base-content-10, transparent) {HOUR_PX / 2}px, var(--color-base-content-10, transparent) {HOUR_PX / 2}px, transparent {HOUR_PX / 2}px, transparent {HOUR_PX * 3 / 4}px, var(--color-base-content-10, transparent) {HOUR_PX * 3 / 4}px, var(--color-base-content-10, transparent) {HOUR_PX * 3 / 4}px, transparent {HOUR_PX * 3 / 4}px, transparent {HOUR_PX}px); background-size:100% {HOUR_PX}px"
          ondblclick={handleWellClick}
          role="button"
          tabindex="-1"
        >
          {#if isToday}
            <div class="absolute left-0 right-0 h-0.5 bg-error z-10" style="top:{nowHour * HOUR_PX}px"></div>
          {/if}

          <!-- Event items (z-index:2+) -->
          {#each packed as item}
            {@const top = item.hour * HOUR_PX}
            {@const height = Math.max(32, item.dur * HOUR_PX)}
            {@const c = AGENT_COLORS[item.agent]}
            {@const a = AGENTS.find(x => x.id === item.agent)}
            {@const indent = item.totalLanes > 1 ? Math.min(20, 80 / item.totalLanes) : 0}
            {@const leftPct = item.lane * indent}
            {@const widthPct = item.totalLanes > 1 ? 100 - leftPct - (item.lane < item.totalLanes - 1 ? 5 : 0) : 100}
            {@const isSelected = selected === item._id}
            {@const zBase = 2 + item.lane}
            {@const isHeartbeat = item.triggerType === 'heartbeat'}
            <div
              class="absolute rounded-sm overflow-hidden cursor-pointer flex items-start transition-shadow border-l-[3px] px-1.5 pt-[3px] {c.fillClass} {c.edgeClass} {c.textClass}"
              class:ring-2={isSelected}
              style="left:calc({leftPct}% + 4px); width:calc({widthPct}% - 8px); top:{top}px; height:{height}px; {isHeartbeat ? 'border-left-style:dashed; ' : ''}{isSelected ? `--tw-ring-color:${c.edgeVar}; z-index:20` : `z-index:${zBase}`}"
              title="{a?.name ?? item.agent}: {item.label}{isHeartbeat && item.interval ? ` (every ${item.interval})` : ''}"
              onclick={(e) => { e.stopPropagation(); selected = item._id; createData = null; }}
              onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); selected = item._id; createData = null; } }}
              role="button"
              tabindex="0"
            >
              <span class="text-xs font-semibold overflow-hidden text-ellipsis whitespace-nowrap flex-1">{isHeartbeat ? '↻ ' : ''}{a?.name ?? item.agent}: {item.label}</span>
              {#if item.run}
                <span class="w-1.5 h-1.5 rounded-full shrink-0 ml-1 mt-1 {item.run.status === 'success' ? 'bg-success' : item.run.status === 'failed' ? 'bg-error' : item.run.status === 'skipped' ? 'bg-warning' : 'bg-base-content/30'}"></span>
              {/if}
            </div>
          {/each}

          <!-- Live preview block while creating -->
          {#if preview}
            {@const pv = preview}
            {@const pc = AGENT_COLORS[pv.agent]}
            {@const pa = AGENTS.find(x => x.id === pv.agent)}
            {@const pTop = pv.hour * HOUR_PX}
            {@const pHeight = Math.max(20, pv.dur * HOUR_PX)}
            {#if pc}
              <div
                class="absolute rounded-sm overflow-hidden flex items-start border-l-[3px] border-dashed px-1.5 py-[3px] opacity-70 animate-pulse {pc.fillClass} {pc.edgeClass} {pc.textClass}"
                style="left:4px; right:4px; top:{pTop}px; height:{pHeight}px; z-index:15"
              >
                <span class="text-xs font-semibold overflow-hidden text-ellipsis whitespace-nowrap flex-1">{pa?.name ?? pv.agent}: {pv.label}</span>
              </div>
            {/if}
          {/if}
        </div>
      </div>
    </div>

    <DayDetailPane item={selectedItem} {createData} bind:preview {onopencanvas} onclose={() => { selected = null; createData = null; }} />
  </div>
</div>
