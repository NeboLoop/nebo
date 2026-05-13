<script>
  import { SCHEDULE, agentColor, triggerGlyph, fmtTime } from './data.js';

  let { enabled } = $props();

  const TODAY = 29;
  const dows = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];

  const cells = [];
  for (let d = 29; d <= 31; d++) cells.push({ day: d, month: 'prev' });
  for (let d = 1;  d <= 30; d++) cells.push({ day: d, month: 'cur' });
  while (cells.length < 42) cells.push({ day: cells.length - 32, month: 'next' });

  function gridIdxToWd(i) { const dow = i % 7; return dow === 0 ? 7 : dow; }

  function dayItems(i) {
    if (cells[i].month !== 'cur') return [];
    const wd = gridIdxToWd(i);
    return SCHEDULE
      .filter(s => s.days.includes(wd) && enabled[s.agent])
      .flatMap(s => s.hours.map(h => ({ ...s, hour: h })))
      .sort((a, b) => a.hour - b.hour);
  }
</script>

<div class="flex-1 flex flex-col bg-base-100 min-h-0 overflow-hidden">
  <!-- dow header -->
  <div class="grid grid-cols-7 bg-base-200/50 border-b border-base-content/10">
    {#each dows as d}
      <div class="px-2.5 py-2 font-mono text-[10px] text-base-content/40 uppercase tracking-wider text-center">{d}</div>
    {/each}
  </div>

  <!-- grid -->
  <div class="flex-1 grid grid-cols-7 grid-rows-6 min-h-0">
    {#each cells as cell, i}
      {@const isCur = cell.month === 'cur'}
      {@const isToday = isCur && cell.day === TODAY}
      {@const items = dayItems(i)}
      {@const visible = items.slice(0, 6)}
      {@const hidden = items.length - visible.length}
      <div
        class="p-1.5 flex flex-col gap-0.5 min-h-0 overflow-hidden"
        class:opacity-40={!isCur}
        class:bg-base-200/30={isToday}
        style:border-right={(i + 1) % 7 !== 0 ? '1px solid color-mix(in srgb, var(--color-base-content) 5%, transparent)' : 'none'}
        style:border-bottom={i < 35 ? '1px solid color-mix(in srgb, var(--color-base-content) 5%, transparent)' : 'none'}
      >
        <div class="flex items-center gap-1 mb-px">
          {#if isToday}
            <span class="inline-flex items-center justify-center w-4.5 h-4.5 rounded-full bg-warning text-warning-content font-mono text-[10px] font-semibold">{cell.day}</span>
          {:else}
            <span class="font-mono text-[11px] font-medium text-base-content/60">{cell.day}</span>
          {/if}
        </div>

        {#each visible as item}
          {@const c = agentColor(item.agent)}
          <div
            class="flex items-center gap-1 text-[9px] px-1 py-px rounded-sm overflow-hidden"
            style="background:{c.bg}; color:{c.ink}; border-left:2px solid {c.ink}"
          >
            <span class="font-mono text-[8px] opacity-80 shrink-0">{triggerGlyph(item.kind)}</span>
            <span class="flex-1 overflow-hidden whitespace-nowrap text-ellipsis font-medium">{item.label}</span>
            <span class="font-mono text-[8px] opacity-70 shrink-0">{fmtTime(item.hour).replace(' ', '').toLowerCase()}</span>
          </div>
        {/each}
        {#if hidden > 0}
          <div class="font-mono text-[9px] text-base-content/40 px-1">+{hidden} more</div>
        {/if}
      </div>
    {/each}
  </div>
</div>
