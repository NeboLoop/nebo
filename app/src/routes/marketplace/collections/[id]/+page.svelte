<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  interface PrivateOrg { id: string; name: string; initial: string; itemCount: number }
  const PRIVATE_ORGS: PrivateOrg[] = []; // TODO: load from API when collections endpoint exists
  const MARKETPLACE_PRIVATE_ITEMS: Record<string, unknown>[] = [];
  const MARKETPLACE_SKILLS: Record<string, unknown>[] = [];
  const MARKETPLACE_AGENTS_LIST: Record<string, unknown>[] = [];
  const MARKETPLACE_PLUGINS: Record<string, unknown>[] = [];
  const MARKETPLACE_CONNECTORS: Record<string, unknown>[] = [];
  import { installedIds, installItem } from '$lib/stores/marketplace.js';
  import { collections, deleteCollection, addItemToCollection, removeItemFromCollection } from '$lib/stores/collections.js';
  import Package from 'lucide-svelte/icons/package';
  import ArrowLeft from 'lucide-svelte/icons/arrow-left';
  import Check from 'lucide-svelte/icons/check';
  import Trash2 from 'lucide-svelte/icons/trash-2';
  import Plus from 'lucide-svelte/icons/plus';
  import X from 'lucide-svelte/icons/x';
  import Search from 'lucide-svelte/icons/search';
  import Star from 'lucide-svelte/icons/star';
  import Globe from 'lucide-svelte/icons/globe';
  import Lock from 'lucide-svelte/icons/lock';

  const id = $derived($page.params.id);

  const org = $derived(PRIVATE_ORGS.find(o => o.id === id));
  const collection = $derived($collections.find(c => c.id === id));

  const orgItems = $derived(org ? MARKETPLACE_PRIVATE_ITEMS.filter(i => i.orgId === org.id) : []);
  const orgCollections = $derived(org ? $collections.filter(c => c.orgId === org.id) : []);

  const iconColors = [
    'bg-primary/15 text-primary', 'bg-accent/15 text-accent', 'bg-success/15 text-success',
    'bg-warning/15 text-warning', 'bg-error/15 text-error', 'bg-info/15 text-info', 'bg-secondary/15 text-secondary',
  ];
  function getIconColor(id: string) {
    let hash = 0;
    for (let i = 0; i < id.length; i++) hash = id.charCodeAt(i) + ((hash << 5) - hash);
    return iconColors[Math.abs(hash) % iconColors.length];
  }
  function getInitials(name: string) {
    return name.split(' ').map(w => w[0]).join('').slice(0, 2).toUpperCase();
  }

  // All items for resolution
  const allItemPool = $derived([
    ...MARKETPLACE_PRIVATE_ITEMS,
    ...MARKETPLACE_SKILLS.map(s => ({ ...s, type: 'skill' as const, orgId: '' })),
    ...MARKETPLACE_AGENTS_LIST.map(a => ({ ...a, type: 'agent' as const, orgId: '' })),
    ...MARKETPLACE_PLUGINS.map(p => ({ ...p, type: 'plugin' as const, orgId: '' })),
    ...MARKETPLACE_CONNECTORS.map(c => ({ ...c, type: 'connector' as const, orgId: '' })),
  ]);

  const collectionItems = $derived(
    (collection?.items ?? [])
      .map(itemId => allItemPool.find(i => i.id === itemId))
      .filter(Boolean)
  );

  const collectionOrg = $derived(collection ? PRIVATE_ORGS.find(o => o.id === collection.orgId) : null);

  const typeLabels: Record<string, string> = { agent: 'Agent', skill: 'Skill', plugin: 'Plugin', connector: 'MCP' };

  const allCollectionInstalled = $derived(
    collectionItems.length > 0 && collectionItems.every(i => i && $installedIds.has(i.id))
  );

  const isOwn = $derived(collection?.orgId === 'personal');

  function installAll() {
    for (const item of collectionItems) {
      if (item && !$installedIds.has(item.id)) {
        installItem({ id: item.id, name: item.name, type: item.type ?? 'skill' });
      }
    }
  }

  function handleDelete() {
    if (!collection) return;
    deleteCollection(collection.id);
    goto('/marketplace/collections');
  }

  // --- Add items modal ---
  let showAddItems = $state(false);
  let addSearch = $state('');

  const addSearchResults = $derived.by(() => {
    const q = addSearch.toLowerCase().trim();
    if (!q) return [];
    const currentIds = new Set(collection?.items ?? []);
    return allItemPool
      .filter(i => !currentIds.has(i.id) && (i.name.toLowerCase().includes(q) || i.desc.toLowerCase().includes(q)))
      .slice(0, 8);
  });

  function handleAddItem(item: Record<string, unknown>) {
    if (!collection) return;
    addItemToCollection(collection.id, item.id);
  }

  function handleRemoveItem(itemId: string) {
    if (!collection) return;
    removeItemFromCollection(collection.id, itemId);
  }

  function getItemPath(item: Record<string, unknown>) {
    const type = item.type;
    if (type === 'agent') return `/marketplace/agents/${item.id}`;
    if (type === 'skill') return `/marketplace/skills/${item.id}`;
    if (type === 'plugin') return `/marketplace/plugins/${item.id}`;
    if (type === 'connector') return `/marketplace/connectors/${item.id}`;
    return '#';
  }
