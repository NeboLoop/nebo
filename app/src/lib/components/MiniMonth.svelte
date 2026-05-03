<script lang="ts">
  const MONTHS = ['January','February','March','April','May','June','July','August','September','October','November','December'];
  const DOWS = ['S','M','T','W','T','F','S'];

  let { selectedDate, onselect } = $props();

  let displayYear = $state(selectedDate.getFullYear());
  let displayMonth = $state(selectedDate.getMonth());

  // Sync display when selectedDate changes externally
  $effect(() => {
    displayYear = selectedDate.getFullYear();
    displayMonth = selectedDate.getMonth();
  });

  const today = new Date();
  const todayKey = `${today.getFullYear()}-${today.getMonth()}-${today.getDate()}`;

  function prevMonth() {
    if (displayMonth === 0) { displayMonth = 11; displayYear--; }
    else { displayMonth--; }
  }
  function nextMonth() {
    if (displayMonth === 11) { displayMonth = 0; displayYear++; }
    else { displayMonth++; }
  }

  const cells = $derived.by(() => {
    const firstDow = new Date(displayYear, displayMonth, 1).getDay();
    const daysInMonth = new Date(displayYear, displayMonth + 1, 0).getDate();
    const daysInPrev = new Date(displayYear, displayMonth, 0).getDate();
    const result = [];
    for (let i = firstDow - 1; i >= 0; i--) result.push({ day: daysInPrev - i, offset: -1 });
    for (let d = 1; d <= daysInMonth; d++) result.push({ day: d, offset: 0 });
    while (result.length < 42) result.push({ day: result.length - firstDow - daysInMonth + 1, offset: 1 });
    return result;
  });

  function cellKey(cell) {
    const y = displayYear + (displayMonth + cell.offset > 11 ? 1 : displayMonth + cell.offset < 0 ? -1 : 0);
    const m = (displayMonth + cell.offset + 12) % 12;
    return `${y}-${m}-${cell.day}`;
  }

  function selectCell(cell) {
    const m = displayMonth + cell.offset;
    const date = new Date(displayYear, m, cell.day);
    onselect(date);
  }

  const selectedKey = $derived(`${selectedDate.getFullYear()}-${selectedDate.getMonth()}-${selectedDate.getDate()}`);
</script>

<div class="px-4 py-3 border-t border-base-content/5">
  <div class="flex items-center justify-between mb-1.5">
    <button class="text-sm cursor-pointer bg-transparent border-none hover:text-primary transition-colors" onclick={prevMonth}>‹</button>
    <span class="text-sm font-medium">{MONTHS[displayMonth]} {displayYear}</span>
    <button class="text-sm cursor-pointer bg-transparent border-none hover:text-primary transition-colors" onclick={nextMonth}>›</button>
  </div>
  <div class="grid grid-cols-7 gap-0.5">
    {#each DOWS as d}
      <div class="text-sm font-mono text-center text-base-content/60">{d}</div>
    {/each}
    {#each cells as c}
      {@const key = cellKey(c)}
      {@const isToday = key === todayKey}
      {@const isSelected = key === selectedKey}
      <button
        class="text-sm font-mono text-center py-0.5 rounded-sm cursor-pointer border-none transition-colors
          {c.offset !== 0 ? 'text-base-content/40' : 'text-base-content'}
          {isSelected ? 'bg-primary text-primary-content font-semibold' : isToday ? 'ring-2 ring-primary font-semibold' : 'hover:bg-base-300'}"
        onclick={() => selectCell(c)}
      >{c.day}</button>
    {/each}
  </div>
</div>
