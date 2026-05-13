<script lang="ts">
  import { onMount } from 'svelte';
  import { AGENT_COLORS, AGENT_COLORS_MAP } from '$lib/tokens.js';
  import { AGENTS } from '$lib/data.js';
  import { getScheduleAgents, runsPerWeek as storeRunsPerWeek, userScheduleItems } from '$lib/stores/schedule.js';
  import MiniMonth from './MiniMonth.svelte';
  import type { Agent, Chat } from '$lib/api/neboComponents';

  let { activePage = 'home', activeChat = '', enabled = null, onToggleAgent = null, marketplaceTab = '' } = $props();
  let collapsed = $state(false);
  let searchText = $state('');
  let selectedDate = $state(new Date());

  let agents = $state<{ id: string; name: string; initial: string; color: string; role: string; status: string }[]>([]);
  let chats = $state<{ id: string; title: string; agent: string; agentColor: string; updatedAt: string; lastMessage?: string }[]>([]);
  let chatGroups = $state<{ label: string; chats: string[] }[]>([]);

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const [agentResp, chatResp] = await Promise.all([
        api.listAgents().catch(() => null),
        api.listChats().catch(() => null)
      ]);

      if (agentResp?.agents?.length) {
        agents = (agentResp.agents as Agent[]).filter(a => !a.isApp).map(a => ({
          id: a.id,
          name: a.name,
          role: a.description || '',
          initial: a.name.charAt(0).toUpperCase(),
          status: a.isEnabled ? 'online' : 'paused',
          color: 'teal',
        }));
      }

      if (chatResp?.chats?.length) {
        chats = chatResp.chats.map(c => ({
          id: c.id,
          title: c.title,
          agent: '',
          agentColor: 'teal',
          updatedAt: formatRelativeTime(new Date(c.updatedAt * 1000).toISOString()),
        }));
        // Group chats by recency
        try {
          const daysResp = await api.listChatDays();
          if (daysResp?.days?.length) {
            chatGroups = (daysResp.days as { day: string; messageCount?: number }[]).map(d => ({
              label: d.day,
              chats: chats.filter(c => c.id).map(c => c.id).slice(0, d.messageCount ?? 5),
            }));
          }
        } catch { /* keep mock groups */ }
      }
    } catch {
      // Keep mock data
    }
  });

  function formatRelativeTime(iso: string): string {
    try {
      const diffMin = Math.floor((Date.now() - new Date(iso).getTime()) / 60_000);
      if (diffMin < 1) return 'now';
      if (diffMin < 60) return `${diffMin}m`;
      const diffHr = Math.floor(diffMin / 60);
      if (diffHr < 24) return `${diffHr}h`;
      const diffDay = Math.floor(diffHr / 24);
      return `${diffDay}d`;
    } catch { return iso; }
  }

  const isSchedule = $derived(activePage === 'schedule');
  const isMarketplace = $derived(activePage === 'marketplace');

  const filteredChats = $derived(
    searchText
      ? chats.filter(c => c.title.toLowerCase().includes(searchText.toLowerCase()))
      : chats
  );

  const enabledCount = $derived(enabled ? Object.values(enabled).filter(Boolean).length : 0);

  const schedAgents = $derived(getScheduleAgents($userScheduleItems));
  function runsPerWeek(agentId: string) {
    return storeRunsPerWeek(agentId, $userScheduleItems);
  }

  const navLinks = [
    { href: '/marketplace', page: 'marketplace', icon: '◈', label: 'Marketplace' },
    { href: '/schedule', page: 'schedule', icon: '▦', label: 'Schedule' },
    { href: '/events', page: 'events', icon: '↯', label: 'Events' },
    { href: '/skills', page: 'skills', icon: '⚡', label: 'Skills' },
  ];

  const marketplaceLinks = [
    { id: 'featured', path: '/marketplace', label: 'Featured', icon: '◈' },
    { id: 'agents', path: '/marketplace/agents', label: 'Agents', icon: '🤖' },
    { id: 'skills', path: '/marketplace/skills', label: 'Skills', icon: '⚡' },
    { id: 'installed', path: '/marketplace/installed', label: 'Installed', icon: '✓' },
    null,
    { id: 'categories', path: '/marketplace/categories', label: 'Categories', icon: '▦' },
  ];
</script>