</script>

<svelte:head><title>{org?.name ?? collection?.name ?? 'Collections'} - Marketplace - Nebo</title></svelte:head>

<!-- Add items modal -->
{#if showAddItems && collection}
  <div class="fixed inset-0 z-[80] flex items-center justify-center p-4" role="dialog" aria-modal="true">
    <div class="absolute inset-0 bg-black/60 backdrop-blur-sm" role="presentation"></div>
    <div class="relative w-full max-w-md rounded-2xl bg-base-100 border border-base-content/10 shadow-2xl flex flex-col z-10 max-h-[70vh]">
      <div class="flex items-center justify-between px-5 py-4 border-b border-base-content/10 shrink-0">
        <div>
          <div class="text-sm font-semibold">Add items</div>
          <div class="text-xs text-base-content/50">Search for skills, agents, plugins, or connectors</div>
        </div>
        <button class="w-7 h-7 rounded-md grid place-items-center hover:bg-base-200 cursor-pointer bg-transparent border-none" onclick={() => showAddItems = false}>
          <X class="w-4 h-4" />
        </button>
      </div>

      <div class="px-5 py-3 border-b border-base-content/10 shrink-0">
        <div class="flex items-center h-9 rounded-lg px-3 gap-1.5 border border-base-content/10 bg-base-100">
          <Search class="w-3.5 h-3.5 text-base-content/50 shrink-0" />
          <input
            type="text"
            bind:value={addSearch}
            placeholder="Search marketplace..."
            class="flex-1 bg-transparent border-none outline-none text-sm placeholder:text-base-content/50 min-w-0"
          />
          {#if addSearch}
            <button class="p-0 bg-transparent border-none cursor-pointer shrink-0" onclick={() => addSearch = ''}>
              <X class="w-3 h-3 text-base-content/50" />
            </button>
          {/if}
        </div>
      </div>

      <div class="flex-1 overflow-y-auto">
        {#if addSearch.trim() && addSearchResults.length > 0}
          {#each addSearchResults as item}
            <div class="flex items-center gap-3 px-5 py-2.5 border-b border-base-content/5 last:border-b-0">
              <div class="w-8 h-8 rounded-lg {getIconColor(item.id)} grid place-items-center text-xs font-bold shrink-0">{getInitials(item.name)}</div>
              <div class="flex-1 min-w-0">
                <div class="text-sm font-medium truncate">{item.name}</div>
                <div class="text-xs text-base-content/50 truncate">{item.desc}</div>
              </div>
              <span class="text-xs text-base-content/40 shrink-0">{typeLabels[item.type] ?? item.type}</span>
              <button
                class="w-7 h-7 rounded-md grid place-items-center bg-primary text-primary-content cursor-pointer border-none hover:brightness-110 transition-all shrink-0"
                onclick={() => handleAddItem(item)}
              >
                <Plus class="w-3.5 h-3.5" />
              </button>
            </div>
          {/each}
        {:else if addSearch.trim()}
          <div class="py-8 text-center text-xs text-base-content/50">No results for "{addSearch}"</div>
        {:else}
          <div class="py-8 text-center text-xs text-base-content/50">Type to search the marketplace</div>
        {/if}
      </div>
    </div>
  </div>
{/if}

<div class="p-6 max-w-[960px]">
  {#if org}
    <!-- ORG VIEW -->
    <a href="/marketplace/collections" class="inline-flex items-center gap-1.5 text-xs text-base-content/50 hover:text-base-content transition-colors mb-5">
      <ArrowLeft class="w-3 h-3" /> Collections
    </a>

    <div class="flex items-center gap-3 mb-6">
      <div class="w-10 h-10 rounded-xl bg-base-300 grid place-items-center text-base font-semibold shrink-0">{org.initial}</div>
      <div class="flex-1">
        <h1 class="text-base font-semibold">{org.name}</h1>
        <div class="text-xs text-base-content/50">{orgItems.length} items · {orgCollections.length} collections</div>
      </div>
    </div>

    {#if orgCollections.length > 0}
      <div class="mb-6">
        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">Collections</div>
        <div class="grid grid-cols-[repeat(auto-fill,minmax(260px,1fr))] gap-2.5">
          {#each orgCollections as col}
            <a href="/marketplace/collections/{col.id}" class="p-3.5 rounded-lg border border-base-300 bg-base-100 cursor-pointer hover:border-base-content/20 hover:shadow-sm transition-all block">
              <div class="flex items-center gap-2 mb-1.5">
                <Package class="w-4 h-4 text-accent shrink-0" />
                <span class="text-sm font-semibold truncate">{col.name}</span>
              </div>
              <div class="text-xs text-base-content/70 leading-snug mb-2">{col.desc}</div>
              <div class="text-xs text-base-content/50 font-mono">{col.itemCount} items · Curated by {col.curator}</div>
            </a>
          {/each}
        </div>
      </div>
    {/if}

    <div class="mb-6">
      <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">All Items</div>
      <div class="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-3">
        {#each orgItems as item}
          <div class="p-4 rounded-xl border border-base-300 bg-base-100 hover:border-base-content/20 hover:shadow-sm transition-all">
            <div class="w-10 h-10 rounded-xl {getIconColor(item.id)} grid place-items-center text-sm font-bold mb-3">{getInitials(item.name)}</div>
            <div class="text-sm font-semibold mb-0.5">{item.name}</div>
            <div class="text-xs text-base-content/60 leading-snug mb-2.5 line-clamp-2">{item.desc}</div>
            <div class="flex items-center justify-between">
              <div class="flex items-center gap-1">
                <Star class="w-3 h-3 text-warning fill-warning" />
                <span class="text-xs font-medium">{item.rating}</span>
                <span class="text-xs text-base-content/40 ml-1">{item.installs}</span>
              </div>
              {#if $installedIds.has(item.id)}
                <span class="text-xs font-medium text-success">Installed</span>
              {:else}
                <span class="text-xs font-medium text-primary">Get</span>
              {/if}
            </div>
          </div>
        {/each}
      </div>
    </div>

  {:else if collection}
    <!-- COLLECTION VIEW -->
    {@const parentOrg = collectionOrg}
    <a href={parentOrg ? `/marketplace/collections/${parentOrg.id}` : '/marketplace/collections'} class="inline-flex items-center gap-1.5 text-xs text-base-content/50 hover:text-base-content transition-colors mb-5">
      <ArrowLeft class="w-3 h-3" /> {parentOrg?.name ?? 'Collections'}
    </a>

    <div class="flex items-start gap-4 mb-6">
      <div class="w-12 h-12 rounded-xl bg-accent/10 grid place-items-center shrink-0">
        <Package class="w-6 h-6 text-accent" />
      </div>
      <div class="flex-1 min-w-0">
        <div class="flex items-center gap-2 mb-1">
          <h1 class="text-base font-semibold">{collection.name}</h1>
          {#if collection.visibility === 'public'}
            <span class="inline-flex items-center gap-1 py-0.5 px-1.5 rounded bg-base-200 text-xs text-base-content/50"><Globe class="w-3 h-3" /> Public</span>
          {:else}
            <span class="inline-flex items-center gap-1 py-0.5 px-1.5 rounded bg-base-200 text-xs text-base-content/50"><Lock class="w-3 h-3" /> Private</span>
          {/if}
          {#if parentOrg}
            <span class="py-0.5 px-1.5 rounded bg-base-200 text-xs font-mono text-base-content/70">{parentOrg.name}</span>
          {/if}
        </div>
        <div class="text-xs text-base-content/70 mb-2">{collection.desc}</div>
        <div class="text-xs text-base-content/50">Curated by {collection.curator} · Updated {collection.updated} · {collectionItems.length} items</div>
      </div>
      <div class="flex items-center gap-2 shrink-0">
        {#if isOwn}
          <button
            class="flex items-center gap-1.5 py-1.5 px-3 rounded-lg border border-base-content/10 text-sm font-medium cursor-pointer hover:bg-base-200 transition-colors bg-transparent"
            onclick={() => showAddItems = true}
          >
            <Plus class="w-3.5 h-3.5" /> Add items
          </button>
          <button
            class="w-8 h-8 rounded-md grid place-items-center hover:bg-error/10 hover:text-error cursor-pointer bg-transparent border border-base-content/10 transition-colors"
            onclick={handleDelete}
            title="Delete collection"
          >
            <Trash2 class="w-3.5 h-3.5" />
          </button>
        {/if}
        {#if collectionItems.length > 0}
          <button
            class="py-1.5 px-4 rounded-lg text-sm font-medium cursor-pointer border-none transition-all {allCollectionInstalled ? 'bg-base-300 text-base-content/70' : 'bg-primary text-primary-content hover:brightness-110'}"
            disabled={allCollectionInstalled}
            onclick={installAll}
          >
            {#if allCollectionInstalled}
              <span class="flex items-center gap-1.5"><Check class="w-3.5 h-3.5" /> All installed</span>
            {:else}
              Install all
            {/if}
          </button>
        {/if}
      </div>
    </div>

    {#if collectionItems.length === 0}
      <div class="text-center py-12 border border-dashed border-base-300 rounded-xl">
        <Package class="w-8 h-8 text-base-content/30 mx-auto mb-3" />
        <div class="text-sm font-medium mb-1">No items yet</div>
        <div class="text-xs text-base-content/50 mb-3">Search and add skills, agents, or plugins to this collection.</div>
        {#if isOwn}
          <button
            class="inline-flex items-center gap-1.5 py-1.5 px-3 rounded-lg bg-primary text-primary-content text-sm font-medium cursor-pointer hover:brightness-110 transition-all border-none"
            onclick={() => showAddItems = true}
          >
            <Plus class="w-3.5 h-3.5" /> Add items
          </button>
        {/if}
      </div>
    {:else}
      <div class="flex flex-col gap-2">
        {#each collectionItems as item}
          {#if item}
            <div class="flex items-center gap-3 py-3 px-4 rounded-xl border border-base-300 bg-base-100">
              <a href={getItemPath(item)} class="w-9 h-9 rounded-lg {getIconColor(item.id)} grid place-items-center text-xs font-bold shrink-0">{getInitials(item.name)}</a>
              <div class="flex-1 min-w-0">
                <a href={getItemPath(item)} class="text-sm font-medium hover:text-primary transition-colors">{item.name}</a>
                <div class="text-xs text-base-content/50 truncate">{item.desc}</div>
              </div>
              <span class="py-0.5 px-1.5 rounded-full bg-base-200 text-xs text-base-content/60 shrink-0">{typeLabels[item.type] ?? item.type}</span>
              {#if $installedIds.has(item.id)}
                <span class="text-xs font-medium text-success shrink-0">Installed</span>
              {:else}
                <button
                  class="py-1 px-3 rounded-lg text-xs font-medium cursor-pointer border-none bg-primary text-primary-content hover:brightness-110 transition-all shrink-0"
                  onclick={() => item && installItem({ id: item.id, name: item.name, type: item.type ?? 'skill' })}
                >Install</button>
              {/if}
              {#if isOwn}
                <button
                  class="w-6 h-6 rounded grid place-items-center hover:bg-error/10 hover:text-error cursor-pointer bg-transparent border-none transition-colors shrink-0"
                  onclick={() => handleRemoveItem(item.id)}
                  title="Remove from collection"
                >
                  <X class="w-3 h-3" />
                </button>
              {/if}
            </div>
          {/if}
        {/each}
      </div>
    {/if}

  {:else}
    <div class="text-center py-12">
      <div class="text-sm font-medium mb-1">Not found</div>
      <a href="/marketplace/collections" class="text-sm text-primary">Back to Collections</a>
    </div>
  {/if}
</div>
