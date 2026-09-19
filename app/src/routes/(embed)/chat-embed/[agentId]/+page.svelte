<script lang="ts">
  /**
   * The chat an app frames (SDK `nebo.chat.mount`). ONE surface with the
   * shell's thread page: the same ChatPane, the same controller, the same
   * history door — install and hire cards, ask widgets, the activity panel,
   * attachments, coworker posts all render as they do in the shell. What the
   * shell adds around the pane (sidebar, header, runs, settings) is absent
   * here; what the embed adds (app context, URL options, the app's token) is
   * kept.
   */
  import { onMount, onDestroy, untrack } from 'svelte';
  import { t } from 'svelte-i18n';
  import { storage } from '$lib/storage';
  import { page } from '$app/stores';
  import { goto } from '$lib/nav';
  import ChatPane from '$lib/components/chat/ChatPane.svelte';
  import { getWebSocketClient } from '$lib/websocket/client';
  import { createChatController } from '$lib/chat/controller.svelte';
  import { parseMessages } from '$lib/chat/history';
  import { toMentionAgent } from '$lib/chat/roster';
  import { appKey } from '$lib/chat/sessionKey';
  import { formatTime } from '$lib/time';
  import { uploadFiles } from '$lib/api/upload';
  import type { UploadedAttachment } from '$lib/types/attachment';

  // The embed's agent and session never change without a full reload.
  const agentId = $page.params.agentId ?? '';

  let agentName = $state('');
  let placeholder = $state('');
  let appContext = $state<Record<string, unknown> | null>(null);

  // Read options from URL params
  const urlParams = $derived(new URLSearchParams($page.url.search));
  const paramPlaceholder = $derived(urlParams.get('placeholder') || '');
  const paramTheme = $derived(urlParams.get('theme') || '');
  const paramBorderless = $derived(urlParams.get('borderless') === '1');
  const paramCtx = $page.url.searchParams.get('ctx') || '';
  const paramScope = $page.url.searchParams.get('scope') || '';

  $effect(() => {
    if (paramPlaceholder) placeholder = paramPlaceholder;
  });

  // Apply theme if specified
  $effect(() => {
    if (paramTheme && paramTheme !== 'auto') {
      document.documentElement.setAttribute('data-theme', paramTheme === 'dark' ? 'nebo-dark' : 'nebo');
    }
  });

  const sessionKey = appKey(agentId, paramCtx || undefined);

  const chat = createChatController({
    agentId,
    sessionKey,
    channel: 'app',
    // The app's context rides on every turn — a send, an edit, a redo alike.
    extraPayload: () => {
      const extra: Record<string, unknown> = {};
      if (appContext) {
        extra.context = appContext;
      } else if (paramCtx) {
        extra.context = { displayedDoc: { documentId: paramCtx } };
      }
      if (paramScope) extra.scope = paramScope;
      return extra;
    },
    onResponseComplete: (text) => {
      window.parent?.postMessage({ type: 'nebo:response-complete', text }, '*');
    },
    // A voice, coworker, or workflow turn on this session: reload the rows.
    onTurnLandedElsewhere: () => { void loadMessages(); },
  });

  // Cards the shared pane renders deep-link to shell-owned surfaces by URL
  // param: ?run= (run detail), ?cw= (the employee-to-employee transcript).
  // The app runs in its own window with no bridge back to the Nebo window,
  // so the frame itself takes the shell route (same-frame; the browser's
  // back button returns to this chat). Shell pages the install modal opens
  // (settings, configure) follow the same path on their own.
  const SHELL_PARAMS = ['run', 'cw', 'cwf'];
  $effect(() => {
    const params = $page.url.searchParams;
    if (!SHELL_PARAMS.some((k) => params.has(k))) return;
    const handoff = new URLSearchParams();
    for (const k of SHELL_PARAMS) {
      const v = params.get(k);
      if (v !== null) handoff.set(k, v);
    }
    goto(`/${agentId}/threads?${handoff}`);
  });

  // Slash commands that clear the conversation
  const CLEAR_COMMANDS = ['/new', '/clear'];

  function handleSend(text: string, attachments?: UploadedAttachment[]) {
    const trimmed = text.trim().toLowerCase();
    const isClear = CLEAR_COMMANDS.includes(trimmed);

    if (isClear) chat.clearMessages();

    chat.send(text, { silent: isClear, attachments });
    window.parent?.postMessage({ type: 'nebo:message-sent', message: text }, '*');
  }

  // Pagination state — the server says whether a page older than the oldest
  // loaded message exists.
  let oldestMessageId = $state<string | null>(null);
  let isLoadingMore = $state(false);
  let hasMore = $state(false);
  /** The first fetch is in flight: the pane shows a spinner, not "empty". */
  let historyLoading = $state(false);

  /** Loads the transcript through the thread's door: /chats/{id}/messages
   *  takes the session key and resolves its active chat, carrying the tool
   *  rows (the activity panel), whether a turn is running, and a question the
   *  run is parked on. The rows parse with the same parser the shell uses. */
  async function loadMessages() {
    oldestMessageId = null;
    hasMore = false;
    // Read the transcript without depending on it (a loader effect that
    // tracks what it writes re-fires itself).
    if (untrack(() => chat.messages.length) === 0) historyLoading = true;
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.getChatMessages(sessionKey);
      if (resp?.messages?.length) {
        hasMore = !!resp.hasMore;
        oldestMessageId = resp.messages[0]?.id ?? null;
        chat.setMessages(parseMessages(resp.messages));
      }
      // The session is still working: show it now, not at the next event.
      const run = resp.activeRun;
      if (run) {
        chat.isLoading = true;
        chat.activityStatus = run.currentTool ? $t('chat.resumedActivity', { values: { tool: run.currentTool } }) : $t('chat.working');
      }
      // ...and if that work is parked on a question, the card the live event
      // carried is rendered here too, answerable the same way.
      if (resp.pendingAsk) {
        chat.isLoading = true;
        chat.showPendingAsk(resp.pendingAsk);
      }
    } catch (e) {
      console.warn('[chat-embed] Failed to load messages', e);
      if (chat.messages.length === 0) chat.setError($t('chat.historyLoadFailed'));
    } finally {
      historyLoading = false;
    }
  }

  async function loadOlderMessages() {
    if (!oldestMessageId || isLoadingMore || !hasMore) return;
    isLoadingMore = true;
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.getChatMessages(sessionKey, undefined, oldestMessageId);
      if (resp?.messages?.length) {
        hasMore = !!resp.hasMore;
        oldestMessageId = resp.messages[0]?.id ?? oldestMessageId;
        chat.prependMessages(parseMessages(resp.messages));
      } else {
        hasMore = false;
      }
    } catch (e) {
      console.warn('[chat-embed] Failed to load older messages', e);
      hasMore = false;
    } finally {
      isLoadingMore = false;
    }
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
        chat.setAllAgents((resp.agents).map(toMentionAgent));
      }
    } catch (e) {
      console.warn('[chat-embed] Failed to load agents for @mentions:', e);
    }

    await loadMessages();

    // Connect WebSocket (this page runs outside the shell, so we bootstrap ourselves)
    const ws = getWebSocketClient();
    const token = storage.get('nebo_token');
    ws.connect(token || undefined);

    // The socket came back after a drop: whatever streamed while it was down
    // is gone from the view (the server keeps no backlog). Reload the way a
    // fresh open does.
    let seenDisruptions = ws.getDisruptionCount();
    cleanups.push(ws.onStatus((status) => {
      if (status !== 'connected') return;
      const now = ws.getDisruptionCount();
      if (now === seenDisruptions) return;
      seenDisruptions = now;
      void loadMessages();
    }));

    // Listen for postMessage commands from parent
    function onParentMessage(e: MessageEvent) {
      if (!e.data || typeof e.data.type !== 'string') return;
      switch (e.data.type) {
        case 'nebo:send':
          if (e.data.message) handleSend(e.data.message);
          break;
        case 'nebo:new-thread':
          chat.newThread();
          oldestMessageId = null;
          hasMore = false;
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
    {historyLoading}
    {agentName}
    {agentId}
    sessionId={sessionKey}
    {placeholder}
    allAgents={chat.allAgents}
    activityStatus={chat.activityStatus}
    {hasMore}
    {isLoadingMore}
    onloadmore={loadOlderMessages}
    onsend={async (text, files) => {
      // A failed upload must SAY so, not eat the message (same as threads).
      let attachments;
      if (files?.length) {
        try {
          attachments = await uploadFiles(files.map(f => f.file));
        } catch (e) {
          chat.setError(`File upload failed — message not sent. ${e instanceof Error ? e.message : ''}`.trim());
          return;
        }
      }
      handleSend(text, attachments);
    }}
    onstop={() => chat.stop()}
    onedit={(idx, text) => chat.edit(idx, text)}
    onredo={(idx) => chat.redo(idx)}
    onasksubmit={(id, val) => chat.submitAsk(id, val)}
    onrestoreversion={(docId, v) => chat.restoreVersion(docId, v)}
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
    isLoading={chat.isLoading}
    tokenUsage={chat.tokenUsage}
    contextStats={chat.contextStats}
    quotaWarning={chat.quotaWarning}
    ondismisswarning={() => chat.dismissWarning()}
    chatError={chat.chatError}
    ondismisserror={() => chat.dismissError()}
  />
</div>
