<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import { AGENT_COLORS_MAP } from '$lib/tokens.js';
  import { AGENT_VIEWS } from '$lib/a2ui/views/index.js';
  import type { A2UIView, A2UINavItem } from '$lib/a2ui/types.js';
  import A2Surface from '$lib/a2ui/A2Surface.svelte';
  import ChatPane from '$lib/components/chat/ChatPane.svelte';
  import type { Agent } from '$lib/api/nebo';

  let allAgents = $state<{ id: string; name: string; initial: string; color: string; role: string; status: string; editable?: boolean }[]>([]);
  let chatMessages = $state<any[]>([]);

  const agentId = $derived($page.params.agentId);
  const config = $derived(agentId ? AGENT_VIEWS[agentId] : undefined);
  const agent = $derived(allAgents.find(a => a.id === agentId));
  const color = $derived(agent ? AGENT_COLORS_MAP[agent.color as keyof typeof AGENT_COLORS_MAP] : null);

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const res = await api.listAgents();
      if (res?.agents?.length) {
        allAgents = res.agents.map((a: Agent) => ({
          id: a.id,
          name: a.name,
          initial: (a.name || '')[0] || '?',
          color: 'violet',
          role: a.description || '',
          status: a.isEnabled ? 'online' : 'idle',
          editable: true,
        }));
      }
    } catch { /* keep mock data */ }
  });
  const navItems = $derived(config ? (config._nav as A2UINavItem[]) : []);

  let activeViewId = $state('');
  let chatOpen = $state(true);

  // Set initial view when agent loads
  $effect(() => {
    if (navItems.length && !activeViewId) {
      activeViewId = navItems[0].viewId;
    }
  });

  const currentView = $derived(
    config && activeViewId ? (config[activeViewId] as A2UIView | undefined) : undefined
  );

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
</script>

<svelte:head>
  <title>{agent?.name ?? 'Workspace'} - Nebo</title>
</svelte:head>

{#if config && agent && color}
  <div class="h-screen flex flex-col bg-base-100">
    <!-- Nav bar -->
    <div class="flex items-center border-b border-base-content/10 shrink-0 h-[44px] px-4 gap-0.5">
      <div class="w-6 h-6 rounded-md flex items-center justify-center text-xs font-mono font-semibold mr-2 {color.bgClass} {color.inkClass}">{agent.initial}</div>
      <span class="text-sm font-semibold mr-4">{agent.name}</span>
      {#each navItems as nav}
        <button
          class="flex items-center gap-1.5 py-1.5 px-3 rounded-md text-sm cursor-pointer border-none transition-colors {activeViewId === nav.viewId ? 'bg-base-100 shadow-[0_0_0_1px_var(--color-base-300)] font-medium text-base-content' : 'bg-transparent hover:bg-base-100/70'}"
          onclick={() => activeViewId = nav.viewId}
        >
          <span>{nav.label}</span>
        </button>
      {/each}
      <div class="flex-1"></div>
      <button
        class="w-7 h-7 rounded-md flex items-center justify-center cursor-pointer bg-transparent border-none text-base-content/50 hover:text-base-content hover:bg-base-200/50 transition-colors"
        onclick={() => chatOpen = !chatOpen}
        title={chatOpen ? 'Hide chat' : 'Show chat'}
      >
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/></svg>
      </button>
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
            agentName={agent.name}
            agentId={agent.id}
            placeholder="Message {agent.name}..."
          />
        </div>
      {/if}
    </div>
  </div>
{:else}
  <div class="h-screen flex items-center justify-center bg-base-100">
    <div class="text-center">
      <div class="text-base font-semibold mb-1">Agent not found</div>
      <div class="text-sm text-base-content/70">No workspace views configured for "{agentId}"</div>
    </div>
  </div>
{/if}
