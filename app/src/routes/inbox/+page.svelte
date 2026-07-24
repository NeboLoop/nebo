<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { marked } from 'marked';
  import CheckCircle2 from 'lucide-svelte/icons/check-circle-2';
  import SearchIcon from 'lucide-svelte/icons/search';
  import X from 'lucide-svelte/icons/x';
  import Copy from 'lucide-svelte/icons/copy';
  import Check from 'lucide-svelte/icons/check';
  import Trash2 from 'lucide-svelte/icons/trash-2';
  import ArrowLeft from 'lucide-svelte/icons/arrow-left';
  import Mail from 'lucide-svelte/icons/mail';
  import ExternalLink from 'lucide-svelte/icons/external-link';
  import { page } from '$app/stores';
  import { goto } from '$lib/nav';
  import {
    notifications, unreadCount, hasMore, loadNotifications, loadMore, markAsRead, markAllRead, removeNotification, type Notification,
  } from '$lib/stores/notifications';

  let copied = $state(false);
  let filter = $state<'all' | 'agent' | 'system' | 'warning' | 'error'>('all');
  let employeeFilter = $state('all');
  let departmentFilter = $state('all');
  let search = $state('');
  let sentinel = $state<HTMLElement | null>(null);
  let loadingMore = false;
  // id → {name, department} for attribution rows and the Employee/Department filters
  let roster = $state<Record<string, { name: string; department?: string }>>({});

  onMount(async () => {
    loadNotifications();
    try {
      const { listAgents } = await import('$lib/api/nebo');
      const res = await listAgents(200);
      const map: Record<string, { name: string; department?: string }> = {};
      const agents = (res.agents || []) as Array<{ id: string; name: string; department?: string | null }>;
      for (const a of agents) map[a.id] = { name: a.name, department: a.department || undefined };
      roster = map;
    } catch { /* roster is progressive enhancement */ }
  });

  // Infinite scroll: when the sentinel row at the bottom of the list scrolls
  // into view, pull the next page.
  $effect(() => {
    if (!sentinel) return;
    const io = new IntersectionObserver(async (entries) => {
      if (entries.some(e => e.isIntersecting) && !loadingMore) {
        loadingMore = true;
        await loadMore();
        loadingMore = false;
      }
    });
    io.observe(sentinel);
    return () => io.disconnect();
  });

  const typeColors: Record<string, string> = {
    agent: 'bg-success', system: 'bg-info', warning: 'bg-warning', error: 'bg-error',
  };
  const filters = [
    { id: 'all', label: 'inbox.filterAll' },
    { id: 'agent', label: 'inbox.filterAgent' },
    { id: 'system', label: 'inbox.filterSystem' },
    { id: 'warning', label: 'inbox.filterWarning' },
    { id: 'error', label: 'inbox.filterError' },
  ] as const;

  // Employees that actually appear in the loaded notifications; departments come
  // from those employees' marketplace metadata.
  const employees = $derived(
    [...new Set($notifications.map(n => n.agentId).filter((id): id is string => !!id))]
      .map(id => ({ id, name: roster[id]?.name ?? id }))
      .sort((a, b) => a.name.localeCompare(b.name))
  );
  const departments = $derived(
    [...new Set(employees.map(e => roster[e.id]?.department).filter((d): d is string => !!d))].sort()
  );

  const sorted = $derived(
    [...$notifications]
      .filter(n => filter === 'all' || n.type === filter)
      .filter(n => employeeFilter === 'all' || n.agentId === employeeFilter)
      .filter(n => departmentFilter === 'all' || (n.agentId && roster[n.agentId]?.department === departmentFilter))
      .filter(n => {
        if (!search.trim()) return true;
        const q = search.toLowerCase();
        return n.title.toLowerCase().includes(q) || n.message.toLowerCase().includes(q);
      })
      .sort((a, b) => b.createdAt - a.createdAt)
  );

  const deptLabel = (slug: string) => slug.replace(/-/g, ' ').replace(/\b\w/g, c => c.toUpperCase());
  // Selection lives in the URL (?m=<id>) so mobile gets a real screen: the
  // OS back gesture / browser back returns to the list. Desktop replaces the
  // entry instead so flipping through messages doesn't pile up history.
  const selectedId = $derived($page.url.searchParams.get('m'));
  const selected = $derived(sorted.find(n => n.id === selectedId) ?? null);
  const isDesktop = () => window.matchMedia('(min-width: 768px)').matches;

  function open(n: Notification) {
    markAsRead(n.id);
    copied = false;
    goto(`/inbox?m=${encodeURIComponent(n.id)}`, { replaceState: isDesktop(), noScroll: true });
  }
  function closeReader() {
    if (isDesktop()) goto('/inbox', { replaceState: true, noScroll: true });
    else history.back();
  }
  function remove(id: string) {
    removeNotification(id);
    if (selectedId === id) goto('/inbox', { replaceState: true, noScroll: true });
  }
  async function copyBody(n: Notification) {
    await navigator.clipboard.writeText(n.message);
    copied = true;
    setTimeout(() => (copied = false), 1500);
  }
