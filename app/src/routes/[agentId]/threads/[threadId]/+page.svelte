<script lang="ts">
  import { getContext, onMount, onDestroy } from 'svelte';
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import ChatPane from '$lib/components/chat/ChatPane.svelte';
  import type { AgentPageContext, EnrichedChat } from '$lib/types/agentPage';
  import { createChatController, toolDisplayName, formatTime } from '$lib/chat/controller.svelte';
  import type { ChatMessage } from '$lib/chat/controller.svelte';
  import { getWebSocketClient } from '$lib/websocket/client';
  import type { Agent, ChatMessage as ApiChatMessage } from '$lib/api/neboComponents';
  import { uploadFiles } from '$lib/api/upload';

  // --- Metadata shapes embedded in API ChatMessage.metadata ---
  interface ToolCallMeta {
    name: string;
    input?: string | Record<string, unknown>;
    status?: string;
  }

  interface ContentBlockMeta {
    type: 'text' | 'tool';
    text?: string;
    toolCallIndex?: number;
  }

  interface MessageMeta {
    toolCalls?: ToolCallMeta[];
    contentBlocks?: ContentBlockMeta[];
  }

  const ctx = getContext<AgentPageContext>('agentPage');
  const agentId = $derived(ctx.agentId);
  const agent = $derived(ctx.agent);
  const threads = $derived(ctx.threads);

  const threadId = $derived($page.params.threadId);
  const thread = $derived(threads.find((t: EnrichedChat) => t.id === threadId));

  // Start loading immediately if navigated from a fresh send (?active=1)
  const startActive = $page.url.searchParams.get('active') === '1';

  // Use initial route params — sessionKey is updated reactively via setSessionKey in $effect below
  const initialAgentId = $page.params.agentId ?? '';
  const initialThreadId = $page.params.threadId ?? '';
  const chat = createChatController({ agentId: initialAgentId, sessionKey: `agent:${initialAgentId}:thread:${initialThreadId}` });

  // When navigated from a fresh send, the run is in-flight under the default session.
  // Listen for its completion by agentId to clear loading and reload messages.
  let activeRunUnsub: (() => void) | null = null;
  if (startActive) {
    chat.isLoading = true;
    const ws = getWebSocketClient();
    activeRunUnsub = ws.on<{ agentId: string }>('chat_complete', (data) => {
      if (data.agentId === agentId) {
        chat.isLoading = false;
        loadMessages();
        activeRunUnsub?.();
        activeRunUnsub = null;
      }
    });
  }

  // Pagination state
  let oldestMessageId = $state<string | null>(null);
  let totalMessages = $state(0);
  let loadedRawCount = $state(0);
  let isLoadingMore = $state(false);
  const hasMore = $derived(loadedRawCount < totalMessages);

  onMount(async () => {
    // Clean up ?active=1 query param so refresh doesn't re-trigger loading
    if (startActive) {
      goto(`/${agentId}/threads/${threadId}`, { replaceState: true, keepFocus: true, noScroll: true });
    }

    // Load agents for @mention chips
    try {
      const api = await import('$lib/api/nebo');
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
    } catch { /* keep empty */ }
  });

  onDestroy(() => {
    activeRunUnsub?.();
    chat.destroy();
  });

  // Reload when threadId changes and update session scope
  $effect(() => {
    if (threadId) {
      chat.setSessionKey(`agent:${agentId}:thread:${threadId}`);
      loadMessages();
    }
  });

  function parseToolInput(input: string | Record<string, unknown> | undefined): Record<string, unknown> {
    if (!input) return {};
    if (typeof input === 'string') {
      try { return JSON.parse(input); } catch { return {}; }
    }
    return input;
  }

  /** Parse raw API messages into ChatMessage[] for the controller. */
  function parseMessages(rawMessages: ApiChatMessage[]): ChatMessage[] {
    const result: ChatMessage[] = [];
    for (const m of rawMessages) {
      if (m.role === 'user') {
        result.push({
          type: 'user' as const,
          id: m.id,
          content: m.content,
          time: formatTime(m.createdAt),
        });
        continue;
      }
      if (m.role !== 'assistant') continue;

      let meta: MessageMeta | null = null;
      if (m.metadata) {
        try { meta = typeof m.metadata === 'string' ? JSON.parse(m.metadata) : m.metadata; } catch {}
      }
      const toolCalls: ToolCallMeta[] = meta?.toolCalls || [];
      const contentBlocks: ContentBlockMeta[] = meta?.contentBlocks || [];

      if (toolCalls.length && contentBlocks.length) {
        for (const block of contentBlocks) {
          if (block.type === 'text' && (block.text || m.content)) {
            result.push({
              type: 'assistant' as const,
              id: m.id,
              content: block.text || m.content || '',
              html: m.html || undefined,
              time: formatTime(m.createdAt),
            });
          } else if (block.type === 'tool' && block.toolCallIndex != null) {
            const tc = toolCalls[block.toolCallIndex];
            if (tc) {
              const request = parseToolInput(tc.input);
              result.push({
                type: 'tool' as const,
                name: toolDisplayName(tc.name || 'tool', request),
                status: tc.status === 'error' ? 'error' : 'success',
                duration: '',
                request,
                response: '',
              });
            }
          }
        }
      } else if (toolCalls.length) {
        if (m.content) {
          result.push({
            type: 'assistant' as const,
            id: m.id,
            content: m.content,
            html: m.html || undefined,
            time: formatTime(m.createdAt),
          });
        }
        for (const tc of toolCalls) {
          const request = parseToolInput(tc.input);
          result.push({
            type: 'tool' as const,
            name: tc.name || 'tool',
            status: tc.status === 'error' ? 'error' : 'success',
            duration: '',
            request,
            response: '',
          });
        }
      } else if (m.content) {
        result.push({
          type: 'assistant' as const,
          id: m.id,
          content: m.content,
          html: m.html || undefined,
          time: formatTime(m.createdAt),
        });
      }
    }
    return result;
  }

  async function loadMessages() {
    if (!threadId) return;
    oldestMessageId = null;
    loadedRawCount = 0;
    totalMessages = 0;
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.getChatMessages(threadId);
      if (resp?.messages?.length) {
        totalMessages = resp.totalMessages ?? resp.messages.length;
        loadedRawCount = resp.messages.length;
        oldestMessageId = resp.messages[0]?.id ?? null;
        chat.setMessages(parseMessages(resp.messages));
      }
    } catch (e) {
      console.warn('[nebo] Failed to load messages for thread', threadId, e);
    }
  }

  async function loadOlderMessages() {
    if (!threadId || !oldestMessageId || isLoadingMore || !hasMore) return;
    isLoadingMore = true;
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.getChatMessages(threadId, undefined, oldestMessageId);
      if (resp?.messages?.length) {
        loadedRawCount += resp.messages.length;
        oldestMessageId = resp.messages[0]?.id ?? oldestMessageId;
        chat.prependMessages(parseMessages(resp.messages));
      } else {
        // No more messages — stop pagination to prevent infinite re-triggers
        totalMessages = loadedRawCount;
      }
    } catch (e) {
      console.warn('[nebo] Failed to load older messages', e);
      // On error, stop pagination to prevent infinite retry loop
      totalMessages = loadedRawCount;
    } finally {
      isLoadingMore = false;
    }
  }
</script>

<ChatPane
  messages={chat.messages}
  agentName={agent?.name ?? 'Agent'}
  agentId={agentId}
  {threadId}
  headerTitle={thread?.name ?? 'Thread'}
  headerRight="Creations"
  allAgents={chat.allAgents}
  tokenUsage={chat.tokenUsage}
  quotaWarning={chat.quotaWarning}
  followupSuggestions={chat.followupSuggestions}
  activityStatus={chat.activityStatus}
  {hasMore}
  {isLoadingMore}
  onloadmore={loadOlderMessages}
  onsend={async (text, files) => {
    const attachments = files?.length ? await uploadFiles(files.map(f => f.file)) : undefined;
    chat.send(text, { attachments });
  }}
  onstop={() => chat.stop()}
  onedit={(idx, text) => chat.edit(idx, text)}
  onredo={(idx) => chat.redo(idx)}
  onasksubmit={(id, val) => chat.submitAsk(id, val)}
  onfollowupselect={(s) => chat.send(s)}
  ondismissfollowups={() => chat.dismissFollowups()}
  ondismisswarning={() => chat.dismissWarning()}
  isLoading={chat.isLoading}
/>
