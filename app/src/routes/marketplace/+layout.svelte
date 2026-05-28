<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';
  import { listStoreProducts } from '$lib/api/index';
  import { collections } from '$lib/stores/collections.js';
  import { installItem } from '$lib/stores/marketplace.js';
  import { getWebSocketClient } from '$lib/websocket/client';
  import CodeInstallModal from '$lib/components/chat/CodeInstallModal.svelte';
  import UserMenu from '$lib/components/UserMenu.svelte';
  import { sidebarCollapsedFor } from '$lib/stores/sidebar.js';
  const sidebarCollapsed = sidebarCollapsedFor('marketplace');
  import Search from 'lucide-svelte/icons/search';
  import X from 'lucide-svelte/icons/x';
  import Lock from 'lucide-svelte/icons/lock';
  let { children } = $props();

  type MarketItem = { id: string; name: string; desc: string; category: string; rating: number; installs: number; featured: boolean; price: string; code: string; type: string; path: string; private: boolean; org?: Record<string, unknown> };
  let allProducts = $state<MarketItem[]>([]);
  let categories = $state<{ slug: string; name: string; emoji: string; count: number }[]>([]);

  onMount(async () => {
    try {
      const res = await listStoreProducts() as { apps?: Record<string, unknown>[] } | null;
      if (res?.apps?.length) {
        allProducts = res.apps.map((a: Record<string, unknown>) => {
          const t = String(a.type || a.category || 'skill');
          const typeMap: Record<string, string> = { agent: 'agents', skill: 'skills', plugin: 'plugins', connector: 'connectors' };
          return {
            id: String(a.id ?? ''), name: String(a.name ?? ''), desc: String(a.description ?? ''),
            category: String(a.category ?? ''), rating: Number(a.rating ?? 0),
            installs: Number(a.installCount ?? 0), featured: Boolean(a.featured ?? false),
            price: String(a.price ?? 'Get'), code: String(a.code ?? ''),
            type: t, path: `/marketplace/${typeMap[t] || 'skills'}/${a.id}`,
            private: false,
          };
        });
        // Derive categories from products
        const catMap = new Map<string, number>();
        for (const p of allProducts) {
          if (p.category) catMap.set(p.category, (catMap.get(p.category) || 0) + 1);
        }
        categories = [...catMap.entries()].map(([slug, count]) => ({
          slug, name: slug.charAt(0).toUpperCase() + slug.slice(1), emoji: '', count,
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

  const allSearchItems = $derived(allProducts);

  const searchResults = $derived.by(() => {
    const q = debouncedQuery.toLowerCase().trim();
    if (!q) return [];
    return allSearchItems
      .filter(item => item.name.toLowerCase().includes(q) || item.desc.toLowerCase().includes(q) || item.category.toLowerCase().includes(q))
      .slice(0, 8);
  });

  const showResults = $derived(searchFocused && debouncedQuery.trim().length > 0);

  const typeLabels: Record<string, string> = { skill: 'Skill', agent: 'Agent', plugin: 'Plugin', connector: 'Connector', private: 'Private' };

  function selectResult(path: string) {
    searchQuery = '';
    debouncedQuery = '';
    searchFocused = false;
    goto(path);
  }

  function clearSearch() {
    searchQuery = '';
    debouncedQuery = '';
  }

  const marketplaceTab = $derived.by(() => {
    const p = $page.url.pathname;
    if (p === '/marketplace') return 'featured';
    // Collections sub-routes: /marketplace/collections/acme → collections-acme
    const colMatch = p.match(/\/marketplace\/collections\/([^/]+)/);
    if (colMatch) return `collections-${colMatch[1]}`;
    if (p === '/marketplace/collections') return 'collections';
    const match = p.match(/\/marketplace\/([^/]+)/);
    return match ? match[1] : 'featured';
  });

  const navItems = [
    { id: 'featured', path: '/marketplace', label: 'Featured' },
    { id: 'agents', path: '/marketplace/agents', label: 'Agents' },
    { id: 'skills', path: '/marketplace/skills', label: 'Skills' },
    { id: 'plugins', path: '/marketplace/plugins', label: 'Plugins' },
    { id: 'connectors', path: '/marketplace/connectors', label: 'Connectors' },
    { id: 'collections', path: '/marketplace/collections', label: 'Collections' },
    { id: 'installed', path: '/marketplace/installed', label: 'Installed' },
  ];

  const topCategories = $derived(categories.slice(0, 8));

  // Install code input
  let codeInput = $state('');
  let codeStatus = $state('idle'); // idle | processing | error
  let codeMessage = $state('');
  let showInstallModal = $state(false);
  const CODE_RE = /^(NEBO|SKIL|AGNT|PLUG|LOOP|WORK|APPX)-[0-9A-Z]{4}-[0-9A-Z]{4}$/i;
  const CODE_TYPE_MAP: Record<string, string> = {
    NEBO: 'nebo', SKIL: 'skill', WORK: 'workflow', AGNT: 'agent',
    LOOP: 'loop', PLUG: 'plugin', APPX: 'app',
  };
  const CODE_STATUS_MAP: Record<string, string> = {
    nebo: 'Connecting to NeboAI...', skill: 'Installing skill...',
    workflow: 'Installing workflow...', agent: 'Installing agent...',
    loop: 'Joining loop...', plugin: 'Installing plugin...', app: 'Installing app...',
  };

  function redeemCode() {
    const code = codeInput.trim().toUpperCase();
    if (!CODE_RE.test(code)) {
      codeStatus = 'error';
      codeMessage = 'Invalid code format';
      setTimeout(() => { codeStatus = 'idle'; codeMessage = ''; }, 2500);
      return;
    }

    // Dispatch code_processing for instant modal feedback
    const match = code.match(CODE_RE);
    if (match) {
      const prefix = match[1].toUpperCase();
      const codeTypeStr = CODE_TYPE_MAP[prefix] || 'code';
      window.dispatchEvent(new CustomEvent('nebo:code_processing', {
        detail: {
          code,
          code_type: codeTypeStr,
          status_message: CODE_STATUS_MAP[codeTypeStr] || 'Processing...',
        },
      }));
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
          {@const icons: Record<string, string> = { featured: '◆', agents: '◉', skills: '⚡', plugins: '🔌', connectors: '⬡', collections: '▤', installed: '✓' }}
          <a
            href={item.path}
            class="w-8 h-8 rounded-md flex items-center justify-center text-sm transition-colors {marketplaceTab === item.id ? 'bg-base-100 shadow-[0_0_0_1px_var(--color-base-300)]' : 'hover:bg-base-200'}"
            title={item.label}
          >{icons[item.id]}</a>
        {/each}
      </div>
      <div class="flex flex-col items-center gap-1 border-t border-base-300 py-2">
        {#each topCategories.slice(0, 6) as cat}
          <a href="/marketplace/categories" class="w-8 h-8 rounded-md flex items-center justify-center text-xs font-bold text-base-content/50 hover:bg-base-200 transition-colors" title={cat.name}>
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
      <div class="p-1.5">
        {#each navItems as item}
          <a
            href={item.path}
            class="flex items-center gap-1.5 py-1 px-2.5 mx-0 rounded-md text-sm transition-colors {marketplaceTab === item.id || (item.id === 'collections' && marketplaceTab.startsWith('collections-'))
              ? 'bg-base-100 shadow-[0_0_0_1px_var(--color-base-300)] font-medium'
              : 'hover:bg-base-200'}"
          >
            {item.label}
          </a>
        {/each}
      </div>

      <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 px-3.5 pt-3 pb-1">Categories</div>
      <div class="px-1.5">
        {#each topCategories as cat}
          <a href="/marketplace/categories" class="flex items-center gap-1.5 py-1 px-2.5 rounded-md text-sm hover:bg-base-200 transition-colors">
            <span class="flex-1">{cat.name}</span>
          </a>
        {/each}
        <a href="/marketplace/categories" class="block py-1.5 px-2.5 text-sm text-primary font-medium">
          All categories &rarr;
        </a>
      </div>
    </div>
  {/if}
  <UserMenu collapsed={$sidebarCollapsed} />
</div>

<!-- Main content area -->
<div class="flex-1 flex flex-col bg-base-100 min-w-0">
  <div class="h-11 px-[18px] border-b border-base-content/10 flex items-center gap-2 shrink-0">
    <span class="text-sm font-semibold">{navItems.find(i => i.id === marketplaceTab)?.label ?? (marketplaceTab.startsWith('collections') ? 'Collections' : 'Marketplace')}</span>
    <div class="relative ml-auto">
      <div class="flex items-center h-[26px] w-[220px] rounded-[5px] px-[9px] gap-1.5 text-sm border border-base-300 bg-base-100">
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
          <button class="p-0 bg-transparent border-none cursor-pointer shrink-0" onclick={clearSearch}>
            <X class="w-3 h-3 text-base-content/50" />
          </button>
        {/if}
      </div>
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

<CodeInstallModal bind:show={showInstallModal} />
