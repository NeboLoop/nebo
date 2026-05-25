<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { page } from '$app/stores';
  import ChatPane from '$lib/components/chat/ChatPane.svelte';
  import { getWebSocketClient } from '$lib/websocket/client';
  import { createChatController } from '$lib/chat/controller.svelte';
  import type { Agent, ChatMessage as ApiChatMessage } from '$lib/api/neboComponents';
  import { uploadFiles } from '$lib/api/upload';
  import type { UploadedAttachment } from '$lib/types/attachment';

  const agentId = $derived($page.params.agentId ?? '');

  let agentName = $state('');
  let placeholder = $state('');
  let appContext = $state<Record<string, unknown> | null>(null);

  // Read options from URL params
  const urlParams = $derived(new URLSearchParams($page.url.search));
  const paramPlaceholder = $derived(urlParams.get('placeholder') || '');
  const paramTheme = $derived(urlParams.get('theme') || '');
  const paramBorderless = $derived(urlParams.get('borderless') === '1');
  const paramCtx = $derived(urlParams.get('ctx') || '');
  const paramScope = $derived(urlParams.get('scope') || '');

  $effect(() => {
    if (paramPlaceholder) placeholder = paramPlaceholder;
  });

  // Apply theme if specified
  $effect(() => {
    if (paramTheme && paramTheme !== 'auto') {
      document.documentElement.setAttribute('data-theme', paramTheme === 'dark' ? 'nebo-dark' : 'nebo');
    }
  });

  const sessionKey = $derived(`agent:${agentId}:app${paramCtx ? ':' + paramCtx : ''}`);

  // Capture current values — embed agentId is stable (route doesn't change without full reload)
  const initialAgentId = $page.params.agentId ?? '';
  const initialSessionKey = `agent:${initialAgentId}:app${$page.url.searchParams.get('ctx') ? ':' + $page.url.searchParams.get('ctx') : ''}`;

  const chat = createChatController({
    agentId: initialAgentId,
    sessionKey: initialSessionKey,
    channel: 'app',
    onResponseComplete: (text) => {
      window.parent?.postMessage({ type: 'nebo:response-complete', text }, '*');
    },
  });

  // Slash commands that clear the conversation
  const CLEAR_COMMANDS = ['/new', '/clear'];

  async function handleSend(text: string, attachments?: UploadedAttachment[]) {
    const trimmed = text.trim().toLowerCase();
    const isClear = CLEAR_COMMANDS.includes(trimmed);

    if (isClear) chat.clearMessages();

    const extra: Record<string, unknown> = {};
    if (appContext) {
      extra.context = appContext;
    } else if (paramCtx) {
      extra.context = { displayedDoc: { documentId: paramCtx } };
    }
    if (paramScope) extra.scope = paramScope;

    chat.send(text, { extraPayload: extra, silent: isClear, attachments });
    window.parent?.postMessage({ type: 'nebo:message-sent', message: text }, '*');
  }

  const cleanups: (() => void)[] = [];

  onMount(async () => {
    const api = await import('$lib/api/nebo');

    // Fetch agent info
    try {
      const detail = await api.getAgent(agentId);
      agentName = detail.displayName || detail.agent?.name || agentId;
      if (!placeholder) {
        placeholder = `Message ${agentName}...`;
      }
    } catch { /* ignore */ }

    // Load agents for @mentions
    try {
      const resp = await api.listAgents();
      if (resp?.agents?.length) {
        chat.setAllAgents((resp.agents as Agent[]).map((a) => ({
          id: a.id,
          name: a.name,
          role: a.description || '',
          initial: a.name.charAt(0).toUpperCase(),
          status: a.isEnabled ? 'online' : 'paused',
          color: 'teal',
        })));
      }
    } catch (e) {
      console.warn('[chat-embed] Failed to load agents for @mentions:', e);
    }

    // Load existing chat history for this session
    try {
      const resp = await api.getSessionMessages(sessionKey);
      if (resp?.messages?.length) {
        const messages = resp.messages as ApiChatMessage[];
        chat.setMessages(messages
          .filter((m) => m.role === 'user' || m.role === 'assistant')
          .map((m) => ({
            id: m.id,
            type: m.role as 'user' | 'assistant',
            content: m.content,
            html: m.html || undefined,
          })));
      }
    } catch { /* first visit — no session yet */ }

    // Connect WebSocket (this page runs outside the root layout, so we bootstrap ourselves)
    const ws = getWebSocketClient();
    const token = localStorage.getItem('nebo_token');
    ws.connect(token || undefined);

    // Listen for postMessage commands from parent
    function onParentMessage(e: MessageEvent) {
      if (!e.data || typeof e.data.type !== 'string') return;
      switch (e.data.type) {
        case 'nebo:send':
          if (e.data.message) handleSend(e.data.message);
          break;
        case 'nebo:new-thread':
          chat.newThread();
          break;
        case 'nebo:set-context':
          appContext = e.data.context ?? null;
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
    chat.destroy();
  });
</script>

<div class="h-screen flex flex-col {paramBorderless ? '' : 'bg-base-100'}">
  <ChatPane
    messages={chat.messages}
    {agentName}
    {agentId}
    sessionId={sessionKey}
    {placeholder}
    allAgents={chat.allAgents}
    onsend={async (text, files) => {
      const attachments = files?.length ? await uploadFiles(files.map(f => f.file)) : undefined;
      handleSend(text, attachments);
    }}
    onstop={() => chat.stop()}
    isLoading={chat.isLoading}
    tokenUsage={chat.tokenUsage}
    quotaWarning={chat.quotaWarning}
    ondismisswarning={() => chat.dismissWarning()}
  />
</div>
