<script>
  const TODAY = 29;
  const dows = ['S','M','T','W','T','F','S'];
  const cells = [];
  for (let d = 29; d <= 31; d++) cells.push({ day: d, month: 'prev' });
  for (let d = 1;  d <= 30; d++) cells.push({ day: d, month: 'cur' });
  while (cells.length < 42) cells.push({ day: cells.length - 32, month: 'next' });
</script>

<div class="px-4 py-3 border-t border-base-content/5">
  <div class="flex items-center justify-between mb-1.5">
    <span class="font-mono text-[11px] text-base-content/40">‹</span>
    <span class="text-[11px] font-medium text-base-content/60">April 2026</span>
    <span class="font-mono text-[11px] text-base-content/40">›</span>
  </div>
  <div class="grid grid-cols-7 gap-0.5">
    {#each dows as d}
      <div class="text-[9px] font-mono text-base-content/40 text-center">{d}</div>
    {/each}
    {#each cells as c}
      <div
        class="text-[10px] font-mono text-center py-0.5 rounded-sm"
        class:opacity-50={c.month !== 'cur'}
        class:text-base-content/60={c.month === 'cur' && !(c.month === 'cur' && c.day === TODAY)}
        class:text-base-content/40={c.month !== 'cur'}
        class:bg-warning={c.month === 'cur' && c.day === TODAY}
        class:text-warning-content={c.month === 'cur' && c.day === TODAY}
        class:font-semibold={c.month === 'cur' && c.day === TODAY}
      >{c.day}</div>
    {/each}
  </div>
</div>
