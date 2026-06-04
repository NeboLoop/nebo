<script lang="ts">
  import { onMount } from 'svelte';
  import type { Memory } from '$lib/api/nebo';
  import Info from 'lucide-svelte/icons/info';
  import X from 'lucide-svelte/icons/x';

  let memories = $state<{ id: string; layer: string; value: string; tags: string[]; accessCount: number }[]>([]);
  let searchText = $state('');
  let layerFilter = $state('all');
  let showInfo = $state(false);
  const layers = ['all', 'tacit', 'daily', 'entity'];

  // Plain-language explanation of each memory layer, shown in the info modal.
  const layerInfo: { key: string; label: string; blurb: string }[] = [
    { key: 'tacit', label: 'Tacit', blurb: 'Lasting facts about you and how you work — your preferences, role, and the people and projects that keep coming up. Your companion leans on these to act the way you’d want.' },
    { key: 'daily', label: 'Daily', blurb: 'Day-to-day notes tied to a specific date — what happened and what’s coming up. These power briefings and follow-ups, and naturally age out over time.' },
    { key: 'entity', label: 'Entity', blurb: 'What your companion knows about the specific people, companies, and things you deal with — so it has context the next time they come up.' }
  ];

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.listMemories();
      if (resp?.memories?.length) {
        memories = resp.memories.map((m: Memory) => ({
          id: String(m.id),
          // Namespaces are stored as "<layer>/<sub>" (e.g. "tacit/general").
          // The layer is the prefix — counts, filter chips, and the badge all
          // key off it, so a full path here zeroed every per-layer count.
          layer: (m.namespace || 'tacit').split('/')[0],
          value: m.value || '',
          tags: m.tags || [],
          accessCount: m.accessCount ?? 0,
        }));
      }
    } catch { /* keep mock data */ }
  });

  const filtered = $derived(
    memories.filter(m => {
      if (layerFilter !== 'all' && m.layer !== layerFilter) return false;
      if (searchText && !m.value.toLowerCase().includes(searchText.toLowerCase())) return false;
      return true;
    })
  );

  const stats = $derived({
    total: memories.length,
    tacit: memories.filter(m => m.layer === 'tacit').length,
    daily: memories.filter(m => m.layer === 'daily').length,
    entity: memories.filter(m => m.layer === 'entity').length,
  });

  const layerColors: Record<string, string> = {
    tacit: 'bg-[var(--agent-violet-bg)] text-[var(--agent-violet-ink)]',
    daily: 'bg-[var(--agent-sky-bg)] text-[var(--agent-sky-ink)]',
    entity: 'bg-[var(--agent-green-bg)] text-[var(--agent-green-ink)]',
  };
</script>

<div class="mb-7">
  <div class="flex items-center gap-1.5 mb-1">
    <h2 class="text-lg font-bold">Memories</h2>
    <button
      type="button"
      onclick={() => (showInfo = true)}
      class="p-0.5 rounded-full text-base-content/40 hover:text-base-content hover:bg-base-200 transition-colors cursor-pointer"
      aria-label="What are the memory types?"
      title="What are the memory types?"
    >
      <Info class="w-4 h-4" />
    </button>
  </div>
  <p class="text-xs text-base-content/70">View and manage your agent's stored knowledge.</p>
</div>

<!-- Stats -->
<div class="flex gap-2.5 mb-4">
  <div class="px-3.5 py-2 rounded-lg bg-base-200/50 text-sm"><span class="font-mono font-bold">{stats.total}</span> total</div>
  <div class="px-3.5 py-2 rounded-lg bg-base-200/50 text-sm"><span class="font-mono font-bold">{stats.tacit}</span> tacit</div>
  <div class="px-3.5 py-2 rounded-lg bg-base-200/50 text-sm"><span class="font-mono font-bold">{stats.daily}</span> daily</div>
  <div class="px-3.5 py-2 rounded-lg bg-base-200/50 text-sm"><span class="font-mono font-bold">{stats.entity}</span> entity</div>
</div>

<!-- Search + filters -->
<div class="flex gap-2 mb-4">
  <input type="text" placeholder="Search memories…" bind:value={searchText}
    class="flex-1 py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30 placeholder:text-base-content" />
  <div class="flex gap-1">
    {#each layers as layer}
      <button class="px-2.5 py-1.5 rounded-lg border text-sm cursor-pointer transition-colors {layerFilter === layer
        ? 'bg-primary/10 text-primary border-primary font-medium'
        : 'border-base-content/10 bg-base-100 hover:bg-base-200'}"
        onclick={() => layerFilter = layer}>
        {layer.charAt(0).toUpperCase() + layer.slice(1)}
      </button>
    {/each}
  </div>
</div>

<!-- Memory list -->
<div class="flex flex-col gap-1.5">
  {#each filtered as mem}
    <div class="flex items-center gap-3 py-2.5 px-3.5 rounded-lg border border-base-content/5 bg-base-100 cursor-pointer hover:border-base-content/15 transition-colors">
      <span class="px-2 py-0.5 rounded text-sm font-semibold font-mono uppercase shrink-0 {layerColors[mem.layer]}">{mem.layer}</span>
      <span class="flex-1 text-sm truncate">{mem.value}</span>
      <div class="flex gap-1 shrink-0">
        {#each mem.tags.slice(0, 2) as tag}
          <span class="px-1.5 py-0.5 rounded bg-base-200 text-sm">{tag}</span>
        {/each}
      </div>
      <span class="font-mono text-xs shrink-0">{mem.accessCount}×</span>
    </div>
  {/each}
</div>

<!-- Memory-types explainer -->
{#if showInfo}
  <div class="fixed inset-0 z-50 flex items-center justify-center">
    <button type="button" class="absolute inset-0 bg-black/60 backdrop-blur-sm" onclick={() => (showInfo = false)} aria-label="Close"></button>
    <div class="relative bg-base-100 rounded-2xl border border-base-300 w-full max-w-md mx-4 p-6">
      <div class="flex items-center justify-between mb-4">
        <h3 class="text-lg font-bold">How memory works</h3>
        <button type="button" onclick={() => (showInfo = false)} class="p-1.5 rounded-full hover:bg-base-200 transition-colors cursor-pointer" aria-label="Close">
          <X class="w-4 h-4" />
        </button>
      </div>
      <p class="text-sm text-base-content/70 mb-4">Your companion remembers across conversations in three layers:</p>
      <div class="flex flex-col gap-3">
        {#each layerInfo as info}
          <div class="flex gap-3">
            <span class="px-2 py-0.5 h-fit rounded text-sm font-semibold font-mono uppercase shrink-0 {layerColors[info.key]}">{info.label}</span>
            <p class="text-sm text-base-content/70 leading-relaxed">{info.blurb}</p>
          </div>
        {/each}
      </div>
    </div>
  </div>
{/if}
