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
  // Per-agent streaming: raw content for persistence, server-rendered HTML for display
  let streamingContent = $state<Record<string, string>>({});
  let streamingHtml = $state<Record<string, string>>({});
  let pendingTools = new Map<string, { idx: number; startTime: number }>();
  let phaseStartTime = 0;
  let allAgents = $state<{ id: string; name: string; role: string; initial: string; status: string; color: string }[]>([]);

  const displayMessages = $derived.by(() => {
    const extra: any[] = [];
    for (const [aid, html] of Object.entries(streamingHtml)) {
      if (html) {
        const isDelegate = aid !== agentId;
        const delegateAgent = isDelegate ? allAgents.find(a => a.id === aid) : null;
        extra.push({
          id: `streaming-${aid}`,
          type: 'assistant' as const,
          content: streamingContent[aid] || '',
          html,
          time: '',
          ...(delegateAgent ? {
            delegateAgentId: delegateAgent.id,
            delegateAgentName: delegateAgent.name,
          } : {}),
        });
      }
    }
    return [...messages, ...extra];
  });

  // Accept events from the primary agent OR from @mention delegates
  function isMyEvent(data: any): boolean {
    return data.agentId === agentId || data.originAgentId === agentId;
  }

  // Listen for WebSocket chat events
  function handleChatStream(e: Event) {
    const data = (e as CustomEvent).detail;
    if (!isMyEvent(data)) return;
    if (data.done) return;
    const aid = data.agentId || agentId;
    // Only set loading for the primary agent, not delegates
    if (aid === agentId && !isLoading) { isLoading = true; phaseStartTime = Date.now(); }
    // Accumulate raw content; use server-rendered HTML for display
    streamingContent[aid] = (streamingContent[aid] || '') + (data.chunk || data.content || '');
    if (data.html) {
      streamingHtml[aid] = data.html;
    }
  }

  function handleChatComplete(e: Event) {
    const data = (e as CustomEvent).detail;
    if (!isMyEvent(data)) return;
    const aid = data.agentId || agentId;
    const content = streamingContent[aid];
    if (content) {
      const isDelegate = aid !== agentId;
      const delegateAgent = isDelegate ? allAgents.find(a => a.id === aid) : null;
      messages = [...messages, {
        id: 'msg-' + Date.now(),
        type: 'assistant' as const,
        content,
        html: data.html || undefined,
        time: formatTime(Date.now()),
        ...(delegateAgent ? {
          delegateAgentId: delegateAgent.id,
          delegateAgentName: delegateAgent.name,
        } : {}),
      }];
      delete streamingContent[aid];
      delete streamingHtml[aid];
    }
    // Only clear loading when the primary agent completes
    if (aid === agentId) {
      isLoading = false;
      phaseStartTime = 0;
      pendingTools.clear();
    }
  }

  function handleChatMessage(e: Event) {
    const data = (e as CustomEvent).detail;
    if (!isMyEvent(data)) return;
    const aid = data.agentId || agentId;
    const isDelegate = aid !== agentId;
    const delegateAgent = isDelegate ? allAgents.find(a => a.id === aid) : null;
    messages = [...messages, {
      id: data.id || 'msg-' + Date.now(),
      type: 'assistant' as const,
      content: data.content || streamingContent[aid] || '',
      time: formatTime(data.createdAt || Date.now()),
      ...(delegateAgent ? {
        delegateAgentId: delegateAgent.id,
        delegateAgentName: delegateAgent.name,
      } : {}),
    }];
    delete streamingContent[aid];
    delete streamingHtml[aid];
    if (aid === agentId) isLoading = false;
  }

  function handleThinking(e: Event) {
    const data = (e as CustomEvent).detail;
    if (!isMyEvent(data)) return;
    const thinkAid = data.agentId || agentId;
    if (thinkAid === agentId && !isLoading) { isLoading = true; phaseStartTime = Date.now(); }
    const elapsed = phaseStartTime > 0 ? Math.round((Date.now() - phaseStartTime) / 1000) : 0;
    const duration = elapsed >= 60
      ? `${Math.floor(elapsed / 60)}m ${elapsed % 60}s`
      : `${elapsed}s`;
    messages = [...messages, {
      type: 'thinking' as const,
      content: data.content || '',
      duration,
    }];
  }

  function handleToolStart(e: Event) {
    const data = (e as CustomEvent).detail;
    if (!isMyEvent(data)) return;
    const toolAid = data.agentId || agentId;
    if (toolAid === agentId && !isLoading) { isLoading = true; phaseStartTime = Date.now(); }

    // If there's accumulated streaming text, commit it as an assistant message
    // BEFORE adding the tool. This ensures tool groups from different agentic
    // loop turns are separated by assistant text messages instead of all being
    // collapsed into a single "Used N tools" group.
    const pendingText = streamingContent[toolAid];
    if (pendingText) {
      const isDelegate = toolAid !== agentId;
      const delegateAgent = isDelegate ? allAgents.find(a => a.id === toolAid) : null;
      messages = [...messages, {
        id: 'msg-' + Date.now(),
        type: 'assistant' as const,
        content: pendingText,
        time: '',
        ...(delegateAgent ? {
          delegateAgentId: delegateAgent.id,
          delegateAgentName: delegateAgent.name,
        } : {}),
      }];
      delete streamingContent[toolAid];
      delete streamingHtml[toolAid];
    }

    let request: Record<string, unknown> = {};
    try {
      request = typeof data.input === 'string' ? JSON.parse(data.input) : (data.input || {});
    } catch { /* keep empty */ }
    const idx = messages.length;
    messages = [...messages, {
      type: 'tool' as const,
      name: data.tool || 'tool',
      status: 'running',
      duration: '...',
      request,
      response: '',
    }];
    if (data.tool_id) {
      pendingTools.set(data.tool_id, { idx, startTime: Date.now() });
    }
  }

  function handleToolResult(e: Event) {
    const data = (e as CustomEvent).detail;
    if (!isMyEvent(data)) return;
    const pending = data.tool_id ? pendingTools.get(data.tool_id) : undefined;
    if (pending) {
      const elapsed = Math.round((Date.now() - pending.startTime) / 1000);
      const duration = elapsed >= 60
        ? `${Math.floor(elapsed / 60)}m ${elapsed % 60}s`
        : `${elapsed}s`;
      const updated = [...messages];
      updated[pending.idx] = {
        ...updated[pending.idx],
        name: data.tool_name || updated[pending.idx].name,
        status: data.is_error ? 'error' : 'success',
        duration,
        response: typeof data.result === 'string' ? data.result : JSON.stringify(data.result, null, 2),
      };
      messages = updated;
      pendingTools.delete(data.tool_id);
    } else {
      // No matching start — add as standalone completed tool
      messages = [...messages, {
        type: 'tool' as const,
        name: data.tool_name || 'tool',
        status: data.is_error ? 'error' : 'success',
        duration: '0s',
        request: {},
        response: typeof data.result === 'string' ? data.result : JSON.stringify(data.result, null, 2),
      }];
    }
  }

  function handleAskRequest(e: Event) {
    const data = (e as CustomEvent).detail;
    if (!isMyEvent(data)) return;
    const requestId = data.request_id as string;
    const prompt = data.prompt as string;
    const widgets = data.widgets ?? [{ type: 'confirm', options: ['Yes', 'No'] }];
    if (requestId) {
      messages = [...messages, {
        type: 'ask' as const,
        requestId,
        prompt,
        widgets,
      }];
    }
  }

  async function handleAskSubmit(requestId: string, value: string) {
    try {
      const { getWebSocketClient } = await import('$lib/websocket/client');
      const ws = getWebSocketClient();
      if (ws.isConnected()) {
        ws.send('ask_response', { request_id: requestId, value });
      }
    } catch { /* ignore */ }
    // Update the ask message to show the chosen response
    messages = messages.map(msg =>
      msg.type === 'ask' && msg.requestId === requestId
        ? { ...msg, response: value }
        : msg
    );
  }

  onMount(async () => {
    // loadMessages() is handled by $effect on threadId — no need to call here
    window.addEventListener('nebo:chat_stream', handleChatStream);
    window.addEventListener('nebo:chat_complete', handleChatComplete);
    window.addEventListener('nebo:chat_message', handleChatMessage);
    window.addEventListener('nebo:thinking', handleThinking);
    window.addEventListener('nebo:tool_start', handleToolStart);
    window.addEventListener('nebo:tool_result', handleToolResult);
    window.addEventListener('nebo:ask_request', handleAskRequest);

    // Load agents for @mention chips
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.listAgents();
      if (resp?.agents?.length) {
        allAgents = resp.agents.map((a: any) => ({
          id: a.id,
          name: a.name,
          role: a.description || '',
          initial: a.name.charAt(0).toUpperCase(),
          status: a.isEnabled ? 'online' : 'paused',
          color: 'teal',
        }));
      }
    } catch { /* keep empty */ }
  });

  onDestroy(() => {
    if (typeof window !== 'undefined') {
      window.removeEventListener('nebo:chat_stream', handleChatStream);
      window.removeEventListener('nebo:chat_complete', handleChatComplete);
      window.removeEventListener('nebo:chat_message', handleChatMessage);
      window.removeEventListener('nebo:thinking', handleThinking);
      window.removeEventListener('nebo:tool_start', handleToolStart);
      window.removeEventListener('nebo:tool_result', handleToolResult);
      window.removeEventListener('nebo:ask_request', handleAskRequest);
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
        messages = resp.messages
          .filter((m: any) => m.role === 'user' || m.role === 'assistant')
          .map((m: any) => ({
            id: m.id,
            type: m.role as 'user' | 'assistant',
            content: m.content,
            html: m.html || undefined,
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
    phaseStartTime = Date.now();
    pendingTools.clear();

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

  async function handleStop() {
    try {
      const { getWebSocketClient } = await import('$lib/websocket/client');
      const ws = getWebSocketClient();
      if (ws.isConnected()) {
        ws.send('cancel', { agent_id: agentId });
      }
    } catch { /* ignore */ }
    isLoading = false;
    streamingContent = {};
    streamingHtml = {};
    phaseStartTime = 0;
  }

  function handleRedo(msgIndex: number) {
    // Find the most recent user message before this assistant message
    let userContent = '';
    for (let i = msgIndex - 1; i >= 0; i--) {
      if (messages[i]?.type === 'user') {
        userContent = messages[i].content;
        break;
      }
    }
    if (!userContent) return;
    // Truncate messages from the assistant response onward and resend
    messages = messages.slice(0, msgIndex);
    handleSend(userContent);
  }

  function handleEdit(msgIndex: number, newContent: string) {
    // Truncate from edited message onward, replace with new content, and resend
    messages = messages.slice(0, msgIndex);
    handleSend(newContent);
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
  {threadId}
  headerTitle={thread?.name ?? 'Thread'}
  headerRight="Creations"
  {allAgents}
  onsend={handleSend}
  onstop={handleStop}
  onedit={handleEdit}
  onredo={handleRedo}
  onasksubmit={handleAskSubmit}
  {isLoading}
/>
