<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { page } from '$app/stores';
  import { AGENT_COLORS_MAP } from '$lib/tokens.js';
  import { transformViewsConfig } from '$lib/a2ui/transform.js';
  import type { A2UIView, A2UINavItem, A2UIViewsConfig } from '$lib/a2ui/types.js';
  import A2Surface from '$lib/a2ui/A2Surface.svelte';
  import ChatPane from '$lib/components/chat/ChatPane.svelte';

  const agentId = $derived($page.params.agentId);

  let agentName = $state('');
  let agentInitial = $state('');
  let agentColor = $state('teal');
  let config = $state<A2UIViewsConfig | null>(null);
  let chatMessages = $state<any[]>([]);
  let isLoading = $state(false);
  let streamingContent = $state('');

  const color = $derived(AGENT_COLORS_MAP[agentColor as keyof typeof AGENT_COLORS_MAP] ?? null);
  const navItems = $derived(config ? config._nav : []);

  let activeViewId = $state('');
  let chatOpen = $state(true);

  // Set initial view when config loads
  $effect(() => {
    if (navItems.length && !activeViewId) {
      activeViewId = navItems[0].viewId;
    }
  });

  const currentView = $derived(
    config && activeViewId ? (config[activeViewId] as A2UIView | undefined) : undefined
  );

  // Reactive view data
  let viewData = $state<Record<string, unknown>>({});
  $effect(() => {
    if (currentView) {
      viewData = { ...currentView.data };
    }
  });

  const reactiveView = $derived(
    currentView ? { ...currentView, data: viewData } : undefined
  );

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const detail = await api.getAgent(agentId);
      if (!detail) return;

      agentName = detail.agent.name;
      agentInitial = agentName.charAt(0).toUpperCase();

      if (detail.views) {
        const views = typeof detail.views === 'string'
          ? JSON.parse(detail.views)
          : detail.views;
        if (views && typeof views === 'object') {
          config = transformViewsConfig(views as Record<string, unknown>);
        }
      }
    } catch { /* agent not found */ }
  });

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

  // Wire actions to WebSocket
  function handleAction(name: string, payload?: Record<string, unknown>) {
    import('$lib/websocket/client').then(({ getWebSocketClient }) => {
      const ws = getWebSocketClient();
      ws.send('a2ui_action', {
        surfaceId: `agent:${agentId}:${activeViewId}`,
        name,
        sourceComponentId: '',
        context: payload ?? { type: 'agent' },
      });
    });
  }

  // Wire chat to WebSocket
  function handleSend(text: string) {
    chatMessages = [...chatMessages, { type: 'user', content: text }];
    isLoading = true;
    streamingContent = '';
    import('$lib/websocket/client').then(({ getWebSocketClient }) => {
      getWebSocketClient().send('chat', {
        session_id: `agent:${agentId}:web`,
        prompt: text,
        agent_id: agentId,
        channel: 'web',
      });
    });
  }

  function handleStop() {
    import('$lib/websocket/client').then(({ getWebSocketClient }) => {
      getWebSocketClient().send('cancel', {
        session_id: `agent:${agentId}:web`,
      });
    });
    isLoading = false;
  }

  // Listen for WS events
  const cleanups: (() => void)[] = [];

  onMount(() => {
    function onChatStream(e: Event) {
      const data = (e as CustomEvent).detail;
      if (data.session_id && !data.session_id.includes(agentId)) return;
      if (data.content || data.chunk || data.text) {
        streamingContent += data.content || data.chunk || data.text || '';
        const last = chatMessages[chatMessages.length - 1];
        if (last?.type === 'assistant') {
          chatMessages = [...chatMessages.slice(0, -1), { ...last, content: streamingContent }];
        } else {
          chatMessages = [...chatMessages, { type: 'assistant', content: streamingContent }];
        }
      }
    }

    function onChatMessage(e: Event) {
      const data = (e as CustomEvent).detail;
      if (data.session_id && !data.session_id.includes(agentId)) return;
      if (data.role === 'assistant' || data.type === 'assistant') {
        const content = data.content || data.text || '';
        if (content) {
          const last = chatMessages[chatMessages.length - 1];
          if (last?.type === 'assistant') {
            chatMessages = [...chatMessages.slice(0, -1), { type: 'assistant', content }];
          } else {
            chatMessages = [...chatMessages, { type: 'assistant', content }];
          }
        }
        streamingContent = '';
        isLoading = false;
      }
    }

    function onChatComplete() {
      isLoading = false;
      streamingContent = '';
    }

    function onA2UIData(e: Event) {
      const data = (e as CustomEvent).detail;
      if (data.path && data.value !== undefined) {
        const parts = (data.path as string).split('/').filter(Boolean);
        const updated = { ...viewData };
        let current: any = updated;
        for (let i = 0; i < parts.length - 1; i++) {
          if (current[parts[i]] === undefined) current[parts[i]] = {};
          current[parts[i]] = { ...current[parts[i]] };
          current = current[parts[i]];
        }
        current[parts[parts.length - 1]] = data.value;
        viewData = updated;
      }
    }

    window.addEventListener('nebo:chat_stream', onChatStream);
    window.addEventListener('nebo:chat_message', onChatMessage);
    window.addEventListener('nebo:chat_complete', onChatComplete);
    window.addEventListener('nebo:a2ui_data', onA2UIData);

    cleanups.push(() => {
      window.removeEventListener('nebo:chat_stream', onChatStream);
      window.removeEventListener('nebo:chat_message', onChatMessage);
      window.removeEventListener('nebo:chat_complete', onChatComplete);
      window.removeEventListener('nebo:a2ui_data', onA2UIData);
    });
  });

  onDestroy(() => {
    cleanups.forEach(fn => fn());
  });
</script>

<svelte:head>
  <title>{agentName || 'Workspace'} - Nebo</title>
</svelte:head>

{#if config && color}
  <div class="h-screen flex flex-col bg-base-100">
    <!-- Nav bar -->
    <div class="flex items-center border-b border-base-content/10 shrink-0 h-[44px] px-4 gap-0.5">
      <div class="w-6 h-6 rounded-md flex items-center justify-center text-xs font-mono font-semibold mr-2 {color.bgClass} {color.inkClass}">{agentInitial}</div>
      <span class="text-sm font-semibold mr-4">{agentName}</span>
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
        {#if reactiveView}
          <A2Surface view={reactiveView} onaction={handleAction} />
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
            {agentName}
            {agentId}
            placeholder="Message {agentName}..."
            onsend={(text) => handleSend(text)}
            onstop={handleStop}
            {isLoading}
          />
        </div>
      {/if}
    </div>
  </div>
{:else}
  <div class="h-screen flex items-center justify-center bg-base-100">
    <div class="text-center">
      {#if agentName && !config}
        <div class="text-base font-semibold mb-1">{agentName}</div>
        <div class="text-sm text-base-content/70">No workspace views configured</div>
      {:else}
        <div class="loading loading-spinner loading-md text-base-content/30"></div>
      {/if}
    </div>
  </div>
{/if}
