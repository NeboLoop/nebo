<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { collections, createCollection } from '$lib/stores/collections.js';
  import { listStoreCollections } from '$lib/api/index';

  interface PrivateOrg { id: string; name: string; initial: string; itemCount: number }
  const PRIVATE_ORGS: PrivateOrg[] = [];
  const MARKETPLACE_PRIVATE_ITEMS: Record<string, unknown>[] = [];

  type MarketCollection = { id: string; name: string; desc: string; itemCount: number };
  let marketCollections = $state<MarketCollection[]>([]);

  onMount(async () => {
    try {
      const res = await listStoreCollections() as { collections?: Record<string, unknown>[] } | null;
      if (Array.isArray(res?.collections)) {
        marketCollections = res.collections.map((c: Record<string, unknown>) => ({
          id: String(c.id ?? ''),
          name: String(c.name ?? ''),
          desc: String(c.description ?? c.desc ?? ''),
          itemCount: Number(c.itemCount ?? (Array.isArray(c.items) ? c.items.length : 0)),
        }));
      }
    } catch {}
  });
  import Package from 'lucide-svelte/icons/package';
  import Plus from 'lucide-svelte/icons/plus';
  import X from 'lucide-svelte/icons/x';
  import Check from 'lucide-svelte/icons/check';
  import Globe from 'lucide-svelte/icons/globe';
  import Lock from 'lucide-svelte/icons/lock';

  // Your own collections (orgId = 'personal')
  const myCollections = $derived($collections.filter(c => c.orgId === 'personal'));

  // --- Create collection modal ---
  let showCreate = $state(false);
  let newName = $state('');
  let newDesc = $state('');
  let newVisibility = $state<'private' | 'public'>('private');
  let creating = $state(false);
  let created = $state(false);

  const canCreate = $derived(newName.trim().length > 0 && !creating);

  function handleCreate() {
    if (!canCreate) return;
    creating = true;

    setTimeout(() => {
      const colId = createCollection({
        name: newName.trim(),
        desc: newDesc.trim(),
        orgId: 'personal',
        items: [],
        itemCount: 0,
        curator: 'You',
        visibility: newVisibility,
      });
      creating = false;
      created = true;

      setTimeout(() => {
        showCreate = false;
        created = false;
        newName = '';
        newDesc = '';
        newVisibility = 'private';
        goto(`/marketplace/collections/${colId}`);
      }, 800);
    }, 600);
  }

  function openCreate() {
    showCreate = true;
    created = false;
    creating = false;
    newName = '';
    newDesc = '';
    newVisibility = 'private';
  }

  function closeCreate() {
    if (creating) return;
    showCreate = false;
  }
</script>

<svelte:head><title>Collections - Marketplace - Nebo</title></svelte:head>

