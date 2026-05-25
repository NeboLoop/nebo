<script lang="ts">
  import { AGENT_COLORS } from '$lib/tokens.js';
  import { AGENTS } from '$lib/data.js';
  import { flattenForDate, attachRunData, userScheduleItems } from '$lib/stores/schedule.js';
  import DayDetailPane from './DayDetailPane.svelte';

  let { enabled, selectedDate } = $props();

  let selected = $state<string | null>(null);

  const DOWS = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
  const today = new Date();

  const cells = $derived.by(() => {
    const year = selectedDate.getFullYear();
    const month = selectedDate.getMonth();
    const firstDow = new Date(year, month, 1).getDay();
    const daysInMonth = new Date(year, month + 1, 0).getDate();
    const daysInPrev = new Date(year, month, 0).getDate();
    const result = [];
    for (let i = firstDow - 1; i >= 0; i--) result.push({ day: daysInPrev - i, offset: -1 });
    for (let d = 1; d <= daysInMonth; d++) result.push({ day: d, offset: 0 });
    while (result.length < 42) result.push({ day: result.length - firstDow - daysInMonth + 1, offset: 1 });
    return result;
  });

  function cellDate(cell: { day: number; offset: number }) {
    return new Date(selectedDate.getFullYear(), selectedDate.getMonth() + cell.offset, cell.day);
  }

  function isToday(cell: { day: number; offset: number }) {
    if (cell.offset !== 0) return false;
    return selectedDate.getFullYear() === today.getFullYear() &&
      selectedDate.getMonth() === today.getMonth() &&
      cell.day === today.getDate();
  }

  function cellItems(cell: { day: number; offset: number }) {
    if (cell.offset !== 0) return [];
    const d = cellDate(cell);
    const dow = d.getDay();
    const wd = dow === 0 ? 7 : dow;
    return attachRunData(flattenForDate(wd, enabled, $userScheduleItems));
  }

  const allItems = $derived(cells.flatMap(c => cellItems(c)));
  const selectedItem = $derived(selected ? allItems.find(p => p._id === selected) : null);
</script>

<div class="flex-1 flex bg-base-100 min-h-0 overflow-hidden">
  <div class="flex-1 flex flex-col min-h-0 min-w-0">
  <!-- dow header -->
  <div class="grid grid-cols-7 bg-base-200/50 border-b border-base-content/10">
    {#each DOWS as d}
      <div class="px-2.5 py-2 text-sm uppercase tracking-wider text-center text-base-content/70">{d}</div>
    {/each}
  </div>

  <!-- grid -->
  <div class="flex-1 grid grid-cols-7 grid-rows-6 min-h-0">
    {#each cells as cell, i}
      {@const isCur = cell.offset === 0}
      {@const todayCell = isToday(cell)}
      {@const items = cellItems(cell)}
      {@const visible = items.slice(0, 3)}
      {@const hidden = items.length - visible.length}
      <div
        class="p-1.5 flex flex-col gap-0.5 min-h-0 overflow-hidden {!isCur ? 'opacity-50' : ''} {todayCell ? 'bg-primary/5' : ''}"
        style:border-right={(i + 1) % 7 !== 0 ? '1px solid var(--color-base-300)' : 'none'}
        style:border-bottom={i < 35 ? '1px solid var(--color-base-300)' : 'none'}
      >
        <div class="flex items-center gap-1 mb-px">
          {#if todayCell}
            <span class="inline-flex items-center justify-center w-4.5 h-4.5 rounded-full bg-primary text-primary-content font-mono text-sm font-semibold">{cell.day}</span>
          {:else}
            <span class="text-xs font-semibold">{cell.day}</span>
          {/if}
        </div>

        {#each visible as item}
          {@const c = AGENT_COLORS[item.agent]}
          {@const a = AGENTS.find(x => x.id === item.agent)}
          {@const isHeartbeat = item.triggerType === 'heartbeat'}
          <button
            class="flex items-center text-xs px-1 py-px rounded-sm overflow-hidden border-l-2 cursor-pointer text-left transition-shadow {isHeartbeat ? 'opacity-50' : ''} {isHeartbeat ? '' : c.fillClass} {c.textClass} {c.edgeClass} {selected === item._id ? 'ring-2' : ''}"
            style="{isHeartbeat ? 'border-left-style:dashed;' : ''} {selected === item._id ? `--tw-ring-color:${c.edgeVar}` : ''}"
            onclick={(e) => { e.stopPropagation(); selected = item._id; }}
            title="{a?.name ?? item.agent} · {item.label}{isHeartbeat ? ` (every ${item.interval})` : ''}"
          >
            <span class="overflow-hidden whitespace-nowrap text-ellipsis font-medium flex-1">{isHeartbeat ? `↻ ${a?.name ?? item.agent} · ${item.interval}` : `${a?.name ?? item.agent}: ${item.label}`}</span>
            {#if item.run && !isHeartbeat}
              <span class="w-1.5 h-1.5 rounded-full shrink-0 ml-0.5 {item.run.status === 'success' ? 'bg-success' : item.run.status === 'failed' ? 'bg-error' : item.run.status === 'skipped' ? 'bg-warning' : 'bg-base-content/30'}"></span>
            {/if}
          </button>
        {/each}
        {#if hidden > 0}
          <div class="font-mono text-xs px-1">+{hidden} more</div>
        {/if}
      </div>
    {/each}
  </div>
  </div>
  <DayDetailPane item={selectedItem} onclose={() => selected = null} />
</div>
