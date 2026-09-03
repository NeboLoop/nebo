<script lang="ts">
  import { launchApp } from '$lib/apps/launcher';
  import FlowsPane from '$lib/components/flows/FlowsPane.svelte';
  import { goto } from '$lib/nav';
  import { getContext, onMount, onDestroy } from 'svelte';
  import { sendClientEvent } from '$lib/api/gocliRequest';
  import { t } from 'svelte-i18n';
  import { page } from '$app/stores';
  import { replaceState } from '$app/navigation';
  import ChatPane from '$lib/components/chat/ChatPane.svelte';
  import type { AgentPageContext, EnrichedChat } from '$lib/types/agentPage';
  import { createChatController } from '$lib/chat/controller.svelte';
  import { parseMessages } from '$lib/chat/history';
  import type { ChatMessage } from '$lib/chat/controller.svelte';
  import { toMentionAgent } from '$lib/chat/roster';
  import { threadKey } from '$lib/chat/sessionKey';
  import { formatTime } from '$lib/time';
  import { getWebSocketClient } from '$lib/websocket/client';
  import type { Agent, ChatMessage as ApiChatMessage } from '$lib/api/neboComponents';
  import { uploadFiles } from '$lib/api/upload';

  const PENDING_SEND_PREFIX = 'nebo:pending-send:';
  const PENDING_ERROR_PREFIX = 'nebo:pending-error:';

  type PendingSend = { text: string; sent?: boolean; ts?: number };

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
  const chat = createChatController({ agentId: initialAgentId, sessionKey: threadKey(initialAgentId, initialThreadId) });

  // When navigated from a fresh send, the run is started on THIS page (after
  // subscribe). Settle listeners clear the pending-send stash and strip ?active=1
  // without a SvelteKit goto (goto can remount and drop chat_error / the bubble).
  let activeRunUnsubs: Array<() => void> = [];
  let pendingSendStarted = false;
  let lastThreadId = '';
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
    // SvelteKit's replaceState, not the raw History API — the raw call
    // desyncs the router's own state tracking.
    replaceState(url.pathname + url.search, $page.state);
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
    const sk = threadKey(initialAgentId, initialThreadId);
    if (data.session_id && data.session_id !== sk) return false;
    if (data.agentId && data.agentId !== initialAgentId) return false;
    return true;
  }

  if (startActive) {
    chat.isLoading = true;
    const ws = getWebSocketClient();
    activeRunUnsubs.push(ws.on<{ agentId?: string; session_id?: string; error?: string }>('chat_error', (data) => {
      if (!isFirstRunEvent(data)) return;
      const message = data.error || $t('chat.somethingWentWrong');
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
        chat.setAllAgents((resp.agents).map(toMentionAgent));
      }
    } catch { /* keep empty */ }
  });

  // Voice turns persist server-side as normal chat messages and announce via
  // `voice_message` — reload so the transcript appears in the thread live
  // (and is already in place when the voice overlay closes).
  let voiceMsgUnsub: (() => void) | null = null;
  onMount(() => {
    voiceMsgUnsub = getWebSocketClient().on<{ chatId?: string }>('voice_message', (d) => {
      if (d?.chatId === threadId) loadMessages();
    });
  });

  // The socket came back after a drop (a phone in the background, a tunnel
  // blip): whatever streamed while it was down is gone from the view, since
  // the server keeps no backlog. Reload the thread the way a fresh open does.
  let wsStatusUnsub: (() => void) | null = null;
  onMount(() => {
    const ws = getWebSocketClient();
    let seenDisruptions = ws.getDisruptionCount();
    wsStatusUnsub = ws.onStatus((status) => {
      if (status !== 'connected') return;
      const now = ws.getDisruptionCount();
      if (now === seenDisruptions) return;
      seenDisruptions = now;
      sendClientEvent('thread_resync', { detail: threadId, code: now });
      loadMessages();
    });
  });

  onDestroy(() => {
    for (const off of activeRunUnsubs) off();
    activeRunUnsubs = [];
    voiceMsgUnsub?.();
    wsStatusUnsub?.();
    chat.destroy();
  });

  $effect(() => {
    if (threadId && agentId) {
      // SvelteKit REUSES this component across thread switches. The first-send
      // guard must reset when the user clicks a different chat, or every later
      // switch skips loadMessages() and the transcript freezes on the chat the
      // send happened in.
      if (lastThreadId && threadId !== lastThreadId) pendingSendStarted = false;
      lastThreadId = threadId;
      const sk = threadKey(agentId, threadId);
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
      // The thread is still working: show it now, not at the next event.
      const run = resp.activeRun;
      if (run) {
        chat.isLoading = true;
        chat.activityStatus = run.currentTool ? $t('chat.resumedActivity', { values: { tool: run.currentTool } }) : $t('chat.working');
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

  // ?ask= — a starter prompt from a pane CTA lands in the composer, then the
  // param is cleared so refresh doesn't re-insert it.
  const askPrefill = $derived($page.url.searchParams.get('ask') ?? '');
  function clearAsk() {
    const url = new URL($page.url);
    url.searchParams.delete('ask');
    goto(url.pathname + url.search, { replaceState: true, noScroll: true, keepFocus: true });
  }
</script>

<ChatPane
  messages={chat.messages}
  agentName={agent?.name ?? $t('common.agent')}
  agentId={agentId}
  {threadId}
  onteachsent={(message) => {
    if (!message) return;
    chat.setMessages([...chat.messages, {
      id: 'msg-' + Date.now(),
      type: 'user',
      content: message,
      time: formatTime(Date.now()),
    }]);
    chat.isLoading = true;
  }}
  headerTitle={thread?.name ?? $t('chat.thread')}
  headerRight={$t('chat.work')}
  onopenruns={ctx.openRuns}
  composerPrefill={askPrefill}
  onprefilled={clearAsk}
  onback={ctx.openList}
  onsettings={ctx.openSettings}
  isolated={ctx.agent?.isolated ?? false}
  isApp={ctx.agent?.isApp ?? false}
  onopenapp={() => launchApp(ctx.agentId, ctx.agent?.name ?? 'App')}

  allAgents={chat.allAgents}
  tokenUsage={chat.tokenUsage}
  contextStats={chat.contextStats}
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
    // A failed upload must SAY so — swallowing it here used to eat both the
    // files and the message text with no feedback.
    let attachments;
    if (files?.length) {
      try {
        attachments = await uploadFiles(files.map(f => f.file));
      } catch (e) {
        chat.setError(`File upload failed — message not sent. ${e instanceof Error ? e.message : ''}`.trim());
        return;
      }
    }
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
>
  {#snippet flowsPane()}<FlowsPane onask={ctx.askEmployee} />{/snippet}
</ChatPane>
