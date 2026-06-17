<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';
  import webapi from '$lib/api/gocliRequest';
  import { installItem } from '$lib/stores/marketplace.js';
  import { getWebSocketClient } from '$lib/websocket/client';
  import { dispatchInstallStart } from '$lib/marketplace/installCodes';
  import UserMenu from '$lib/components/UserMenu.svelte';
  import { sidebarCollapsedFor } from '$lib/stores/sidebar.js';
  import { slugify } from '$lib/data/categories';
  const sidebarCollapsed = sidebarCollapsedFor('marketplace');
  import Search from 'lucide-svelte/icons/search';
  import X from 'lucide-svelte/icons/x';
  import Lock from 'lucide-svelte/icons/lock';
  let { children } = $props();

  type MarketItem = { id: string; name: string; desc: string; category: string; rating: number; installs: number; featured: boolean; price: string; code: string; type: string; path: string; private: boolean; org?: Record<string, unknown> };
  let categories = $state<{ slug: string; name: string; emoji: string; count: number }[]>([]);

  // Sum every known per-type count field so a category's total reflects all
  // artifact kinds (skills, plugins, agents, collections, apps, connectors).
  function categoryTotal(c: Record<string, unknown>): number {
    const keys = ['skillCount', 'pluginCount', 'agentCount', 'collectionCount', 'appCount', 'connectorCount', 'workflowCount', 'toolCount'];
    return keys.reduce((sum, k) => sum + Number(c[k] ?? 0), 0);
  }

  onMount(async () => {
    // Categories for the sidebar — fetched directly with their server-side counts.
    try {
      const catRes = await webapi.get<any>('/api/v1/store/categories').catch(() => ({ categories: [] }));
      const cats = (catRes.categories || []) as Record<string, unknown>[];
      if (cats.length) {
        categories = cats.map((c) => ({
          slug: String(c.slug ?? ''),
          name: String(c.name ?? ''),
          emoji: String(c.emoji ?? ''),
          count: categoryTotal(c),
        }));
      }
    } catch {}
  });

  // Search
  let searchQuery = $state('');
  let searchFocused = $state(false);
  let debounceTimer: ReturnType<typeof setTimeout> | null = null;
  let debouncedQuery = $state('');

  $effect(() => {
    const q = searchQuery;
    if (debounceTimer) clearTimeout(debounceTimer);
    debounceTimer = setTimeout(() => { debouncedQuery = q; }, 150);
    return () => { if (debounceTimer) clearTimeout(debounceTimer); };
  });

  // Backend-powered search — scales past the first loaded page to the whole
  // catalog (NeboLoop Search). The dropdown shows the top 8; Enter / "See all"
  // opens the full results page.
  let searchResults = $state<MarketItem[]>([]);
  let searchLoading = $state(false);

  $effect(() => {
    const q = debouncedQuery.trim();
    if (!q) {
      searchResults = [];
      searchLoading = false;
      return;
    }
    searchLoading = true;
    webapi
      .get<any>('/api/v1/store/products', { q, pageSize: 8 })
      .then((res: any) => {
        if (debouncedQuery.trim() !== q) return;
        searchResults = ((res?.products as Record<string, unknown>[]) || []).map((a) => {
          const tp = String(a.type || a.category || 'skill');
          const typeMap: Record<string, string> = { agent: 'agents', skill: 'skills', plugin: 'plugins', connector: 'connectors', app: 'apps', collection: 'collections' };
          return {
            id: String(a.id ?? ''), name: String(a.name ?? ''), desc: String(a.description ?? ''),
            category: String(a.category ?? ''), rating: 0, installs: 0, featured: false,
            price: String(a.price ?? 'Get'), code: String(a.code ?? ''),
            type: tp, path: `/marketplace/${typeMap[tp] || 'skills'}/${a.id}`, private: false
          };
        });
      })
      .catch(() => { if (debouncedQuery.trim() === q) searchResults = []; })
      .finally(() => { if (debouncedQuery.trim() === q) searchLoading = false; });
  });

  const showResults = $derived(searchFocused && debouncedQuery.trim().length > 0);

  const typeLabels: Record<string, string> = { skill: 'Skill', agent: 'Agent', plugin: 'Plugin', connector: 'Connector', app: 'App', collection: 'Collection', private: 'Private' };

  function selectResult(path: string) {
    searchQuery = '';
    debouncedQuery = '';
    searchFocused = false;
    goto(path);
  }

  function submitSearch() {
    const q = searchQuery.trim();
    if (!q) return;
    searchFocused = false;
    goto(`/marketplace/search?q=${encodeURIComponent(q)}`);
  }

  function clearSearch() {
    searchQuery = '';
    debouncedQuery = '';
  }

  // On the unified /marketplace page the active tab is driven by the `kind`
  // param (all/agents/apps/...). Everywhere else it derives from the pathname.
  const activeKind = $derived($page.url.searchParams.get('kind') || 'all');
  const activePrice = $derived($page.url.searchParams.get('price') || 'all');
  const activeCategory = $derived($page.url.searchParams.get('category') || '');
  // Detail pages (/marketplace/<type>/<id>) shouldn't show the kind filter bar.
  const isDetail = $derived(/^\/marketplace\/(agents|apps|skills|plugins|connectors|collections)\/.+/.test($page.url.pathname));

  const marketplaceTab = $derived.by(() => {
    const p = $page.url.pathname;
    if (p === '/marketplace') return activeKind;
    const match = p.match(/\/marketplace\/([^/]+)/);
    return match ? match[1] : 'all';
  });

  const navItems = [
    { id: 'all', path: '/marketplace', label: 'All' },
    { id: 'agents', path: '/marketplace?kind=agents', label: 'Agents' },
    { id: 'apps', path: '/marketplace?kind=apps', label: 'Apps' },
    { id: 'skills', path: '/marketplace?kind=skills', label: 'Skills' },
    { id: 'plugins', path: '/marketplace?kind=plugins', label: 'Plugins' },
    { id: 'connectors', path: '/marketplace?kind=connectors', label: 'Connectors' },
    { id: 'collections', path: '/marketplace?kind=collections', label: 'Collections' },
    { id: 'shared', path: '/marketplace/shared', label: 'Shared' },
    { id: 'installed', path: '/marketplace/installed', label: 'Installed' },
  ];

  const priceOptions = [
    { value: 'all', label: 'All' },
    { value: 'free', label: 'Free' },
    { value: 'paid', label: 'Paid' },
  ];

  // Set the ?price= filter on the current /marketplace URL.
  function setPrice(value: string) {
    const url = new URL($page.url);
    url.pathname = '/marketplace';
    if (value === 'all') url.searchParams.delete('price');
    else url.searchParams.set('price', value);
    goto(url.pathname + url.search, { replaceState: true, noScroll: true });
  }

  const topCategories = $derived(categories.slice(0, 8));

  // Install code input
  let codeInput = $state('');
  let codeStatus = $state('idle'); // idle | processing | error
  let codeMessage = $state('');
  function redeemCode() {
    const code = codeInput.trim().toUpperCase();
    // dispatchInstallStart opens the modal instantly AND validates the format.
    if (!dispatchInstallStart(code)) {
      codeStatus = 'error';
      codeMessage = 'Invalid code format';
      setTimeout(() => { codeStatus = 'idle'; codeMessage = ''; }, 2500);
      return;
    }

    // Send via WebSocket — backend intercepts and handles
    const ws = getWebSocketClient();
    if (ws.isConnected()) {
      ws.send('chat', { prompt: code, agent_id: 'assistant' });
    }

    codeInput = '';
    codeStatus = 'idle';
  }
