<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { page } from '$app/stores';
  import ChatPane from '$lib/components/chat/ChatPane.svelte';
  import { getWebSocketClient } from '$lib/websocket/client';

  type AgentInfo = { id: string; name: string; role: string; initial: string; status: string; color: string };

  const agentId = $derived($page.params.agentId ?? '');

  let agentName = $state('');
  let chatMessages = $state<any[]>([]);
  let allAgents = $state<AgentInfo[]>([]);
  let isLoading = $state(false);
  let streamingContent = $state('');
  let placeholder = $state('');

  // Read options from URL params
  const urlParams = $derived(new URLSearchParams($page.url.search));
  const paramPlaceholder = $derived(urlParams.get('placeholder') || '');
  const paramTheme = $derived(urlParams.get('theme') || '');
  const paramBorderless = $derived(urlParams.get('borderless') === '1');

  $effect(() => {
    if (paramPlaceholder) placeholder = paramPlaceholder;
  });

  // Apply theme if specified
  $effect(() => {
    if (paramTheme && paramTheme !== 'auto') {
      document.documentElement.setAttribute('data-theme', paramTheme === 'dark' ? 'nebo-dark' : 'nebo');
    }
  });

  const sessionKey = $derived(`agent:${agentId}:app`);

  // Slash commands that clear the conversation
  const CLEAR_COMMANDS = ['/new', '/clear'];

  function handleSend(text: string) {
    const trimmed = text.trim().toLowerCase();

    // /new and /clear: clear local messages, then send to server
    if (CLEAR_COMMANDS.includes(trimmed)) {
      chatMessages = [];
      streamingContent = '';
    } else {
      chatMessages = [...chatMessages, { type: 'user', content: text }];
    }

    isLoading = true;
    streamingContent = '';

    const ws = getWebSocketClient();
    ws.send('chat', {
      session_id: sessionKey,
      prompt: text,
      agent_id: agentId,
      channel: 'app',
    });

    // Notify parent
    window.parent?.postMessage({ type: 'nebo:message-sent', message: text }, '*');
  }

  function handleStop() {
    const ws = getWebSocketClient();
    ws.send('cancel', { session_id: sessionKey });
    isLoading = false;
  }

  function newThread() {
    chatMessages = [];
    streamingContent = '';
    isLoading = false;

    const ws = getWebSocketClient();
    ws.send('rotate_chat', { session_id: sessionKey });
  }

  const cleanups: (() => void)[] = [];

  onMount(async () => {
    const api = await import('$lib/api/nebo');

    // Fetch agent info
    try {
      const resp = await fetch(`/api/v1/agents/${agentId}`);
      if (resp.ok) {
        const detail = await resp.json();
        agentName = detail.displayName || detail.agent?.name || agentId;
        if (!placeholder) {
          placeholder = `Message ${agentName}...`;
        }
      }
    } catch { /* ignore */ }

    // Load agents for @mentions
    try {
      const resp = await api.listAgents();
      console.log('[chat-embed] listAgents response:', resp?.agents?.length, 'agents');
      if (resp?.agents?.length) {
        allAgents = resp.agents.map((a: any) => ({
          id: a.id,
          name: a.name,
          role: a.description || '',
          initial: a.name.charAt(0).toUpperCase(),
          status: a.isEnabled ? 'online' : 'paused',
          color: 'teal',
        }));
        console.log('[chat-embed] allAgents set:', allAgents.length, 'agents, current agentId:', agentId);
      }
    } catch (e) {
      console.warn('[chat-embed] Failed to load agents for @mentions:', e);
    }

    // Load existing chat history for this session
    try {
      const resp = await api.getSessionMessages(sessionKey);
      if (resp?.messages?.length) {
        chatMessages = resp.messages
          .filter((m: any) => m.role === 'user' || m.role === 'assistant')
          .map((m: any) => ({
            id: m.id,
            type: m.role as 'user' | 'assistant',
            content: m.content,
            html: m.html || undefined,
          }));
      }
    } catch { /* first visit — no session yet */ }

    // Connect WebSocket (this page runs outside the root layout, so we bootstrap ourselves)
    const ws = getWebSocketClient();
    const token = localStorage.getItem('nebo_token');
    ws.connect(token || undefined);

    // Listen for chat events
    const offStream = ws.on('chat_stream', (data: any) => {
      if (data.session_id && !data.session_id.includes(agentId)) return;
      const chunk = data.content || data.chunk || data.text || '';
      if (chunk) {
        streamingContent += chunk;
        const last = chatMessages[chatMessages.length - 1];
        if (last?.type === 'assistant') {
          chatMessages = [...chatMessages.slice(0, -1), { ...last, content: streamingContent }];
        } else {
          chatMessages = [...chatMessages, { type: 'assistant', content: streamingContent }];
        }
      }
    });
    cleanups.push(offStream);

    const offMessage = ws.on('chat_message', (data: any) => {
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
        // Notify parent
        window.parent?.postMessage({
          type: 'nebo:response-complete',
          text: content || streamingContent,
        }, '*');
        streamingContent = '';
        isLoading = false;
      }
    });
    cleanups.push(offMessage);

    const offComplete = ws.on('chat_complete', (data: any) => {
      if (data.session_id && !data.session_id.includes(agentId)) return;
      isLoading = false;
      streamingContent = '';
    });
    cleanups.push(offComplete);

    // Listen for session_reset events (server-side /new or /clear)
    const offReset = ws.on('session_reset', (data: any) => {
      if (data.session_id && !data.session_id.includes(agentId)) return;
      if (data.success) {
        chatMessages = [];
        streamingContent = '';
      }
    });
    cleanups.push(offReset);

    // Listen for postMessage commands from parent
    function onParentMessage(e: MessageEvent) {
      if (!e.data || typeof e.data.type !== 'string') return;
      switch (e.data.type) {
        case 'nebo:send':
          if (e.data.message) handleSend(e.data.message);
          break;
        case 'nebo:new-thread':
          newThread();
          break;
        case 'nebo:configure':
          if (e.data.options?.placeholder) placeholder = e.data.options.placeholder;
          break;
      }
    }
    window.addEventListener('message', onParentMessage);
    cleanups.push(() => window.removeEventListener('message', onParentMessage));

    // Notify parent we're ready
    window.parent?.postMessage({ type: 'nebo:ready' }, '*');
  });

  onDestroy(() => {
    cleanups.forEach(fn => fn());
  });
</script>

<div class="h-screen flex flex-col {paramBorderless ? '' : 'bg-base-100'}">
  <ChatPane
    messages={chatMessages}
    {agentName}
    {agentId}
    {placeholder}
    {allAgents}
    onsend={(text) => handleSend(text)}
    onstop={handleStop}
    {isLoading}
  />
</div>
