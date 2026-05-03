<script lang="ts">
  import { onMount } from 'svelte';
  import type { Memory } from '$lib/api/nebo';

  let memories = $state<{ id: string; layer: string; value: string; tags: string[]; accessCount: number }[]>([]);
  let searchText = $state('');
  let layerFilter = $state('all');
  const layers = ['all', 'tacit', 'daily', 'entity'];

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.listMemories();
      if (resp?.memories?.length) {
        memories = resp.memories.map((m: Memory) => ({
          id: String(m.id),
          layer: m.namespace || 'tacit',
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
  <h2 class="text-lg font-bold mb-1">Memories</h2>
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