<aside class="flex flex-col border-r border-base-content/10 bg-base-100 shrink-0 overflow-hidden {collapsed ? 'w-14' : 'w-[260px]'}">
  <!-- Top: brand + collapse toggle -->
  <div class="h-12 px-3.5 border-b border-base-content/10 flex items-center gap-2.5">
    <a href="/" class="w-[22px] h-[22px] rounded-md bg-base-content text-base-100 flex items-center justify-center font-mono text-sm font-semibold shrink-0">N</a>
    {#if !collapsed}
      <a href="/" class="font-semibold text-sm tracking-tight flex-1">Nebo</a>
    {/if}
    <button class="text-base-content hover:text-base-content text-sm p-1 rounded cursor-pointer" onclick={() => collapsed = !collapsed}>
      {collapsed ? '→' : '←'}
    </button>
  </div>

  {#if !collapsed}
    {#if isSchedule && enabled}
      <!-- Schedule: agent toggles -->
      <div class="px-4 pt-3.5 pb-2 flex items-center justify-between">
        <span class="text-xs font-semibold uppercase tracking-wider">Agents</span>
        <span class="font-mono text-xs">{enabledCount} of {schedAgents.length}</span>
      </div>

      <div class="px-2 flex flex-col gap-px">
        {#each schedAgents as id}
          {@const a = AGENTS.find(x => x.id === id)}
          {@const c = AGENT_COLORS[id]}
          {@const on = enabled[id]}
          {#if a}
            <label class="flex items-center gap-2.5 px-2 py-1.5 rounded-md cursor-pointer text-sm hover:bg-base-content/5 transition-colors {on ? '' : 'opacity-60'}">
              <input type="checkbox" class="checkbox checkbox-xs {c.edgeClass}" checked={on} onchange={() => onToggleAgent?.(id)} />
              <span class="flex-1 font-medium">{a.name}</span>
              <span class="font-mono text-xs">{runsPerWeek(id)}/wk</span>
              <input type="checkbox" checked={on} onchange={() => onToggleAgent?.(id)} hidden />
            </label>
          {/if}
        {/each}
      </div>

      <div class="h-4"></div>

      <div class="px-4 pb-1 text-xs font-semibold uppercase tracking-wider">Trigger types</div>
      <div class="px-4 flex flex-col gap-2 text-sm">
        <span class="flex items-center gap-2">
          <span class="w-5 h-5 rounded bg-base-200 border border-base-content/10 font-mono text-xs inline-flex items-center justify-center">↻</span>
          Scheduled
        </span>
        <span class="flex items-center gap-2">
          <span class="w-5 h-5 rounded bg-base-200 border border-base-content/10 font-mono text-xs inline-flex items-center justify-center">⚡</span>
          Event
        </span>
        <span class="flex items-center gap-2">
          <span class="w-5 h-5 rounded bg-base-200 border border-base-content/10 font-mono text-xs inline-flex items-center justify-center">›</span>
          You
        </span>
      </div>

    {:else if isMarketplace}
      <!-- Marketplace: sub-nav -->
      <div class="px-3 pt-3 pb-1">
        <a href="/" class="flex items-center gap-1.5 px-2 py-1.5 text-sm hover:text-base-content transition-colors mb-2">← Back to chats</a>
      </div>
      <div class="px-4 py-1.5 text-xs font-semibold uppercase tracking-wider">Marketplace</div>
      <div class="px-2 flex flex-col gap-0.5">
        {#each marketplaceLinks as item}
          {#if item === null}
            <div class="h-2"></div>
          {:else}
            <a
              href={item.path}
              class="flex items-center gap-2 px-3 py-1.5 rounded-lg text-sm font-medium transition-colors {marketplaceTab === item.id
                ? 'bg-primary/10 text-primary'
                : 'text-base-content hover:bg-base-content/5'}"
            >
              <span class="w-4 text-center text-sm">{item.icon}</span>
              {item.label}
            </a>
          {/if}
        {/each}
      </div>

    {:else}
      <!-- Default: chats & agents -->
      <div class="px-3 pt-2.5 pb-1.5">
        <button class="w-full py-1.5 px-3 rounded-lg border border-dashed border-base-content/10 text-sm flex items-center gap-1.5 cursor-pointer hover:bg-base-content/5 transition-colors">
          <span>+</span> New chat
        </button>
      </div>

      <div class="px-3 pb-2">
        <input type="text" placeholder="Search chats…" bind:value={searchText}
          class="w-full py-1.5 px-2.5 rounded-md border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30 placeholder:text-base-content" />
      </div>

      <div class="px-4 pt-2 pb-1 text-xs font-semibold uppercase tracking-wider">Agents</div>
      <div class="px-2 flex flex-col gap-px">
        {#each agents.slice(0, 6) as agent}
          {@const c = AGENT_COLORS_MAP[agent.color]}
          <a href="/chat" class="flex items-center gap-2 py-1 px-2 rounded-md hover:bg-base-content/5 transition-colors">
            <span class="w-5 h-5 rounded text-sm font-mono font-semibold flex items-center justify-center shrink-0 {c.bgClass} {c.inkClass}">{agent.initial}</span>
            <span class="flex-1 text-sm font-medium truncate">{agent.name}</span>
            {#if agent.status === 'running'}
              <span class="w-1.5 h-1.5 rounded-full bg-warning shrink-0"></span>
            {:else if agent.status === 'online'}
              <span class="w-1.5 h-1.5 rounded-full bg-success shrink-0"></span>
            {/if}
          </a>
        {/each}
      </div>

      {#each chatGroups as group}
        {#if group.chats.length > 0}
          <div class="px-4 pt-2.5 pb-1 text-xs font-semibold uppercase tracking-wider">{group.label}</div>
          {#each group.chats as chatId}
            {@const chat = chats.find(c => c.id === chatId)}
            {#if chat}
              {@const c = AGENT_COLORS_MAP[chat.agentColor]}
              <a href="/chat" class="flex items-center gap-2 py-1 px-4 hover:bg-base-content/5 transition-colors {activeChat === chat.id ? 'bg-base-content/5' : ''}">
                <span class="w-4 h-4 rounded-sm text-sm font-mono font-semibold flex items-center justify-center shrink-0 {c.bgClass} {c.inkClass}">{chat.agent.charAt(0).toUpperCase()}</span>
                <span class="flex-1 text-sm truncate">{chat.title}</span>
                <span class="font-mono text-xs shrink-0">{chat.updatedAt}</span>
              </a>
            {/if}
          {/each}
        {/if}
      {/each}
    {/if}
  {:else}
    <!-- Collapsed icons -->
    <div class="py-3 flex flex-col items-center gap-2">
      <button class="w-7 h-7 rounded-md border border-base-content/10 flex items-center justify-center text-sm cursor-pointer hover:bg-base-content/5">+</button>
      {#each agents.slice(0, 6) as agent}
        {@const c = AGENT_COLORS_MAP[agent.color]}
        <span class="w-5 h-5 rounded text-sm font-mono font-semibold flex items-center justify-center {c.bgClass} {c.inkClass}" title={agent.name}>{agent.initial}</span>
      {/each}
    </div>
  {/if}

  <!-- Bottom: mini month (schedule) or spacer -->
  {#if isSchedule && !collapsed}
    <div class="flex-1"></div>
    <MiniMonth {selectedDate} onselect={(d: Date) => selectedDate = d} />
  {:else}
    <div class="flex-1"></div>
  {/if}

  <!-- Bottom nav -->
  <div class="p-2 flex flex-col gap-px border-t border-base-content/5">
    {#if !collapsed}
      {#each navLinks as link}
        <a href={link.href} class="flex items-center gap-2 py-1 px-2 rounded-md text-sm transition-colors {activePage === link.page ? 'bg-base-content/5 text-base-content font-medium' : 'text-base-content hover:bg-base-content/5 hover:text-base-content'}">
          <span class="w-4 text-center text-sm">{link.icon}</span> {link.label}
        </a>
      {/each}
      <div class="h-px bg-base-content/5 my-1 mx-2"></div>
      <a href="/settings/account" class="flex items-center gap-2 py-1 px-2 rounded-md text-sm transition-colors {activePage === 'settings' ? 'bg-base-content/5 text-base-content font-medium' : 'text-base-content hover:bg-base-content/5 hover:text-base-content'}">
        <span class="w-4 text-center text-sm">⚙</span> Settings
      </a>
      <a href="/upgrade" class="flex items-center gap-2 py-1 px-2 rounded-md text-sm transition-colors {activePage === 'upgrade' ? 'bg-base-content/5 text-base-content font-medium' : 'text-base-content hover:bg-base-content/5 hover:text-base-content'}">
        <span class="w-4 text-center text-sm">↑</span> Upgrade
      </a>
    {:else}
      <a href="/settings/account" class="w-7 h-7 mx-auto rounded-md border border-base-content/10 flex items-center justify-center text-sm cursor-pointer hover:bg-base-content/5" title="Settings">⚙</a>
    {/if}
  </div>
</aside>
