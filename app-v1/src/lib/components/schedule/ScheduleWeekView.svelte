<script>
  import { AGENTS, SCHEDULE, CAL_DAYS, agentColor, triggerGlyph, fmtTime, fmtHour } from './data.js';

  let { enabled } = $props();

  const HOUR_START = 6, HOUR_END = 22;
  const TOTAL_HOURS = HOUR_END - HOUR_START;
  const TODAY_IDX = 2;

  const rulerMarks = Array.from({ length: TOTAL_HOURS / 2 + 1 }, (_, i) => ({
    h: HOUR_START + i * 2,
    pct: (i / (TOTAL_HOURS / 2)) * 100,
  }));

  function dayItems(dayNum) {
    return SCHEDULE
      .filter(s => s.days.includes(dayNum) && enabled[s.agent])
      .flatMap(s => s.hours.map(h => ({ ...s, hour: h })));
  }

  function packDay(items) {
    const sorted = [...items].sort((a, b) => a.hour - b.hour);
    const lanes = [];
    sorted.forEach(it => {
      for (const lane of lanes) {
        const lastEnd = lane[lane.length - 1].hour + lane[lane.length - 1].dur;
        if (it.hour >= lastEnd) { lane.push(it); return; }
      }
      lanes.push([it]);
    });
    const total = lanes.length || 1;
    return lanes.flatMap((lane, idx) => lane.map(it => ({ ...it, lane: idx, totalLanes: total })));
  }

  function dateNum(i) { const d = 27 + i; return d > 30 ? d - 30 : d; }
</script>

<div class="flex-1 flex flex-col bg-base-100 min-h-0 overflow-hidden">
  <!-- day strip -->
  <div class="flex border-b border-base-content/10 bg-base-200/50">
    <div class="w-15 border-r border-base-content/10"></div>
    {#each CAL_DAYS as d, i}
      <div class="flex-1 px-3 py-2" class:bg-base-100={i === TODAY_IDX} style:border-right={i < CAL_DAYS.length - 1 ? '1px solid color-mix(in srgb, var(--color-base-content) 5%, transparent)' : 'none'}>
        <div class="font-mono text-[10px] text-base-content/40 uppercase tracking-wider">{d}</div>
        <div class="mt-0.5 {i === TODAY_IDX ? 'font-semibold text-base-content' : 'text-base-content/60'}">
          {#if i === TODAY_IDX}
            <span class="inline-flex items-center justify-center w-6 h-6 rounded-full bg-warning text-warning-content text-xs font-semibold">{dateNum(i)}</span>
          {:else}
            <span class="text-base">{dateNum(i)}</span>
          {/if}
        </div>
      </div>
    {/each}
  </div>

  <!-- body -->
  <div class="flex-1 flex relative overflow-hidden">
    <!-- ruler -->
    <div class="w-15 border-r border-base-content/10 relative bg-base-200/50">
      {#each rulerMarks as m}
        <div class="absolute right-2 font-mono text-[10px] text-base-content/40 -translate-y-1/2" style:top="{m.pct}%">{fmtHour(m.h)}</div>
      {/each}
    </div>

    <!-- day columns -->
    {#each CAL_DAYS as _, dayIdx}
      {@const dayNum = dayIdx + 1}
      {@const packed = packDay(dayItems(dayNum))}
      {@const isToday = dayIdx === TODAY_IDX}
      <div class="flex-1 relative" style:border-right={dayIdx < CAL_DAYS.length - 1 ? '1px solid color-mix(in srgb, var(--color-base-content) 5%, transparent)' : 'none'} style:background={isToday ? 'color-mix(in srgb, var(--color-base-content) 2%, transparent)' : ''}>
        {#if isToday}
          <div class="absolute left-0 right-0 h-px bg-warning z-4" style:top="{((14.5 - HOUR_START) / TOTAL_HOURS) * 100}%">
            <span class="absolute -left-1 -top-1 w-1.5 h-1.5 rounded-full bg-warning"></span>
          </div>
        {/if}

        {#each packed as item}
          {@const top = ((item.hour - HOUR_START) / TOTAL_HOURS) * 100}
          {@const height = Math.max(2.5, (item.dur / TOTAL_HOURS) * 100)}
          {@const c = agentColor(item.agent)}
          {@const widthPct = 100 / item.totalLanes}
          {@const leftPct = widthPct * item.lane}
          <div
            class="absolute rounded-sm overflow-hidden cursor-pointer flex flex-col"
            style="left:calc({leftPct}% + 2px); width:calc({widthPct}% - 4px); top:{top}%; height:{height}%; min-height:18px; background:{c.bg}; border-left:2.5px solid {c.ink}; color:{c.ink}; padding:2px 4px; font-size:9px"
            title="{item.label} · {AGENTS.find(a => a.id === item.agent).name}"
          >
            <div class="flex items-center gap-1 whitespace-nowrap overflow-hidden text-ellipsis font-semibold">
              <span class="font-mono text-[9px] opacity-80">{triggerGlyph(item.kind)}</span>
              <span class="overflow-hidden text-ellipsis">{item.label}</span>
            </div>
            {#if height >= 4.5}
              <span class="font-mono text-[8px] opacity-70">{fmtTime(item.hour)}</span>
            {/if}
          </div>
        {/each}
      </div>
    {/each}
  </div>
</div>
