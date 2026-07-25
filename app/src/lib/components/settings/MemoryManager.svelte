<script lang="ts">
  // Per-agent memory manager. Scopes the listing to one agent (the backend
  // resolves agent_id → that agent's memory scope), so each agent shows only
  // ITS memories — never the global pool. Layout mirrors the old global page
  // (stats, search, layer filter, key+value rows) plus the types explainer.
  import { t } from 'svelte-i18n';
  import { onMount } from 'svelte';
  import { listMemories, deleteMemory, type Memory } from '$lib/api/nebo';
  import Info from 'lucide-svelte/icons/info';
  import X from 'lucide-svelte/icons/x';
  import Trash2 from 'lucide-svelte/icons/trash-2';

  type MemoryRow = { id: string; layer: string; key: string; value: string; tags: string[] };

  let { agentId }: { agentId: string } = $props();

  let memories = $state<MemoryRow[]>([]);
  let loading = $state(true);
  let searchText = $state('');
  let layerFilter = $state('all');
  let showInfo = $state(false);
  let deletingId = $state<string | null>(null);
  let selected = $state<MemoryRow | null>(null);
  let confirmingDelete = $state(false);
  let confirmTimer: ReturnType<typeof setTimeout> | undefined;

  // Layers derive from live data ('project', etc. — not a fixed set). Known
  // layers sort first in a stable order; anything else is alphabetical after.
  const knownLayerOrder = ['tacit', 'daily', 'entity', 'project'];
  const presentLayers = $derived.by(() => {
    const distinct = [...new Set(memories.map((m) => m.layer))];
    distinct.sort((a, b) => {
      const ia = knownLayerOrder.indexOf(a);
      const ib = knownLayerOrder.indexOf(b);
      return (
        (ia === -1 ? knownLayerOrder.length : ia) - (ib === -1 ? knownLayerOrder.length : ib) ||
        a.localeCompare(b)
      );
    });
    return distinct;
  });
  const layers = $derived(['all', ...presentLayers]);

  function openMemory(mem: MemoryRow) {
    selected = mem;
    confirmingDelete = false;
    clearTimeout(confirmTimer);
  }

  function closeMemory() {
    selected = null;
    confirmingDelete = false;
    clearTimeout(confirmTimer);
  }

  // Pretty-print JSON values; otherwise split run-on "Label: text. Label: text."
  // strings into one line per labelled segment for readability.
  function formatValue(value: string): { isJson: boolean; text: string; lines: string[] } {
    const trimmed = value.trim();
    if (trimmed.startsWith('{') || trimmed.startsWith('[')) {
      try {
        return { isJson: true, text: JSON.stringify(JSON.parse(trimmed), null, 2), lines: [] };
      } catch {
        /* not valid JSON — fall through to raw text */
      }
    }
    return { isJson: false, text: value, lines: value.split(/(?<=\.)\s+(?=[A-Z][a-zA-Z-]{1,30}:)/) };
  }

  // Two-step delete: first click arms confirmation for 3s, second click deletes.
  async function handleDelete() {
    if (!selected) return;
    if (!confirmingDelete) {
      confirmingDelete = true;
      clearTimeout(confirmTimer);
      confirmTimer = setTimeout(() => (confirmingDelete = false), 3000);
      return;
    }
    clearTimeout(confirmTimer);
    const id = selected.id;
    deletingId = id;
    try {
      await deleteMemory(id);
      memories = memories.filter((m) => m.id !== id);
      closeMemory();
    } catch {
      /* leave it in place on failure */
      confirmingDelete = false;
    } finally {
      deletingId = null;
    }
  }

  const layerInfo: { key: string; label: string; blurb: string }[] = [
    { key: 'tacit', label: 'memoryManager.layerTacit', blurb: 'memoryManager.tacitBlurb' },
    { key: 'daily', label: 'memoryManager.layerDaily', blurb: 'memoryManager.dailyBlurb' },
    { key: 'entity', label: 'memoryManager.layerEntity', blurb: 'memoryManager.entityBlurb' }
  ];

  const layerFilterKeys: Record<string, string> = {
    all: 'settingsMemories.all',
    tacit: 'memoryManager.layerTacit',
    daily: 'memoryManager.layerDaily',
    entity: 'memoryManager.layerEntity',
  };

  const layerStatKeys: Record<string, string> = {
    tacit: 'memoryManager.statTacit',
    daily: 'memoryManager.statDaily',
    entity: 'memoryManager.statEntity',
  };

  // Layers without a dedicated i18n key (e.g. 'project') show the capitalized raw name.
  function capitalize(s: string): string {
    return s.charAt(0).toUpperCase() + s.slice(1);
  }

  async function load() {
    loading = true;
    try {
      const resp = await listMemories(200, 0, undefined, agentId);
      memories = (resp?.memories ?? []).map((m: Memory) => ({
        id: String(m.id),
        layer: (m.namespace || 'tacit').split('/')[0],
        key: m.key || '',
        value: m.value || '',
        tags: m.tags || [],
      }));
    } catch {
      memories = [];
    }
    loading = false;
  }

  onMount(load);
  // Reload when switching agents.
  $effect(() => { void agentId; load(); });

  const filtered = $derived(
    memories.filter((m) => {
      if (layerFilter !== 'all' && m.layer !== layerFilter) return false;
      const q = searchText.toLowerCase();
      if (q && !m.value.toLowerCase().includes(q) && !m.key.toLowerCase().includes(q)) return false;
      return true;
    })
  );

  const layerCounts = $derived(
    Object.fromEntries(presentLayers.map((l) => [l, memories.filter((m) => m.layer === l).length]))
  );

  const layerColors: Record<string, string> = {
    tacit: 'bg-[var(--agent-violet-bg)] text-[var(--agent-violet-ink)]',
    daily: 'bg-[var(--agent-sky-bg)] text-[var(--agent-sky-ink)]',
    entity: 'bg-[var(--agent-green-bg)] text-[var(--agent-green-ink)]',
  };