</script>

<svelte:head><title>Marketplace - Nebo</title></svelte:head>

<!-- Left panel: marketplace nav -->
<div class="{$sidebarCollapsed ? 'w-12 min-w-12' : 'w-[220px] min-w-[220px]'} border-r border-base-300 flex flex-col bg-base-200 shrink-0 transition-all duration-150">
  <div class="h-11 border-b border-base-300 flex items-center shrink-0 {$sidebarCollapsed ? 'justify-center' : 'px-3.5 justify-between'}">
    {#if !$sidebarCollapsed}
      <span class="text-sm font-semibold flex-1">Marketplace</span>
    {/if}
    <button class="w-7 h-7 rounded-md flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none shrink-0" onclick={() => $sidebarCollapsed = !$sidebarCollapsed} title={$sidebarCollapsed ? 'Expand sidebar' : 'Collapse sidebar'}>
      <svg width="16" height="16" viewBox="0 0 16 16" fill="none"><rect x="1.5" y="2.5" width="13" height="11" rx="1.5" stroke="currentColor" stroke-width="1.2"/><line x1="5.5" y1="3" x2="5.5" y2="13" stroke="currentColor" stroke-width="1.2"/></svg>
    </button>
  </div>

  {#if $sidebarCollapsed}
    <div class="flex-1 overflow-y-auto">
      <div class="flex flex-col items-center gap-1 py-2">
        {#each navItems as item}
          {@const icons: Record<string, string> = { all: '◆', agents: '◉', apps: '▦', skills: '⚡', plugins: '🔌', connectors: '⬡', collections: '▤', shared: '↗', installed: '✓' }}
          <a
            href={item.path}
            class="w-8 h-8 rounded-md flex items-center justify-center text-sm transition-colors {marketplaceTab === item.id ? 'bg-base-100 shadow-[0_0_0_1px_var(--color-base-300)]' : 'hover:bg-base-200'}"
            title={item.label}
          >{icons[item.id]}</a>
        {/each}
      </div>
      <div class="flex flex-col items-center gap-1 border-t border-base-300 py-2">
        {#each topCategories.slice(0, 6) as cat}
          <a href="/marketplace?category={slugify(cat.name)}" class="w-8 h-8 rounded-md flex items-center justify-center text-xs font-bold text-base-content/50 hover:bg-base-200 transition-colors" title={cat.name}>
            {cat.name[0]}
          </a>
        {/each}
      </div>
    </div>
  {:else}
    <!-- Install code -->
    <div class="py-2.5 px-3.5 border-b border-base-300">
      <div class="text-xs text-base-content/70 mb-1">Install code</div>
      <form class="flex gap-1" onsubmit={(e) => { e.preventDefault(); redeemCode(); }}>
        <input
          type="text"
          bind:value={codeInput}
          placeholder="NEBO-XXXX-XXXX"
          class="flex-1 min-w-0 py-1 px-2 rounded-[5px] border border-base-300 font-mono text-sm bg-base-100 outline-none uppercase {codeStatus === 'error' ? 'border-error' : codeStatus === 'success' ? 'border-success' : ''}"
          disabled={codeStatus === 'processing'}
        />
        <button
          type="submit"
          class="py-1 px-2 rounded-[5px] border border-base-300 bg-base-100 text-sm cursor-pointer font-medium disabled:opacity-50"
          disabled={codeStatus === 'processing' || !codeInput.trim()}
        >{codeStatus === 'processing' ? '...' : 'Go'}</button>
      </form>
      {#if codeMessage}
        <div class="text-sm mt-1 {codeStatus === 'error' ? 'text-error' : codeStatus === 'success' ? 'text-success' : 'text-base-content/60'}">
          {codeMessage}
        </div>
      {/if}
    </div>

    <div class="flex-1 overflow-y-auto">
      <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 px-3.5 pt-3 pb-1">Category</div>
      <div class="px-1.5">
        <a
          href="/marketplace"
          class="flex items-center gap-1.5 py-1 px-2.5 rounded-md text-sm transition-colors {!activeCategory
            ? 'bg-base-100 shadow-[0_0_0_1px_var(--color-base-300)] font-medium'
            : 'hover:bg-base-200'}"
        >
          <span class="flex-1 truncate">All categories</span>
        </a>
        {#each categories as cat}
          <a
            href="/marketplace?category={slugify(cat.name)}"
            class="flex items-center gap-1.5 py-1 px-2.5 rounded-md text-sm transition-colors {activeCategory === slugify(cat.name)
              ? 'bg-base-100 shadow-[0_0_0_1px_var(--color-base-300)] font-medium'
              : 'hover:bg-base-200'}"
          >
            <span class="flex-1 truncate">{cat.name}</span>
            {#if cat.count > 0}
              <span class="text-xs font-mono text-base-content/50 shrink-0">{cat.count}</span>
            {/if}
          </a>
        {/each}
      </div>

      <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 px-3.5 pt-3 pb-1">Pricing</div>
      <div class="p-1.5">
        {#each priceOptions as opt}
          <button
            type="button"
            class="w-full flex items-center gap-1.5 py-1 px-2.5 rounded-md text-sm text-left transition-colors border-none bg-transparent cursor-pointer {activePrice === opt.value
              ? 'bg-base-100 shadow-[0_0_0_1px_var(--color-base-300)] font-medium'
              : 'hover:bg-base-200'}"
            onclick={() => setPrice(opt.value)}
          >
            {opt.label}
          </button>
        {/each}
      </div>
    </div>
  {/if}
  <UserMenu collapsed={$sidebarCollapsed} />
</div>

<!-- Main content area -->
<div class="flex-1 flex flex-col bg-base-100 min-w-0">
  <div class="h-12 px-[18px] border-b border-base-content/10 flex items-center gap-2 shrink-0">
    {#if !isDetail}
      <div class="flex items-center gap-1 flex-1 min-w-0 overflow-x-auto">
        {#each navItems as item}
          <a
            href={item.path}
            class="shrink-0 px-3 py-1 rounded-lg text-sm transition-colors {marketplaceTab === item.id
              ? 'bg-base-content text-base-100 font-semibold'
              : 'text-base-content/70 font-medium hover:text-base-content hover:bg-base-200'}"
          >{item.label}</a>
        {/each}
      </div>
    {:else}
      <div class="flex-1"></div>
    {/if}
    <div class="relative ml-auto shrink-0">
      <form class="flex items-center h-[26px] w-[220px] rounded-[5px] px-[9px] gap-1.5 text-sm border border-base-300 bg-base-100" onsubmit={(e) => { e.preventDefault(); submitSearch(); }}>
        <Search class="w-3 h-3 text-base-content/50 shrink-0" />
        <input
          type="text"
          bind:value={searchQuery}
          onfocus={() => searchFocused = true}
          onblur={() => setTimeout(() => searchFocused = false, 200)}
          placeholder="Search marketplace..."
          class="flex-1 bg-transparent border-none outline-none text-sm placeholder:text-base-content/50 min-w-0"
        />
        {#if searchQuery}
          <button type="button" class="p-0 bg-transparent border-none cursor-pointer shrink-0" onclick={clearSearch}>
            <X class="w-3 h-3 text-base-content/50" />
          </button>
        {/if}
      </form>
      {#if showResults}
        <div class="absolute top-full right-0 mt-1 w-[340px] bg-base-100 border border-base-300 rounded-lg shadow-xl z-50 overflow-hidden">
          {#if searchResults.length > 0}
            {#each searchResults as item}
              <button
                class="w-full flex items-center gap-3 px-3.5 py-2.5 text-left cursor-pointer hover:bg-base-200 transition-colors bg-transparent border-none border-b border-b-base-300 last:border-b-0"
                onmousedown={() => selectResult(item.path)}
              >
                <div class="flex-1 min-w-0">
                  <div class="text-sm font-medium truncate flex items-center gap-1.5">
                    {#if item.private}
                      <Lock class="w-3 h-3 text-base-content/50 shrink-0" />
                    {/if}
                    {item.name}
                  </div>
                  <div class="text-xs text-base-content/70 truncate">{item.desc}</div>
                </div>
                {#if item.private && item.org}
                  <span class="px-1.5 py-0.5 rounded text-xs font-mono bg-accent/10 text-accent shrink-0">{item.org.name}</span>
                {:else}
                  <span class="px-1.5 py-0.5 rounded text-xs font-mono bg-base-200 text-base-content/70 shrink-0">{typeLabels[item.type]}</span>
                {/if}
              </button>
            {/each}
            <button
              class="w-full px-3.5 py-2.5 text-left text-sm font-medium text-primary cursor-pointer hover:bg-base-200 transition-colors bg-transparent border-none border-t border-t-base-300"
              onmousedown={submitSearch}
            >See all results for "{debouncedQuery}"</button>
          {:else if searchLoading}
            <div class="px-3.5 py-4 text-center text-xs text-base-content/50">Searching…</div>
          {:else}
            <div class="px-3.5 py-4 text-center text-xs text-base-content/50">No results for "{debouncedQuery}"</div>
          {/if}
        </div>
      {/if}
    </div>
  </div>
  <div class="flex-1 min-h-0 overflow-y-auto">
    {@render children()}
  </div>
</div>
