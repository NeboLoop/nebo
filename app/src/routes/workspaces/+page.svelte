<script lang="ts">
  import { onMount } from 'svelte';
  import { AGENT_COLORS_MAP } from '$lib/tokens.js';
  import { AGENT_VIEWS } from '$lib/a2ui/views/index.js';
  import type { A2UIView, A2UINavItem, A2UIViewsConfig } from '$lib/a2ui/types.js';
  import A2Surface from '$lib/a2ui/A2Surface.svelte';
  import ChatPane from '$lib/components/chat/ChatPane.svelte';
  import UserMenu from '$lib/components/UserMenu.svelte';
  import { sidebarCollapsedFor } from '$lib/stores/sidebar.js';
  import type { Agent } from '$lib/api/nebo';
  const sidebarCollapsed = sidebarCollapsedFor('workspaces');

  const COLOR_CYCLE = Object.keys(AGENT_COLORS_MAP);

  let allAgents = $state<{ id: string; name: string; initial: string; color: string; role: string; status: string }[]>([]);
  let chatMessages = $state<any[]>([]);

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.listAgents();
      if (resp?.agents?.length) {
        allAgents = resp.agents.map((a: Agent, i: number) => ({
          id: a.id,
          name: a.name,
          role: a.description || '',
          initial: a.name.charAt(0).toUpperCase(),
          status: a.isEnabled ? 'online' : 'paused',
          color: COLOR_CYCLE[i % COLOR_CYCLE.length],
        }));
      }
    } catch { /* keep mock data */ }
  });

  // Discover agents that have views
  const agentEntries = $derived(
    Object.entries(AGENT_VIEWS)
      .map(([id, config]) => {
        const agent = allAgents.find(a => a.id === id);
        return agent ? { id, agent, config } : null;
      })
      .filter((e): e is NonNullable<typeof e> => e !== null)
  );

  let selectedAgentId = $state('');
  let chatOpen = $state(true);
  let activeViewIds = $state<Record<string, string>>({});

  const selectedEntry = $derived(agentEntries.find(e => e.id === selectedAgentId));
  const navItems = $derived(selectedEntry ? (selectedEntry.config._nav as A2UINavItem[]) : []);
  const activeViewId = $derived(activeViewIds[selectedAgentId] || navItems[0]?.viewId || '');
  const currentView = $derived(
    selectedEntry && activeViewId
      ? (selectedEntry.config[activeViewId] as A2UIView | undefined)
      : undefined
  );
  const wsAgent = $derived(selectedEntry?.agent);
  const wsColor = $derived(wsAgent ? AGENT_COLORS_MAP[wsAgent.color as keyof typeof AGENT_COLORS_MAP] : null);

  function selectAgent(id: string) {
    if (selectedAgentId === id) {
      selectedAgentId = '';
    } else {
      selectedAgentId = id;
      if (!activeViewIds[id]) {
        const config = AGENT_VIEWS[id];
        if (config?._nav?.length) {
          activeViewIds[id] = config._nav[0].viewId;
        }
      }
    }
  }

  // Chat panel resize
  const CHAT_MIN = 280;
  const CHAT_DEFAULT = 360;
  let chatWidth = $state(CHAT_DEFAULT);
  let chatResizing = $state(false);
  let contentEl = $state<HTMLDivElement | null>(null);

  function startChatResize(e: MouseEvent) {
    e.preventDefault();
    chatResizing = true;
    const onMove = (ev: MouseEvent) => {
      if (!contentEl) return;
      const rect = contentEl.getBoundingClientRect();
      const newWidth = rect.right - ev.clientX;
      const maxWidth = rect.width * 0.6;
      chatWidth = Math.max(CHAT_MIN, Math.min(maxWidth, newWidth));
    };
    const onUp = () => {
      chatResizing = false;
      window.removeEventListener('mousemove', onMove);
      window.removeEventListener('mouseup', onUp);
    };
    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
  }

  function handleAction(name: string, payload?: Record<string, unknown>) {
    console.log('[A2UI Action]', name, payload);
  }

  function popOut() {
    if (selectedAgentId) {
      window.open(`/workspace/${selectedAgentId}`, `workspace-${selectedAgentId}`, 'width=1200,height=800');
    }
  }
