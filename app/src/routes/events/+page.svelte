<script lang="ts">
  import { onMount } from 'svelte';
  import Sidebar from '$lib/components/Sidebar.svelte';
  import { EVENT_COLORS } from '$lib/tokens.js';

  let events = $state<{ id: string; type: string; source: string; payload: string; time: string }[]>([]);
  let filter = $state('all');
  const types = ['all', 'agent', 'workflow', 'tool', 'error'];
  const filtered = $derived(filter === 'all' ? events : events.filter(e => e.type === filter));

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.listEventSources();
      if (resp?.sources?.length) {
        events = resp.sources.map((e) => ({
          id: e.value,
          type: e.kind || 'agent',
          source: e.value,
          payload: e.description || e.label,
          time: '',
        }));
      }
    } catch { /* keep mock data */ }
  });
</script>

<svelte:head><title>Events - Nebo</title></svelte:head>

<div class="flex h-screen bg-base-100 text-base-content text-sm">
  <Sidebar activePage="events" />
  <div class="flex-1 flex flex-col min-w-0 min-h-0">
    <div class="h-12 px-5 border-b border-base-content/10 flex items-center gap-3.5 shrink-0">
      <span class="text-sm font-semibold">Events</span>
      <div class="ml-auto h-7 w-[200px] rounded-md border border-base-content/10 bg-base-100 flex items-center px-2.5 gap-2 text-sm">
        <span class="font-mono">⌘K</span><span>Search or run…</span>
      </div>
    </div>

    <div class="flex-1 overflow-auto p-6">
      <div class="max-w-[800px]">
        <h1 class="text-xl font-bold tracking-tight mb-4">System Events</h1>

        <div class="flex gap-1.5 mb-4">
          {#each types as t}
            <button class="px-3 py-1 rounded-2xl border text-sm cursor-pointer transition-colors {filter === t
              ? 'bg-primary/10 text-primary border-primary'
              : 'border-base-content/10 bg-base-100 hover:border-base-content/20'}"
              onclick={() => filter = t}>
              {t.charAt(0).toUpperCase() + t.slice(1)}
            </button>
          {/each}
        </div>

        <div class="flex flex-col gap-1.5">
          {#each filtered as event}
            {@const c = EVENT_COLORS[event.type]}
            <div class="flex items-center gap-3 py-2.5 px-3.5 rounded-lg border border-base-content/5 bg-base-100">
              <span class="px-2 py-0.5 rounded text-sm font-semibold font-mono uppercase tracking-wide shrink-0 {c.bgClass} {c.textClass}">{event.type}</span>
              <span class="text-sm font-medium w-[100px] shrink-0">{event.source}</span>
              <span class="flex-1 text-sm truncate">{event.payload}</span>
              <span class="font-mono text-xs shrink-0">{event.time}</span>
            </div>
          {/each}
        </div>
      </div>
    </div>
  </div>
</div>