<!-- Create collection modal -->
{#if showCreate}
  <div class="fixed inset-0 z-[80] flex items-center justify-center p-4" role="dialog" aria-modal="true">
    <div class="absolute inset-0 bg-black/60 backdrop-blur-sm" role="presentation"></div>
    <div class="relative w-full max-w-md rounded-2xl bg-base-100 border border-base-content/10 shadow-2xl flex flex-col z-10">
      <!-- Header -->
      <div class="flex items-center justify-between px-5 py-4 border-b border-base-content/10 shrink-0">
        <div class="flex items-center gap-2.5">
          <div class="w-8 h-8 rounded-lg bg-accent/10 grid place-items-center">
            <Package class="w-4 h-4 text-accent" />
          </div>
          <div>
            <div class="text-sm font-semibold">New Collection</div>
            <div class="text-xs text-base-content/50">Create a curated bundle</div>
          </div>
        </div>
        <button class="w-7 h-7 rounded-md grid place-items-center hover:bg-base-200 cursor-pointer bg-transparent border-none" onclick={closeCreate} disabled={creating}>
          <X class="w-4 h-4" />
        </button>
      </div>

      <!-- Body -->
      <div class="px-5 py-4 flex flex-col gap-4">
        <label class="block">
          <span class="block text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Name</span>
          <input
            type="text"
            bind:value={newName}
            placeholder="e.g. Sales Enablement Kit"
            class="w-full py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30"
          />
        </label>

        <label class="block">
          <span class="block text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Description</span>
          <textarea
            bind:value={newDesc}
            rows="2"
            placeholder="What is this collection for?"
            class="w-full py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30 resize-y"
          ></textarea>
        </label>

        <div>
          <span class="block text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Visibility</span>
          <div class="flex gap-2">
            <button
              class="flex-1 flex items-center gap-2 py-2.5 px-3 rounded-lg border cursor-pointer transition-all {newVisibility === 'private' ? 'border-primary bg-primary/5' : 'border-base-content/10 bg-transparent hover:bg-base-200/50'}"
              onclick={() => newVisibility = 'private'}
            >
              <Lock class="w-3.5 h-3.5 {newVisibility === 'private' ? 'text-primary' : 'text-base-content/40'}" />
              <div class="text-left">
                <div class="text-sm font-medium">Private</div>
                <div class="text-xs text-base-content/50">Only you can see this</div>
              </div>
            </button>
            <button
              class="flex-1 flex items-center gap-2 py-2.5 px-3 rounded-lg border cursor-pointer transition-all {newVisibility === 'public' ? 'border-primary bg-primary/5' : 'border-base-content/10 bg-transparent hover:bg-base-200/50'}"
              onclick={() => newVisibility = 'public'}
            >
              <Globe class="w-3.5 h-3.5 {newVisibility === 'public' ? 'text-primary' : 'text-base-content/40'}" />
              <div class="text-left">
                <div class="text-sm font-medium">Public</div>
                <div class="text-xs text-base-content/50">Anyone can discover this</div>
              </div>
            </button>
          </div>
        </div>
      </div>

      <!-- Footer -->
      <div class="flex items-center justify-between px-5 py-4 border-t border-base-content/10 shrink-0">
        <div class="text-xs text-base-content/40">You can add items after creating</div>
        <div class="flex items-center gap-2">
          {#if !created}
            <button
              class="px-4 py-2 rounded-lg border border-base-content/10 text-sm font-medium cursor-pointer hover:bg-base-200 transition-colors bg-transparent"
              onclick={closeCreate}
              disabled={creating}
            >Cancel</button>
          {/if}
          <button
            class="px-4 py-2 rounded-lg text-sm font-medium cursor-pointer transition-all border-none disabled:opacity-50 {created ? 'bg-success text-success-content' : 'bg-primary text-primary-content hover:brightness-110'}"
            disabled={!canCreate && !created}
            onclick={handleCreate}
          >
            {#if created}
              <span class="flex items-center gap-1.5"><Check class="w-3.5 h-3.5" /> Created</span>
            {:else if creating}
              <span class="flex items-center gap-1.5"><span class="loading loading-spinner loading-xs"></span> Creating...</span>
            {:else}
              Create Collection
            {/if}
          </button>
        </div>
      </div>
    </div>
  </div>
{/if}

<div class="p-6 max-w-[960px]">
  <div class="flex items-center justify-between mb-6">
    <div>
      <div class="text-base font-semibold mb-1">Collections</div>
      <div class="text-xs text-base-content/50">Curated bundles of skills, agents, and plugins.</div>
    </div>
    <button
      class="flex items-center gap-1.5 py-1.5 px-3 rounded-lg bg-primary text-primary-content text-sm font-medium cursor-pointer hover:brightness-110 transition-all border-none"
      onclick={openCreate}
    >
      <Plus class="w-3.5 h-3.5" />
      New Collection
    </button>
  </div>

  <!-- Your collections -->
  {#if myCollections.length > 0}
    <div class="mb-6">
      <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">Your Collections</div>
      <div class="grid grid-cols-[repeat(auto-fill,minmax(260px,1fr))] gap-2.5">
        {#each myCollections as col}
          <a href="/marketplace/collections/{col.id}" class="p-3.5 rounded-lg border border-base-300 bg-base-100 cursor-pointer hover:border-base-content/20 hover:shadow-sm transition-all block">
            <div class="flex items-center gap-2 mb-1.5">
              <Package class="w-4 h-4 text-accent shrink-0" />
              <span class="text-sm font-semibold truncate flex-1">{col.name}</span>
              {#if col.visibility === 'public'}
                <Globe class="w-3 h-3 text-base-content/40 shrink-0" />
              {:else}
                <Lock class="w-3 h-3 text-base-content/40 shrink-0" />
              {/if}
            </div>
            <div class="text-xs text-base-content/70 leading-snug mb-2">{col.desc}</div>
            <div class="text-xs text-base-content/50 font-mono">{col.itemCount} items · {col.updated}</div>
          </a>
        {/each}
      </div>
    </div>
  {/if}

  <!-- Marketplace collections -->
  {#if marketCollections.length > 0}
    <div class="mb-6">
      <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">Curated Collections</div>
      <div class="grid grid-cols-[repeat(auto-fill,minmax(260px,1fr))] gap-2.5">
        {#each marketCollections as col}
          <a href="/marketplace/collections/{col.id}" class="p-3.5 rounded-lg border border-base-300 bg-base-100 cursor-pointer hover:border-base-content/20 hover:shadow-sm transition-all block">
            <div class="flex items-center gap-2 mb-1.5">
              <Package class="w-4 h-4 text-accent shrink-0" />
              <span class="text-sm font-semibold truncate flex-1">{col.name}</span>
            </div>
            <div class="text-xs text-base-content/70 leading-snug mb-2">{col.desc}</div>
            <div class="text-xs text-base-content/50 font-mono">{col.itemCount} items</div>
          </a>
        {/each}
      </div>
    </div>
  {/if}

  <!-- Shared with you -->
  {#if PRIVATE_ORGS.length === 0 && myCollections.length === 0 && marketCollections.length === 0}
    <div class="text-center py-12">
      <Package class="w-8 h-8 text-base-content/30 mx-auto mb-3" />
      <div class="text-sm font-medium mb-1">No collections yet</div>
      <div class="text-xs text-base-content/50 max-w-[280px] mx-auto">Create your own collection or wait for an organization to share theirs with you.</div>
    </div>
  {:else if PRIVATE_ORGS.length > 0}
    <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">Shared with you</div>
    <div class="flex flex-col gap-6">
      {#each PRIVATE_ORGS as org}
        {@const orgItems = MARKETPLACE_PRIVATE_ITEMS.filter(i => i.orgId === org.id)}
        {@const orgCollections = $collections.filter(c => c.orgId === org.id)}
        <div>
          <a href="/marketplace/collections/{org.id}" class="flex items-center gap-2.5 mb-3 group">
            <div class="w-8 h-8 rounded-lg bg-base-300 grid place-items-center text-sm font-semibold shrink-0">{org.initial}</div>
            <div class="flex-1 min-w-0">
              <div class="text-sm font-semibold group-hover:text-primary transition-colors">{org.name}</div>
              <div class="text-xs text-base-content/50">{orgItems.length} items · {orgCollections.length} collections</div>
            </div>
            <span class="text-sm text-primary opacity-0 group-hover:opacity-100 transition-opacity">View all &rarr;</span>
          </a>

          {#if orgCollections.length > 0}
            <div class="grid grid-cols-[repeat(auto-fill,minmax(260px,1fr))] gap-2.5">
              {#each orgCollections as col}
                <a href="/marketplace/collections/{col.id}" class="p-3.5 rounded-lg border border-base-300 bg-base-100 cursor-pointer hover:border-base-content/20 hover:shadow-sm transition-all block">
                  <div class="flex items-center gap-2 mb-1.5">
                    <Package class="w-4 h-4 text-accent shrink-0" />
                    <span class="text-sm font-semibold truncate">{col.name}</span>
                  </div>
                  <div class="text-xs text-base-content/70 leading-snug mb-2">{col.desc}</div>
                  <div class="text-xs text-base-content/50 font-mono">{col.itemCount} items · {col.updated}</div>
                </a>
              {/each}
            </div>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</div>
