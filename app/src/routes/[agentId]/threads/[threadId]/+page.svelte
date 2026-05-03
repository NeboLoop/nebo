<script lang="ts">
  import { getContext, onMount, onDestroy } from 'svelte';
  import ChatPane from '$lib/components/chat/ChatPane.svelte';
  import type { AgentPageContext, EnrichedChat } from '$lib/types/agentPage';

  const ctx = getContext<AgentPageContext>('agentPage');
  const agentId = $derived(ctx.agentId);
  const agent = $derived(ctx.agent);
  const threads = $derived(ctx.threads);

  import { page } from '$app/stores';
  const threadId = $derived($page.params.threadId);
  const thread = $derived(threads.find((t: EnrichedChat) => t.id === threadId));

  let messages = $state<any[]>([]);
  let isLoading = $state(false);
  let streamingContent = $state('');
  const displayMessages = $derived(
    streamingContent
      ? [...messages, { id: 'streaming', type: 'assistant' as const, content: streamingContent, time: '' }]
      : messages
  );

  // Listen for WebSocket chat events
  function handleChatStream(e: Event) {
    const data = (e as CustomEvent).detail;
    if (data.agentId !== agentId) return;
    if (data.done) return;
    streamingContent += data.chunk || data.content || '';
  }

  function handleChatComplete(e: Event) {
    const data = (e as CustomEvent).detail;
    if (data.agentId !== agentId) return;
    if (streamingContent) {
      messages = [...messages, {
        id: 'msg-' + Date.now(),
        type: 'assistant' as const,
        content: streamingContent,
        time: formatTime(Date.now()),
      }];
      streamingContent = '';
    }
    isLoading = false;
  }

  function handleChatMessage(e: Event) {
    const data = (e as CustomEvent).detail;
    if (data.agentId !== agentId) return;
    messages = [...messages, {
      id: data.id || 'msg-' + Date.now(),
      type: 'assistant' as const,
      content: data.content || streamingContent,
      time: formatTime(data.createdAt || Date.now()),
    }];
    streamingContent = '';
    isLoading = false;
  }

  onMount(() => {
    loadMessages();
    window.addEventListener('nebo:chat_stream', handleChatStream);
    window.addEventListener('nebo:chat_complete', handleChatComplete);
    window.addEventListener('nebo:chat_message', handleChatMessage);
  });

  onDestroy(() => {
    if (typeof window !== 'undefined') {
      window.removeEventListener('nebo:chat_stream', handleChatStream);
      window.removeEventListener('nebo:chat_complete', handleChatComplete);
      window.removeEventListener('nebo:chat_message', handleChatMessage);
    }
  });

  // Reload when threadId changes
  $effect(() => {
    if (threadId) loadMessages();
  });

  async function loadMessages() {
    if (!threadId) return;
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.getChatMessages(threadId);
      if (resp?.messages?.length) {
        messages = resp.messages.map((m: any) => ({
          id: m.id,
          type: m.role as 'user' | 'assistant',
          content: m.content,
          time: formatTime(m.createdAt),
        }));
      }
    } catch (e) {
      console.warn('[nebo] Failed to load messages for thread', threadId, e);
    }
  }

  async function handleSend(text: string) {
    const userMsg = { id: 'msg-' + Date.now(), type: 'user' as const, content: text, time: 'now' };
    messages = [...messages, userMsg];
    isLoading = true;

    try {
      const { getWebSocketClient } = await import('$lib/websocket/client');
      const ws = getWebSocketClient();
      if (ws.isConnected()) {
        ws.send('chat', { prompt: text, agent_id: agentId });
      } else {
        const api = await import('$lib/api/nebo');
        await api.chatWithAgent(agentId, { prompt: text });
      }
    } catch {
      // Message will appear locally even if send fails
    }
  }

  function formatTime(ts: string | number): string {
    try {
      const n = typeof ts === 'number' ? ts : Number(ts);
      const date = !isNaN(n) && n > 0
        ? new Date(n < 1e12 ? n * 1000 : n)
        : new Date(String(ts));
      if (isNaN(date.getTime())) return '';
      return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    } catch { return ''; }
  }
</script>

<ChatPane
  messages={displayMessages}
  agentName={agent?.name ?? 'Agent'}
  agentId={agentId}
  headerTitle={thread?.name ?? 'Thread'}
  headerRight="Creations"
  onsend={handleSend}
  {isLoading}
/>