</script>

<svelte:head><title>Workspaces - Nebo</title></svelte:head>

<!-- Left panel: agent list -->
<div class="{$sidebarCollapsed ? 'w-12 min-w-12' : 'w-[220px] min-w-[220px]'} border-r border-base-300 flex flex-col bg-base-200 shrink-0 transition-all duration-150">
  <div class="h-11 border-b border-base-300 flex items-center shrink-0 {$sidebarCollapsed ? 'justify-center' : 'px-3.5 justify-between'}">
    {#if !$sidebarCollapsed}
      <span class="text-sm font-semibold flex-1">Apps</span>
    {/if}
    <button class="w-7 h-7 rounded-md flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none shrink-0" onclick={() => $sidebarCollapsed = !$sidebarCollapsed} title={$sidebarCollapsed ? 'Expand sidebar' : 'Collapse sidebar'}>
      <svg width="16" height="16" viewBox="0 0 16 16" fill="none"><rect x="1.5" y="2.5" width="13" height="11" rx="1.5" stroke="currentColor" stroke-width="1.2"/><line x1="5.5" y1="3" x2="5.5" y2="13" stroke="currentColor" stroke-width="1.2"/></svg>
    </button>
  </div>
  <div class="flex-1 overflow-y-auto py-1">
    {#if $sidebarCollapsed}
      <div class="flex flex-col items-center gap-1 py-1">
        {#each agentEntries as entry}
          {@const c = AGENT_COLORS_MAP[entry.agent.color as keyof typeof AGENT_COLORS_MAP]}
          <button
            class="w-8 h-8 rounded-md flex items-center justify-center text-sm font-mono font-semibold shrink-0 cursor-pointer border-none transition-colors {c.bgClass} {c.inkClass} {selectedAgentId === entry.id ? 'ring-[1.5px] ring-base-content' : ''}"
            onclick={() => selectAgent(entry.id)}
            title={entry.agent.name}
          >{entry.agent.initial}</button>
        {/each}
      </div>
    {:else}
      {#each agentEntries as entry}
        {@const c = AGENT_COLORS_MAP[entry.agent.color as keyof typeof AGENT_COLORS_MAP]}
        <button
          class="w-full flex items-center gap-2 py-1.5 px-2.5 mx-1.5 rounded-md cursor-pointer transition-colors text-left {selectedAgentId === entry.id ? 'bg-base-100 shadow-[0_0_0_1px_var(--color-base-300)]' : 'hover:bg-base-100/70'}"
          onclick={() => selectAgent(entry.id)}
        >
          <div class="w-7 h-7 rounded-md flex items-center justify-center text-sm font-mono font-semibold shrink-0 {c.bgClass} {c.inkClass}">{entry.agent.initial}</div>
          <div class="flex-1 min-w-0">
            <div class="text-sm font-medium truncate">{entry.agent.name}</div>
            <div class="text-xs text-base-content/70 truncate">{entry.agent.role}</div>
          </div>
        </button>
      {/each}
    {/if}
  </div>
  <UserMenu collapsed={$sidebarCollapsed} />
</div>

<!-- Main content -->
<div class="flex-1 flex flex-col bg-base-100 min-w-0">
  {#if selectedEntry && wsAgent}
    <!-- Nav bar -->
    <div class="flex items-center border-b border-base-content/10 shrink-0 h-[44px] px-4 gap-0.5">
      {#each navItems as nav}
        <button
          class="flex items-center gap-1.5 py-1.5 px-3 rounded-md text-sm cursor-pointer border-none transition-colors {activeViewId === nav.viewId ? 'bg-base-100 shadow-[0_0_0_1px_var(--color-base-300)] font-medium text-base-content' : 'bg-transparent hover:bg-base-100/70'}"
          onclick={() => activeViewIds[selectedAgentId] = nav.viewId}
        >
          <span>{nav.label}</span>
        </button>
      {/each}
      <div class="flex-1"></div>
      <span class="text-xs text-base-content/50 mr-2">Powered by {wsAgent.name}</span>
      <button
        class="w-7 h-7 rounded-md flex items-center justify-center cursor-pointer bg-transparent border-none text-base-content/50 hover:text-base-content hover:bg-base-200/50 transition-colors mr-1"
        onclick={() => chatOpen = !chatOpen}
        title={chatOpen ? 'Hide chat' : 'Show chat'}
      >
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/></svg>
      </button>
      <button class="py-1 px-2.5 rounded-[5px] border border-base-300 bg-base-100 text-sm cursor-pointer hover:bg-base-200 transition-colors" onclick={popOut}>Pop out</button>
    </div>

    <!-- A2UI Surface + Chat -->
    <div class="flex-1 flex min-h-0 {chatResizing ? 'select-none' : ''}" bind:this={contentEl}>
      <div class="flex-1 overflow-y-auto p-5 min-w-0">
        {#if currentView}
          <A2Surface view={currentView} onaction={handleAction} />
        {/if}
      </div>
      {#if chatOpen}
        <!-- Resize handle -->
        <div
          class="w-0 shrink-0 cursor-col-resize relative z-10 group"
          onmousedown={startChatResize}
          role="separator"
          aria-orientation="vertical"
        >
          <div class="absolute inset-y-0 -left-2 -right-2"></div>
          <div class="absolute inset-y-0 -left-px w-0.5 bg-primary/30 opacity-0 group-hover:opacity-100 transition-opacity duration-300 delay-150 {chatResizing ? '!opacity-100' : ''}"></div>
          <div class="absolute top-1/2 -translate-y-1/2 -left-1.5 w-3 h-8 rounded-full bg-base-300 border border-base-content/10 flex items-center justify-center opacity-0 group-hover:opacity-100 transition-opacity duration-300 delay-150 {chatResizing ? '!opacity-100' : ''}">
            <div class="flex flex-col gap-0.5">
              <div class="w-0.5 h-0.5 rounded-full bg-base-content/30"></div>
              <div class="w-0.5 h-0.5 rounded-full bg-base-content/30"></div>
              <div class="w-0.5 h-0.5 rounded-full bg-base-content/30"></div>
            </div>
          </div>
        </div>
        <!-- Chat panel -->
        <div class="flex flex-col min-h-0 min-w-0 overflow-hidden shrink-0 border-l border-base-300" style="width: {chatWidth}px">
          <ChatPane
            messages={chatMessages}
            agentName={wsAgent.name}
            agentId={wsAgent.id}
            placeholder="Message {wsAgent.name}..."
          />
        </div>
      {/if}
    </div>

  {:else}
    <!-- No agent selected — grid overview -->
    <div class="h-11 px-[18px] border-b border-base-content/10 flex items-center gap-2 shrink-0">
      <span class="text-sm font-semibold">Apps</span>
    </div>
    <div class="flex-1 overflow-y-auto p-6">
      <div class="mb-5">
        <div class="text-base font-semibold mb-1">Your Apps</div>
        <div class="text-sm">Agent-powered applications. Open inline or pop out as standalone windows.</div>
      </div>
      <div class="grid grid-cols-2 gap-3">
        {#each agentEntries as entry}
          {@const c = AGENT_COLORS_MAP[entry.agent.color as keyof typeof AGENT_COLORS_MAP]}
          <button
            class="p-4 rounded-lg border border-base-300 bg-base-100 cursor-pointer hover:border-base-content/50 transition-colors text-left"
            onclick={() => selectAgent(entry.id)}
          >
            <div class="w-8 h-8 rounded-md flex items-center justify-center text-sm font-mono font-semibold mb-2 {c.bgClass} {c.inkClass}">{entry.agent.initial}</div>
            <div class="text-sm font-semibold mb-0.5">{entry.agent.name}</div>
            <div class="text-sm mb-2">{entry.agent.role}</div>
            <div class="flex items-center gap-1 text-xs text-base-content/70">
              {entry.config._nav.length} views
            </div>
          </button>
        {/each}
      </div>
    </div>
  {/if}
</div>
