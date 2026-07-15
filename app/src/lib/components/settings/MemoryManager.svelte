<script lang="ts">
  // Per-agent memory manager. Scopes the listing to one agent (the backend
  // resolves agent_id → that agent's memory scope), so each agent shows only
  // ITS memories — never the global pool. Layout mirrors the old global page
  // (stats, search, layer filter, key+value rows) plus the types explainer.
  import { onMount } from 'svelte';
  import { listMemories, deleteMemory, type Memory } from '$lib/api/nebo';
  import Info from 'lucide-svelte/icons/info';
  import X from 'lucide-svelte/icons/x';
  import Trash2 from 'lucide-svelte/icons/trash-2';

  let { agentId }: { agentId: string } = $props();

  let memories = $state<{ id: string; layer: string; key: string; value: string; tags: string[] }[]>([]);
  let loading = $state(true);
  let searchText = $state('');
  let layerFilter = $state('all');
  let showInfo = $state(false);
  let deletingId = $state<string | null>(null);
  const layers = ['all', 'tacit', 'daily', 'entity'];

  async function removeMemory(id: string) {
    deletingId = id;
    try {
      await deleteMemory(id);
      memories = memories.filter((m) => m.id !== id);
    } catch {
      /* leave it in place on failure */
    } finally {
      deletingId = null;
    }
  }

  const layerInfo: { key: string; label: string; blurb: string }[] = [
    { key: 'tacit', label: 'Tacit', blurb: 'Lasting facts about you and how this agent should work — preferences, role, and the people and projects that keep coming up.' },
    { key: 'daily', label: 'Daily', blurb: 'Day-to-day notes tied to a specific date — what happened and what’s coming up. These age out over time.' },
    { key: 'entity', label: 'Entity', blurb: 'What this agent knows about the specific people, companies, and things it deals with.' }
  ];

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

  const stats = $derived({
    total: memories.length,
    tacit: memories.filter((m) => m.layer === 'tacit').length,
    daily: memories.filter((m) => m.layer === 'daily').length,
    entity: memories.filter((m) => m.layer === 'entity').length,
  });

  const layerColors: Record<string, string> = {
    tacit: 'bg-[var(--agent-violet-bg)] text-[var(--agent-violet-ink)]',
    daily: 'bg-[var(--agent-sky-bg)] text-[var(--agent-sky-ink)]',
    entity: 'bg-[var(--agent-green-bg)] text-[var(--agent-green-ink)]',
  };
</script>

<div class="flex items-center gap-1.5 mb-1">
  <h3 class="text-xs font-semibold uppercase tracking-wider text-base-content/50">Memory Banks</h3>
  <button
    type="button"
    onclick={() => (showInfo = true)}
    class="p-0.5 rounded-full text-base-content/40 hover:text-base-content hover:bg-base-200 transition-colors cursor-pointer"
    aria-label="What are the memory types?"
    title="What are the memory types?"
  >
    <Info class="w-3.5 h-3.5" />
  </button>
</div>
<p class="text-xs text-base-content/70 mb-3">This agent's stored knowledge — private to it, plus your shared preferences.</p>

{#if loading}
  <div class="text-xs text-base-content/50 py-6 text-center">Loading memories…</div>
{:else}
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
      class="flex-1 py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30 placeholder:text-base-content/40" />
    <div class="flex gap-1">
      {#each layers as layer}
        <button class="px-2.5 py-1.5 rounded-lg border text-sm cursor-pointer transition-colors {layerFilter === layer
          ? 'bg-primary/10 text-primary border-primary font-medium'
          : 'border-base-content/10 bg-base-100 hover:bg-base-200'}"
          onclick={() => (layerFilter = layer)}>
          {layer.charAt(0).toUpperCase() + layer.slice(1)}
        </button>
      {/each}
    </div>
  </div>

  <!-- Memory list -->
  {#if filtered.length === 0}
    <div class="text-xs text-base-content/50 py-8 text-center">No memories yet.</div>
  {:else}
    <div class="flex flex-col gap-1.5">
      {#each filtered as mem}
        <div class="group flex items-center gap-3 py-2.5 px-3.5 rounded-lg border border-base-content/5 bg-base-100">
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
          <button
            type="button"
            onclick={() => removeMemory(mem.id)}
            disabled={deletingId === mem.id}
            class="opacity-0 group-hover:opacity-100 transition-opacity p-1 rounded hover:bg-error/10 text-base-content/40 hover:text-error cursor-pointer shrink-0 disabled:opacity-50"
            aria-label="Delete memory"
            title="Delete this memory"
          >
            <Trash2 class="w-3.5 h-3.5" />
          </button>
        </div>
      {/each}
    </div>
  {/if}
{/if}

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
      <p class="text-sm text-base-content/70 mb-4">This agent remembers across conversations in three layers — and only its own:</p>
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