</script>

<div class="flex items-center gap-1.5 mb-1">
  <h3 class="text-xs font-semibold uppercase tracking-wider text-base-content/50">{$t('memoryManager.title')}</h3>
  <button
    type="button"
    onclick={() => (showInfo = true)}
    class="p-0.5 rounded-full text-base-content/40 hover:text-base-content hover:bg-base-200 transition-colors cursor-pointer"
    aria-label={$t('memoryManager.whatAreTypes')}
    title={$t('memoryManager.whatAreTypes')}
  >
    <Info class="w-3.5 h-3.5" />
  </button>
</div>
<p class="text-xs text-base-content/70 mb-3">{$t('memoryManager.subtitle')}</p>

{#if loading}
  <div class="text-xs text-base-content/50 py-6 text-center">{$t('settingsMemories.loadingMemories')}</div>
{:else}
  <!-- Stats -->
  <div class="flex flex-wrap gap-2.5 mb-4">
    <div class="px-3.5 py-2 rounded-lg bg-base-200/50 text-sm"><span class="font-mono font-bold">{memories.length}</span> {$t('memoryManager.statTotal')}</div>
    {#each presentLayers as layer}
      <div class="px-3.5 py-2 rounded-lg bg-base-200/50 text-sm"><span class="font-mono font-bold">{layerCounts[layer]}</span> {layerStatKeys[layer] ? $t(layerStatKeys[layer]) : layer}</div>
    {/each}
  </div>

  <!-- Search + filters -->
  <div class="flex gap-2 mb-4">
    <input type="text" placeholder={$t('settingsMemories.searchPlaceholder')} bind:value={searchText}
      class="flex-1 py-2 px-3 rounded-lg border border-base-content/25 bg-base-200/40 text-sm outline-none focus:border-base-content/50 placeholder:text-base-content/40" />
    <div class="flex flex-wrap gap-1 justify-end">
      {#each layers as layer}
        <button class="px-2.5 py-1.5 rounded-lg border text-sm cursor-pointer transition-colors {layerFilter === layer
          ? 'bg-primary/10 text-primary border-primary font-medium'
          : 'border-base-content/10 bg-base-100 hover:bg-base-200'}"
          onclick={() => (layerFilter = layer)}>
          {layerFilterKeys[layer] ? $t(layerFilterKeys[layer]) : capitalize(layer)}
        </button>
      {/each}
    </div>
  </div>

  <!-- Memory list -->
  {#if filtered.length === 0}
    <div class="text-xs text-base-content/50 py-8 text-center">{$t('memoryManager.noMemories')}</div>
  {:else}
    <div class="flex flex-col gap-1.5">
      {#each filtered as mem}
        <button
          type="button"
          onclick={() => openMemory(mem)}
          class="flex items-center gap-3 py-2.5 px-3.5 rounded-lg border border-base-content/5 bg-base-100 hover:bg-base-200/50 transition-colors cursor-pointer text-left w-full"
        >
          <span class="px-1.5 py-0.5 rounded text-[0.625rem] font-semibold uppercase tracking-wide shrink-0 {layerColors[mem.layer] ?? 'bg-base-200 text-base-content/70'}">{mem.layer}</span>
          <div class="flex-1 min-w-0">
            <span class="text-xs font-mono text-base-content/50">{mem.key}</span>
            <p class="text-sm truncate">{mem.value}</p>
          </div>
          <div class="flex gap-1 shrink-0">
            {#each mem.tags.slice(0, 2) as tag}
              <span class="px-1.5 py-0.5 rounded bg-base-200 text-xs">{tag}</span>
            {/each}
          </div>
        </button>
      {/each}
    </div>
  {/if}
{/if}

<svelte:window onkeydown={(e) => { if (e.key === 'Escape' && selected) closeMemory(); }} />

{#if selected}
  {@const formatted = formatValue(selected.value)}
  <div class="fixed inset-0 z-50 flex items-center justify-center p-4" role="dialog" aria-modal="true">
    <button type="button" class="absolute inset-0 bg-black/60 backdrop-blur-sm" onclick={closeMemory} aria-label={$t('common.close')}></button>
    <div class="relative bg-base-100 rounded-2xl border border-base-300 w-[min(92vw,36rem)] max-h-[85vh] flex flex-col">
      <!-- Header: layer badge + full key -->
      <div class="flex items-start justify-between gap-3 px-5 pt-5 pb-3">
        <div class="min-w-0">
          <span class="px-1.5 py-0.5 rounded text-[0.625rem] font-semibold uppercase tracking-wide {layerColors[selected.layer] ?? 'bg-base-200 text-base-content/70'}">{selected.layer}</span>
          <p class="text-xs font-mono text-base-content/50 break-all mt-1.5">{selected.key}</p>
        </div>
        <button type="button" onclick={closeMemory} class="p-1.5 rounded-full hover:bg-base-200 transition-colors cursor-pointer shrink-0" aria-label={$t('common.close')}>
          <X class="w-4 h-4" />
        </button>
      </div>

      <!-- Full value -->
      <div class="px-5 pb-4 overflow-y-auto min-h-0">
        {#if formatted.isJson}
          <pre class="text-xs font-mono whitespace-pre-wrap break-words">{formatted.text}</pre>
        {:else}
          <div class="flex flex-col gap-1.5">
            {#each formatted.lines as line}
              <p class="text-sm whitespace-pre-wrap break-words">{line}</p>
            {/each}
          </div>
        {/if}
        {#if selected.tags.length > 0}
          <div class="flex flex-wrap gap-1 mt-3">
            {#each selected.tags as tag}
              <span class="px-1.5 py-0.5 rounded bg-base-200 text-xs">{tag}</span>
            {/each}
          </div>
        {/if}
      </div>

      <!-- Footer: two-step delete -->
      <div class="flex items-center justify-end px-5 py-4 border-t border-base-content/10">
        <button
          type="button"
          onclick={handleDelete}
          disabled={deletingId === selected.id}
          class="flex items-center gap-1.5 px-4 py-2 rounded-lg text-sm font-medium cursor-pointer transition-colors disabled:opacity-50 {confirmingDelete
            ? 'bg-error text-error-content hover:brightness-110'
            : 'border border-base-content/10 text-error hover:bg-error/10'}"
        >
          <Trash2 class="w-3.5 h-3.5" />
          {confirmingDelete ? $t('memoryManager.confirmDelete') : $t('common.delete')}
        </button>
      </div>
    </div>
  </div>
{/if}

{#if showInfo}
  <div class="fixed inset-0 z-50 flex items-center justify-center">
    <button type="button" class="absolute inset-0 bg-black/60 backdrop-blur-sm" onclick={() => (showInfo = false)} aria-label={$t('common.close')}></button>
    <div class="relative bg-base-100 rounded-2xl border border-base-300 w-[min(92vw,28rem)] p-6">
      <div class="flex items-center justify-between mb-4">
        <h3 class="text-lg font-bold">{$t('memoryManager.howItWorks')}</h3>
        <button type="button" onclick={() => (showInfo = false)} class="p-1.5 rounded-full hover:bg-base-200 transition-colors cursor-pointer" aria-label={$t('common.close')}>
          <X class="w-4 h-4" />
        </button>
      </div>
      <p class="text-sm text-base-content/70 mb-4">{$t('memoryManager.howItWorksIntro')}</p>
      <div class="flex flex-col gap-3">
        {#each layerInfo as info}
          <div class="flex gap-3">
            <span class="px-2 py-0.5 h-fit rounded text-sm font-semibold font-mono uppercase shrink-0 {layerColors[info.key]}">{$t(info.label)}</span>
            <p class="text-sm text-base-content/70 leading-relaxed">{$t(info.blurb)}</p>
          </div>
        {/each}
      </div>
    </div>
  </div>
{/if}
