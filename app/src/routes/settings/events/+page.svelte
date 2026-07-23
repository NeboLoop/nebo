<script lang="ts">
  import SettingsHeader from '$lib/components/settings/SettingsHeader.svelte';
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { EVENT_COLORS } from '$lib/tokens.js';

  let events = $state<{ id: string; type: string; source: string; payload: string; time: string }[]>([]);
  let filter = $state('all');
  // Chips come from the kinds actually present, so they always match the data.
  const types = $derived(['all', ...Array.from(new Set(events.map(e => e.type))).sort()]);
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
    } catch { /* keep empty */ }
  });
</script>

<SettingsHeader title={$t('settingsEvents.title')} description={$t('settingsEvents.description')} />

<!-- Filters -->
<div class="flex gap-1.5 mb-4 flex-wrap">
  {#each types as kind}
    <button
      class="px-3 py-1 rounded-2xl border text-sm cursor-pointer transition-colors {filter === kind
        ? 'bg-primary/10 text-primary border-primary'
        : 'border-base-content/10 bg-base-100 hover:border-base-content/20'}"
      onclick={() => filter = kind}
    >
      {kind === 'all' ? $t('settingsMemories.all') : kind.charAt(0).toUpperCase() + kind.slice(1)}
    </button>
  {/each}
</div>

<!-- Events -->
<div class="flex flex-col gap-1.5 mb-7">
  {#each filtered as event}
    {@const c = EVENT_COLORS[event.type] ?? EVENT_COLORS.agent}
    <div class="flex items-center justify-between gap-4 p-3 rounded-lg border border-base-content/5 bg-base-100">
      <div class="flex items-center gap-3 min-w-0">
        <span class="px-2 py-0.5 rounded text-sm font-semibold font-mono uppercase tracking-wide shrink-0 {c.bgClass} {c.textClass}">{event.type}</span>
        <div class="min-w-0">
          <div class="text-sm font-medium truncate">{event.source}</div>
          <div class="text-xs text-base-content/60 truncate">{event.payload}</div>
        </div>
      </div>
      {#if event.time}
        <span class="font-mono text-xs text-base-content/50 shrink-0">{event.time}</span>
      {/if}
    </div>
  {/each}
  {#if filtered.length === 0}
    <div class="rounded-xl border border-base-300 px-4 py-6 text-center text-sm text-base-content/60">{$t('settingsEvents.noEvents')}</div>
  {/if}
</div>