</script>

<svelte:head><title>{$t('inbox.title')}</title></svelte:head>

<div class="flex-1 flex min-h-0 min-w-0 bg-base-100">
  <!-- Message list (email-style rows). On mobile the list and reading pane swap full-screen. -->
  <div class="w-full min-w-0 md:w-80 lg:w-96 md:shrink-0 border-r border-base-300 bg-base-200/50 flex-col min-h-0 {selected ? 'hidden md:flex' : 'flex'}">
    <div class="flex items-center justify-between h-12 px-4 border-b border-base-content/10 shrink-0">
      <h1 class="text-base font-semibold">{$t('inbox.title')}</h1>
      {#if $unreadCount > 0}
        <button class="btn btn-ghost btn-xs text-base-content/70" onclick={markAllRead}>
          {$t('notifications.markAllRead')}
        </button>
      {/if}
    </div>
    <div class="px-4 py-2.5 border-b border-base-content/10 shrink-0 flex flex-col gap-2">
      <div class="flex items-center h-8 rounded-[5px] px-[9px] gap-1.5 text-sm border border-base-300 bg-base-100">
        <SearchIcon class="w-3 h-3 text-base-content/50 shrink-0" />
        <input
          type="text"
          bind:value={search}
          placeholder={$t('common.search')}
          class="flex-1 bg-transparent border-none outline-none text-sm placeholder:text-base-content/50 min-w-0"
        />
        {#if search}
          <button type="button" class="p-0 bg-transparent border-none cursor-pointer shrink-0" onclick={() => (search = '')} aria-label={$t('common.close')}>
            <X class="w-3 h-3 text-base-content/50" />
          </button>
        {/if}
      </div>
      <div class="flex items-center gap-1 overflow-x-auto">
        {#each filters as f (f.id)}
          <button
            class="btn btn-xs whitespace-nowrap {filter === f.id ? 'btn-neutral' : 'btn-ghost text-base-content/60'}"
            onclick={() => (filter = f.id)}
          >
            {$t(f.label)}
          </button>
        {/each}
      </div>
      {#if employees.length > 0}
        <div class="flex items-center gap-1.5">
          <select class="select select-xs flex-1 min-w-0 bg-base-100 border-base-300" bind:value={employeeFilter} aria-label={$t('inbox.byEmployee')}>
            <option value="all">{$t('inbox.byEmployee')}: {$t('inbox.filterAll')}</option>
            {#each employees as e (e.id)}
              <option value={e.id}>{e.name}</option>
            {/each}
          </select>
          {#if departments.length > 0}
            <select class="select select-xs flex-1 min-w-0 bg-base-100 border-base-300" bind:value={departmentFilter} aria-label={$t('inbox.byDepartment')}>
              <option value="all">{$t('inbox.byDepartment')}: {$t('inbox.filterAll')}</option>
              {#each departments as d (d)}
                <option value={d}>{deptLabel(d)}</option>
              {/each}
            </select>
          {/if}
        </div>
      {/if}
    </div>
    <div class="flex-1 overflow-y-auto">
      {#if sorted.length === 0}
        <div class="flex flex-col items-center justify-center text-center py-24 gap-2 px-4">
          <CheckCircle2 class="w-8 h-8 text-success/60" />
          <div class="text-sm font-medium">{$t('inbox.empty')}</div>
          <div class="text-xs text-base-content/50">{$t('inbox.emptyDesc')}</div>
        </div>
      {:else}
        {#each sorted as n (n.id)}
          <div
            class="group relative flex items-start gap-2.5 px-4 py-3 border-b border-base-content/10 cursor-pointer transition-colors {selectedId === n.id
              ? 'bg-base-100 border-l-2 border-l-primary pl-[14px]'
              : 'border-l-2 border-l-transparent hover:bg-base-200 pl-[14px]'}"
            onclick={() => open(n)}
            onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); open(n); } }}
            role="button"
            tabindex="0"
          >
            <div class="w-2 h-2 rounded-full mt-1.5 shrink-0 {n.read ? 'bg-transparent' : typeColors[n.type] || 'bg-info'}"></div>
            <div class="flex-1 min-w-0">
              <div class="flex items-baseline gap-2">
                {#if n.agentId && roster[n.agentId]}
                  <span class="text-xs shrink-0 {n.read ? 'text-base-content/50' : 'text-base-content/70 font-medium'}">{roster[n.agentId].name}</span>
                {/if}
                <span class="text-sm truncate {n.read ? 'font-normal text-base-content/70' : 'font-semibold text-base-content'}">{n.title}</span>
                <span class="text-xs text-base-content/50 font-mono shrink-0 ml-auto">{n.time}</span>
              </div>
              <p class="text-xs text-base-content/60 truncate mt-0.5">{n.message}</p>
            </div>
            <button
              onclick={(e) => { e.stopPropagation(); remove(n.id); }}
              class="absolute right-2 bottom-2 p-1 rounded hover:bg-base-content/10 transition-opacity cursor-pointer bg-base-200 border-none opacity-0 group-hover:opacity-100"
              aria-label={$t('notifications.closeNotification')}
            >
              <Trash2 class="w-3 h-3 text-base-content/40" />
            </button>
          </div>
        {/each}
        {#if $hasMore}
          <div bind:this={sentinel} class="flex justify-center py-3">
            <span class="loading loading-dots loading-xs text-base-content/40"></span>
          </div>
        {/if}
      {/if}
    </div>
  </div>

  <!-- Reading pane -->
  <div class="flex-1 min-w-0 flex-col min-h-0 {selected ? 'flex' : 'hidden md:flex'}">
    {#if selected}
      <div class="flex items-center gap-2 h-12 px-4 border-b border-base-content/10 shrink-0">
        <button class="md:hidden p-1.5 rounded hover:bg-base-200 cursor-pointer bg-transparent border-none" onclick={closeReader} aria-label={$t('common.close')}>
          <ArrowLeft class="w-4 h-4 text-base-content/70" />
        </button>
        <div class="w-2 h-2 rounded-full shrink-0 {typeColors[selected.type] || 'bg-info'}"></div>
        <span class="text-sm font-medium truncate min-w-0">{selected.title}</span>
        {#if selected.agentId && roster[selected.agentId]}
          <span class="badge badge-ghost badge-sm shrink-0">{roster[selected.agentId].name}{roster[selected.agentId].department ? ` · ${deptLabel(roster[selected.agentId].department!)}` : ''}</span>
        {/if}
        <span class="text-xs text-base-content/50 font-mono shrink-0">{selected.time}</span>
        <div class="ml-auto flex items-center gap-1 shrink-0">
          {#if selected.link}
            <button class="btn btn-ghost btn-xs gap-1.5" onclick={() => selected?.link && goto(selected.link)}>
              <ExternalLink class="w-3.5 h-3.5" />
              {$t('common.open')}
            </button>
          {/if}
          <button class="btn btn-ghost btn-xs gap-1.5" onclick={() => selected && copyBody(selected)}>
            {#if copied}<Check class="w-3.5 h-3.5 text-success" />{:else}<Copy class="w-3.5 h-3.5" />{/if}
            {$t('common.copy')}
          </button>
          <button class="btn btn-ghost btn-xs" onclick={() => selected && remove(selected.id)} aria-label={$t('common.delete')}>
            <Trash2 class="w-3.5 h-3.5 text-base-content/50" />
          </button>
        </div>
      </div>
      <div class="flex-1 overflow-y-auto">
        <div class="max-w-2xl mx-auto px-6 py-6">
          <div class="prose prose-sm max-w-none [&>:first-child]:mt-0">
            {@html marked.parse(selected.message, { async: false })}
          </div>
        </div>
      </div>
    {:else}
      <div class="flex-1 flex flex-col items-center justify-center text-center gap-2 text-base-content/40">
        <Mail class="w-8 h-8" />
        <div class="text-xs">{$t('inbox.emptyDesc')}</div>
      </div>
    {/if}
  </div>
</div>
