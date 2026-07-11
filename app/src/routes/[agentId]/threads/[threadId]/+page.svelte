<script lang="ts">
  import { getContext, onMount, onDestroy } from 'svelte';
  import { page } from '$app/stores';
  import ChatPane from '$lib/components/chat/ChatPane.svelte';
  import type { AgentPageContext, EnrichedChat } from '$lib/types/agentPage';
  import { createChatController, toolDisplayName, formatTime, artifactsToWorkItems, artifactsToAttachments } from '$lib/chat/controller.svelte';
  import type { ChatMessage } from '$lib/chat/controller.svelte';
  import { getWebSocketClient } from '$lib/websocket/client';
  import type { Agent, ChatMessage as ApiChatMessage } from '$lib/api/neboComponents';
  import { uploadFiles } from '$lib/api/upload';

  const PENDING_SEND_PREFIX = 'nebo:pending-send:';
  const PENDING_ERROR_PREFIX = 'nebo:pending-error:';

  type PendingSend = { text: string; sent?: boolean; ts?: number };

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
    /** System-injected messages (e.g. <system-reminder> steering) — visible to
     * the model, hidden from the user. Never render these as chat bubbles. */
    hidden?: boolean;
    /** Run-produced artifact URLs persisted at chat_complete (Work items + inline media). */
    artifacts?: string[];
  }

  const ctx = getContext<AgentPageContext>('agentPage');
  const agentId = $derived(ctx.agentId);
  const agent = $derived(ctx.agent);
  const threads = $derived(ctx.threads);

  const threadId = $derived($page.params.threadId);
  const thread = $derived(threads.find((t: EnrichedChat) => t.id === threadId));

  // Start loading immediately if navigated from a fresh send (?active=1)
  const startActive = $page.url.searchParams.get('active') === '1';

  const initialAgentId = $page.params.agentId ?? '';
  const initialThreadId = $page.params.threadId ?? '';
  const chat = createChatController({ agentId: initialAgentId, sessionKey: `agent:${initialAgentId}:thread:${initialThreadId}` });

  // When navigated from a fresh send, the run is started on THIS page (after
  // subscribe). Settle listeners clear the pending-send stash and strip ?active=1
  // without a SvelteKit goto (goto can remount and drop chat_error / the bubble).
  let activeRunUnsubs: Array<() => void> = [];
  let pendingSendStarted = false;
  let firstRunSettled = false;

  function pendingSendKey(id: string) {
    return `${PENDING_SEND_PREFIX}${id}`;
  }
  function pendingErrorKey(id: string) {
    return `${PENDING_ERROR_PREFIX}${id}`;
  }

  function clearActiveQueryParam() {
    if (typeof window === 'undefined') return;
    const url = new URL(window.location.href);
    if (!url.searchParams.has('active')) return;
    url.searchParams.delete('active');
    const next = url.pathname + (url.search ? url.search : '');
    history.replaceState(history.state, '', next);
  }

  function settleFirstRun(opts?: { clearPendingSend?: boolean }) {
    if (firstRunSettled) return;
    firstRunSettled = true;
    if (opts?.clearPendingSend !== false && initialThreadId) {
      sessionStorage.removeItem(pendingSendKey(initialThreadId));
    }
    clearActiveQueryParam();
    for (const off of activeRunUnsubs) off();
    activeRunUnsubs = [];
  }

  function isFirstRunEvent(data: { agentId?: string; session_id?: string }) {
    const sk = `agent:${initialAgentId}:thread:${initialThreadId}`;
    if (data.session_id && data.session_id !== sk) return false;
    if (data.agentId && data.agentId !== initialAgentId) return false;
    return true;
  }

  if (startActive) {
    chat.isLoading = true;
    const ws = getWebSocketClient();
    activeRunUnsubs.push(ws.on<{ agentId?: string; session_id?: string; error?: string }>('chat_error', (data) => {
      if (!isFirstRunEvent(data)) return;
      const message = data.error || 'Something went wrong.';
      // Survive a remount: keep pending-send + stash the error so a new page
      // instance can restore the bubble and the provider banner.
      sessionStorage.setItem(pendingErrorKey(initialThreadId), message);
      chat.setError(message);
      settleFirstRun({ clearPendingSend: false });
    }));
    activeRunUnsubs.push(ws.on<{ agentId?: string; session_id?: string }>('chat_complete', (data) => {
      if (!isFirstRunEvent(data)) return;
      chat.isLoading = false;
      // Successful runs persist messages — reload so IDs match the DB.
      // Provider errors reject before persistence; skip reload so we keep the
      // optimistic user bubble + error banner.
      if (!chat.chatError) {
        sessionStorage.removeItem(pendingErrorKey(initialThreadId));
        loadMessages();
        settleFirstRun({ clearPendingSend: true });
      } else {
        settleFirstRun({ clearPendingSend: false });
      }
    }));
  }

  // Pagination state
  let oldestMessageId = $state<string | null>(null);
  let totalMessages = $state(0);
  let loadedRawCount = $state(0);
  let isLoadingMore = $state(false);
  const hasMore = $derived(loadedRawCount < totalMessages);

  onMount(async () => {
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
          color: a.color || 'teal',
          isApp: a.isApp ?? false,
        })));
      }
    } catch { /* keep empty */ }
  });

  onDestroy(() => {
    for (const off of activeRunUnsubs) off();
    activeRunUnsubs = [];
    chat.destroy();
  });

  $effect(() => {
    if (threadId && agentId) {
      const sk = `agent:${agentId}:thread:${threadId}`;
      chat.setSessionKey(sk);

      // Restore a chat_error stashed when the first-send page instance was torn
      // down before the banner could render.
      const errKey = pendingErrorKey(threadId);
      const stashedError = sessionStorage.getItem(errKey);
      if (stashedError) {
        chat.setError(stashedError);
        sessionStorage.removeItem(errKey);
      }

      // Fresh send from /threads: prompt was stashed so we send only after this
      // page's controller is subscribed (avoids the disappearing first message).
      // Keep the stash until settled so a remount can restore the bubble without
      // double-sending.
      if (!pendingSendStarted) {
        const key = pendingSendKey(threadId);
        const raw = sessionStorage.getItem(key);
        if (raw) {
          try {
            const parsed = JSON.parse(raw) as PendingSend;
            if (parsed.text?.trim()) {
              pendingSendStarted = true;
              if (!parsed.sent) {
                sessionStorage.setItem(key, JSON.stringify({ ...parsed, sent: true }));
                chat.send(parsed.text);
                return;
              }
              // Remount after send already went out — restore bubble, keep waiting.
              if (chat.messages.length === 0) {
                chat.setMessages([{
                  id: 'msg-pending',
                  type: 'user',
                  content: parsed.text,
                  time: formatTime(Date.now()),
                }]);
              }
              chat.isLoading = !chat.chatError;
              if (chat.chatError) sessionStorage.removeItem(key);
              return;
            }
          } catch {
            /* fall through to history load */
          }
        }
      }

      // Don't clobber an in-flight first send with an empty/partial history fetch.
      if (!pendingSendStarted) {
        loadMessages();
      }
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
      let meta: MessageMeta | null = null;
      if (m.metadata) {
        try { meta = typeof m.metadata === 'string' ? JSON.parse(m.metadata) : m.metadata; } catch {}
      }
      // System-injected messages (steering reminders, post-tool nudges) are for
      // the model only — never render them as chat bubbles.
      if (meta?.hidden) continue;

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

      const toolCalls: ToolCallMeta[] = meta?.toolCalls || [];
      const contentBlocks: ContentBlockMeta[] = meta?.contentBlocks || [];

      // Rebuild the turn as nested assistant bubbles: each narration segment owns
      // the tools that followed it (tools live ON the message, never as sibling
      // entries — so they can't orphan). The persisted contentBlocks preserve the
      // exact text/tool interleaving; this mirrors the live controller + NeboLoop.
      type AssistantMsg = Extract<ChatMessage, { type: 'assistant' }>;
      const bubbles: AssistantMsg[] = [];
      let cur: AssistantMsg | null = null;
      let seq = 0;
      const newBubble = (content: string): AssistantMsg => {
        const b: AssistantMsg = { type: 'assistant', id: `${m.id}-${seq++}`, content, time: formatTime(m.createdAt) };
        bubbles.push(b);
        return b;
      };
      const pushTool = (target: AssistantMsg, tc: ToolCallMeta) => {
        const request = parseToolInput(tc.input);
        (target.tools ??= []).push({
          // Raw name so the display formats the signature; persisted records carry
          // no humanized outcome, so toolDisplayName is the friendly fallback label.
          name: tc.name || 'tool',
          label: toolDisplayName(tc.name || 'tool', request),
          status: tc.status === 'error' ? 'error' : 'success',
          request,
          response: '',
        });
      };

      if (toolCalls.length && contentBlocks.length) {
        for (const block of contentBlocks) {
          if (block.type === 'text') {
            const text = block.text || '';
            // Text after this bubble ran tools starts a fresh bubble.
            if (!cur || cur.tools?.length) cur = newBubble(text);
            else cur.content = cur.content ? `${cur.content}\n${text}` : text;
          } else if (block.type === 'tool' && block.toolCallIndex != null) {
            const tc = toolCalls[block.toolCallIndex];
            if (tc) { if (!cur) cur = newBubble(''); pushTool(cur, tc); }
          }
        }
      } else if (toolCalls.length) {
        cur = newBubble(m.content || '');
        for (const tc of toolCalls) pushTool(cur, tc);
      } else if (m.content) {
        cur = newBubble(m.content);
      }

      // A single plain segment can carry the server-rendered html; multi-segment
      // turns render each segment's markdown from its own text.
      if (bubbles.length === 1 && !bubbles[0].tools?.length && m.html) {
        bubbles[0].html = m.html;
      }

      // Persisted run artifacts (metadata.artifacts, written at chat_complete)
      // re-attach to the turn's LAST bubble so Work cards and inline media survive
      // history reload.
      if (meta?.artifacts?.length && bubbles.length) {
        const workItems = artifactsToWorkItems(meta.artifacts);
        const attachments = artifactsToAttachments(meta.artifacts);
        const last = bubbles[bubbles.length - 1];
        if (workItems.length) last.workItems = workItems;
        if (attachments.length) last.attachments = attachments;
      }

      result.push(...bubbles);
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
  headerRight="Work"
  allAgents={chat.allAgents}
  tokenUsage={chat.tokenUsage}
  quotaWarning={chat.quotaWarning}
  chatError={chat.chatError}
  activityStatus={chat.activityStatus}
  {hasMore}
  {isLoadingMore}
  onloadmore={loadOlderMessages}
  onsend={async (text, files) => {
    if (threadId) {
      sessionStorage.removeItem(pendingSendKey(threadId));
      sessionStorage.removeItem(pendingErrorKey(threadId));
    }
    const attachments = files?.length ? await uploadFiles(files.map(f => f.file)) : undefined;
    chat.send(text, { attachments });
  }}
  onstop={() => chat.stop()}
  onedit={(idx, text) => chat.edit(idx, text)}
  onredo={(idx) => chat.redo(idx)}
  onasksubmit={(id, val) => chat.submitAsk(id, val)}
  onrestoreversion={(docId, v) => chat.restoreVersion(docId, v)}
  ondismisswarning={() => chat.dismissWarning()}
  ondismisserror={() => {
    if (threadId) {
      sessionStorage.removeItem(pendingErrorKey(threadId));
      sessionStorage.removeItem(pendingSendKey(threadId));
    }
    chat.dismissError();
  }}
  isLoading={chat.isLoading}
/>
