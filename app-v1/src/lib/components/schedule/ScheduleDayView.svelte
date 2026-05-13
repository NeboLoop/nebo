<script>
  import { onMount } from 'svelte';
  import { AGENTS, SCHEDULE, agentColor, triggerGlyph, fmtTime, fmtHour, packLanes } from './data.js';
  import ScheduleDetailPane from './ScheduleDetailPane.svelte';

  let { enabled } = $props();

  const HOUR_START = 6, HOUR_END = 22;
  const TOTAL_HOURS = HOUR_END - HOUR_START;
  const HOUR_PX = 64;
  const TODAY = 3;
  const NOW = 14.5;

  let selected = $state(null);
  let scrollEl = $state(null);

  onMount(() => {
    if (scrollEl) scrollEl.scrollTop = (8 - HOUR_START) * HOUR_PX - 12;
  });

  const items = $derived(
    SCHEDULE
      .filter(s => s.days.includes(TODAY) && enabled[s.agent])
      .flatMap((s, sIdx) => s.hours.map((h, hIdx) => ({
        ...s, hour: h, end: h + s.dur, _id: `${sIdx}-${hIdx}`,
      })))
      .sort((a, b) => a.hour - b.hour || (b.end - b.hour) - (a.end - a.hour))
  );

  const packed = $derived(packLanes(items));
  const rulerMarks = Array.from({ length: TOTAL_HOURS }, (_, i) => HOUR_START + i);
  const selectedItem = $derived(selected ? packed.find(p => p._id === selected) : null);
</script>

<div class="flex-1 flex flex-col bg-base-100 min-h-0 overflow-hidden">
  <!-- summary strip -->
  <div class="flex items-center gap-3 px-4 py-2.5 border-b border-base-content/10 bg-base-200/50">
    <span class="font-mono text-[10px] uppercase tracking-wider text-base-content/50">{items.length} runs today</span>
  </div>

  <div class="flex-1 flex min-h-0">
    <!-- scrollable timeline -->
    <div class="flex-1 flex relative overflow-auto min-w-0" bind:this={scrollEl}>
      <!-- hour ruler -->
      <div class="w-16 shrink-0 border-r border-base-content/10 relative bg-base-200/50" style:height="{TOTAL_HOURS * HOUR_PX}px">
        {#each rulerMarks as h, i}
          <div class="absolute right-2.5 font-mono text-[10px] text-base-content/40 -translate-y-1/2 bg-base-200/50 px-0.5" style:top="{i * HOUR_PX}px">
            {fmtHour(h)}
          </div>
        {/each}
      </div>

      <!-- event well -->
      <div class="flex-1 relative" style="height:{TOTAL_HOURS * HOUR_PX}px; background-image:linear-gradient(to bottom, var(--color-base-content, #000) 1px, transparent 1px); background-size:100% {HOUR_PX}px; --tw-bg-opacity:0.05">
        <!-- now line -->
        <div class="absolute left-0 right-0 h-px bg-warning z-5 pointer-events-none" style:top="{(NOW - HOUR_START) * HOUR_PX}px">
          <span class="absolute -left-1 -top-1 w-2 h-2 rounded-full bg-warning"></span>
          <span class="absolute right-2.5 -top-4 font-mono text-[10px] text-warning bg-base-100 px-1">2:30 PM</span>
        </div>

        {#each packed as item}
          {@const top = (item.hour - HOUR_START) * HOUR_PX}
          {@const height = Math.max(20, item.dur * HOUR_PX)}
          {@const c = agentColor(item.agent)}
          {@const a = AGENTS.find(x => x.id === item.agent)}
          {@const widthPct = 100 / item.totalLanes}
          {@const leftPct = widthPct * item.lane}
          {@const isSelected = selected === item._id}
          <button
            class="absolute rounded text-left overflow-hidden cursor-pointer flex flex-col gap-0.5 transition-shadow"
            class:ring-2={isSelected}
            class:z-20={isSelected}
            style="left:calc({leftPct}% + 4px); width:calc({widthPct}% - 8px); top:{top}px; height:{height}px; background:{c.bg}; border-left:3px solid {c.ink}; color:{c.ink}; padding:4px 8px; {isSelected ? `--tw-ring-color:${c.ink}` : ''}"
            title="{item.label} · {a.name} · {fmtTime(item.hour)}"
            onclick={(e) => { e.stopPropagation(); selected = item._id; }}
          >
            <div class="flex items-baseline gap-1.5 whitespace-nowrap overflow-hidden leading-tight text-xs">
              <span class="font-mono text-[10px] opacity-85 shrink-0">{triggerGlyph(item.kind)}</span>
              <span class="font-semibold overflow-hidden text-ellipsis flex-1">{item.label}</span>
            </div>
            {#if height >= 32}
              <div class="font-mono text-[10px] opacity-75 flex items-center gap-1.5 whitespace-nowrap overflow-hidden">
                <span>{a.name}</span>
                <span>·</span>
                <span>{fmtTime(item.hour)}</span>
              </div>
            {/if}
          </button>
        {/each}
      </div>
    </div>

    <ScheduleDetailPane item={selectedItem} onclose={() => selected = null} />
  </div>
</div>
