<script lang="ts">
  import ColorDayView from './ColorDayView.svelte';
  import ColorWeekView from './ColorWeekView.svelte';
  import ColorMonthView from './ColorMonthView.svelte';

  const MONTHS = ['January','February','March','April','May','June','July','August','September','October','November','December'];
  const DAYS = ['Sunday','Monday','Tuesday','Wednesday','Thursday','Friday','Saturday'];

  let { view = $bindable('day'), selectedDate = $bindable(new Date()), enabled, onopencanvas }: { view: string; selectedDate: Date; enabled: Record<string, boolean>; onopencanvas?: (agentFull: string) => void } = $props();

  let showHeartbeats = $state(false);

  function goToday() { selectedDate = new Date(); }

  function goPrev() {
    const d = new Date(selectedDate);
    if (view === 'day') d.setDate(d.getDate() - 1);
    else if (view === 'week') d.setDate(d.getDate() - 7);
    else d.setMonth(d.getMonth() - 1);
    selectedDate = d;
  }
  function goNext() {
    const d = new Date(selectedDate);
    if (view === 'day') d.setDate(d.getDate() + 1);
    else if (view === 'week') d.setDate(d.getDate() + 7);
    else d.setMonth(d.getMonth() + 1);
    selectedDate = d;
  }

  const dateLabel = $derived.by(() => {
    const y = selectedDate.getFullYear();
    const m = MONTHS[selectedDate.getMonth()];
    if (view === 'day') {
      return `${m} ${selectedDate.getDate()}, ${y}`;
    } else if (view === 'week') {
      const start = new Date(selectedDate);
      start.setDate(start.getDate() - ((start.getDay() + 6) % 7)); // Monday
      const end = new Date(start);
      end.setDate(end.getDate() + 6);
      const sm = MONTHS[start.getMonth()];
      const em = MONTHS[end.getMonth()];
      if (start.getMonth() === end.getMonth()) {
        return `${sm} ${start.getDate()} – ${end.getDate()}, ${y}`;
      }
      return `${sm} ${start.getDate()} – ${em} ${end.getDate()}, ${end.getFullYear()}`;
    }
    return `${m} ${y}`;
  });

  const dateSub = $derived(view === 'day' ? DAYS[selectedDate.getDay()] : '');
</script>

<div class="flex-1 flex flex-col min-h-0 overflow-hidden">
  <!-- Header -->
  <div class="px-5 py-3.5 flex items-center gap-2 border-b border-base-content/10 shrink-0">
    <button class="w-7 h-7 rounded-md flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none text-lg" onclick={goPrev}>‹</button>
    <button class="w-7 h-7 rounded-md flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none text-lg" onclick={goNext}>›</button>

    <h1 class="text-xl font-bold tracking-tight ml-1">{dateLabel}</h1>
    {#if dateSub}
      <span class="text-xs text-base-content/70">{dateSub}</span>
    {/if}
    <div class="flex-1"></div>

    <!-- View toggle -->
    <div class="inline-flex bg-base-200/80 rounded-lg p-0.5 border border-base-content/5">
      {#each ['day', 'week', 'month'] as v}
        <button
          class="px-3.5 py-1 rounded-md text-sm cursor-pointer transition-all {view === v ? 'bg-base-100 font-medium text-base-content shadow-sm' : 'text-base-content'}"
          onclick={() => view = v}
        >{v[0].toUpperCase() + v.slice(1)}</button>
      {/each}
    </div>

    {#if view !== 'month'}
      <button
        class="w-7 h-7 rounded-md flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none text-sm {showHeartbeats ? 'text-primary bg-primary/10' : 'text-base-content/50'}"
        onclick={() => showHeartbeats = !showHeartbeats}
        title="{showHeartbeats ? 'Hide' : 'Show'} interval events"
      >↻</button>
    {/if}
    <button class="btn btn-ghost btn-sm text-sm" onclick={goToday}>Today</button>
  </div>

  <!-- Calendar content -->
  <div class="flex-1 min-h-0 flex">
    {#if view === 'day'}
      <ColorDayView {enabled} {selectedDate} {onopencanvas} {showHeartbeats} />
    {:else if view === 'week'}
      <ColorWeekView {enabled} {selectedDate} {onopencanvas} {showHeartbeats} />
    {:else}
      <ColorMonthView {enabled} {selectedDate} />
    {/if}
  </div>
</div>
