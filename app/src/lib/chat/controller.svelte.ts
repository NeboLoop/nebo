/**
 * Unified chat controller — ONE way to manage chat state across all surfaces.
 *
 * Handles: WS event subscription, message accumulation, streaming, tool tracking,
 * token usage, quota warnings, followup suggestions, ask widgets, and all
 * send/stop/edit/redo actions.
 *
 * Each surface (thread page, embed, web app) creates a controller instance
 * and wires it to ChatPane. Surface-specific logic (routing, history loading,
 * parent postMessage, A2UI) stays in the surface page.
 */

import { getWebSocketClient } from '$lib/websocket/client';
import type { AskWidgetDef } from '$lib/components/chat/AskWidget.svelte';
import type { UploadedAttachment } from '$lib/types/attachment';
import { dispatchInstallStart } from '$lib/marketplace/installCodes';

export interface TokenUsage {
  input: number;
  output: number;
  cacheRead?: number;
  cacheCreation?: number;
  overhead?: number;
}

export interface AgentInfo {
  id: string;
  name: string;
  role: string;
  initial: string;
  status: string;
  color: string;
  isApp?: boolean;
}

/** A produced document/report/sheet/design surfaced in the "Work" panel (click to open). */
export interface WorkItem {
  id: string;
  title: string;
  kind: 'document' | 'code' | 'table' | 'slides';
  url: string;
  /** Source file behind a compiled artifact (e.g. the .jsx behind a .html) —
   *  the viewer offers a Preview/Code toggle instead of two separate items. */
  codeUrl?: string;
}

export type ChatMessage =
  | { type: 'user'; content: string; time?: string; id?: string; attachments?: UploadedAttachment[] }
  | { type: 'thinking'; content: string; duration: string }
  | { type: 'tool'; name: string; status: string; duration: string; request: Record<string, unknown>; response: string; statusText?: string }
  | { type: 'ask'; requestId: string; prompt: string; widgets: AskWidgetDef[]; response?: string }
  | { type: 'assistant'; content: string; html?: string; time?: string; delegateAgentId?: string; delegateAgentName?: string; id?: string; attachments?: UploadedAttachment[]; workItems?: WorkItem[] };

export interface ChatControllerConfig {
  agentId: string;
  /** Explicit session key. When set, events are filtered by session_id.
   *  When absent, events are filtered by agentId/originAgentId. */
  sessionKey?: string;
  /** Channel for outbound messages (e.g., 'app', 'web'). */
  channel?: string;
  /** Called when a response completes — use for embed postMessage, etc. */
  onResponseComplete?: (content: string) => void;
}

export interface SendOptions {
  /** Extra payload fields merged into the WS message. */
  extraPayload?: Record<string, unknown>;
  /** If true, send without adding a user message to the chat. */
  silent?: boolean;
}

/** Build a display-friendly name for a tool call. */
export function toolDisplayName(tool: string, input: Record<string, unknown>): string {
  const resource = input.resource as string | undefined;
  const action = input.action as string | undefined;
  if (tool === 'plugin') {
    const command = input.command as string | undefined;
    const cmdPrefix = command?.split(/[\s+]/)[0];
    if (resource && cmdPrefix) return `${resource}: ${cmdPrefix}`;
    return resource || 'plugin';
  }
  if (tool === 'app' && action && input.app) return `${action} ${input.app}`;
  // Sub-agent spawn: show description or truncated prompt instead of "task: spawn"
  if (tool === 'agent' && resource === 'task' && action === 'spawn') {
    const desc = input.description as string | undefined;
    if (desc) return desc;
    const prompt = input.prompt as string | undefined;
    if (prompt) return prompt.length > 60 ? prompt.slice(0, 57) + '...' : prompt;
    return 'spawning sub-agent';
  }
  if (resource && action) return `${resource}: ${action}`;
  if (resource) return resource;
  if (['event', 'skill'].includes(tool) && action) return action;
  return tool;
}

function toolActivityLabel(toolName: string): string {
  const labels: Record<string, string> = {
    bash:    'running a command',
    grep:    'searching files',
    glob:    'finding files',
    read:    'reading a file',
    write:   'writing a file',
    edit:    'editing a file',

    web:     'searching the web',
    browser: 'reading a page',
    bot:     'thinking it through',
    desktop: 'using the desktop',
    event:   'checking the schedule',
    loop:    'sending a message',

    os:      'checking the workspace',
  };
  return labels[toolName] || 'working';
}

const IMAGE_VIDEO_EXTS = ['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg', 'mp4', 'webm', 'mov'];
const urlExt = (url: string) => (url.split('/').pop() || '').split('.').pop()?.toLowerCase() || '';
const isMedia = (url: string) => IMAGE_VIDEO_EXTS.includes(urlExt(url));

/** Map run-produced media URLs (/api/v1/files/...) to inline attachments (images/video).
 *  Documents go to the Work panel instead — see artifactsToWorkItems. Used for both
 *  live chat_complete events and persisted message metadata on history load. */
export function artifactsToAttachments(artifacts: unknown): UploadedAttachment[] {
  if (!Array.isArray(artifacts)) return [];
  return artifacts
    .filter((u): u is string => typeof u === 'string' && u.length > 0 && isMedia(u))
    .map((url) => {
      const filename = url.split('/').pop() || 'file';
      const ext = urlExt(url);
      const mimeType =
        ({
          png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif',
          webp: 'image/webp', svg: 'image/svg+xml', mp4: 'video/mp4', webm: 'video/webm',
          mov: 'video/quicktime',
        } as Record<string, string>)[ext] || 'application/octet-stream';
      return { fileId: url, filename, mimeType, size: 0, url };
    });
}

/** Map run-produced DOCUMENT URLs to "Work" items (reports/sheets/code → clickable cards
 *  that open + render in the Work panel). Media is excluded (rendered inline instead). */
export function artifactsToWorkItems(artifacts: unknown): WorkItem[] {
  if (!Array.isArray(artifacts)) return [];
  const urls = artifacts.filter(
    (u): u is string => typeof u === 'string' && u.length > 0 && !isMedia(u)
  );
  // Pair a compiled .html with its .jsx/.tsx source (same stem): ONE item with
  // a Preview/Code toggle, not two cards for the same deliverable.
  const stem = (u: string) => (u.split('/').pop() || '').replace(/\.[^.]+$/, '');
  const sourceFor = (u: string) =>
    urls.find((s) => ['jsx', 'tsx'].includes(urlExt(s)) && stem(s) === stem(u));
  const paired = new Set(urls.filter((u) => urlExt(u) === 'html').map(sourceFor).filter(Boolean));
  return urls
    .filter((url) => !paired.has(url))
    .map((url) => {
      const title = url.split('/').pop() || 'file';
      const ext = urlExt(url);
      const kind: WorkItem['kind'] =
        ext === 'csv' || ext === 'xlsx' || ext === 'xls' ? 'table'
          : ext === 'pptx' || ext === 'ppt' ? 'slides'
            : ['js', 'ts', 'jsx', 'tsx', 'py', 'rs', 'go', 'json', 'sh', 'css'].includes(ext) ? 'code'
              : 'document';
      return { id: url, title, kind, url, codeUrl: ext === 'html' ? sourceFor(url) : undefined };
    });
}

/** Format a timestamp for display. */
export function formatTime(ts: string | number): string {
  try {
    const n = typeof ts === 'number' ? ts : Number(ts);
    const date = !isNaN(n) && n > 0
      ? new Date(n < 1e12 ? n * 1000 : n)
      : new Date(String(ts));
    if (isNaN(date.getTime())) return '';
    return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  } catch { return ''; }
}

export function createChatController(config: ChatControllerConfig) {
  const agentId = config.agentId;
  const ws = getWebSocketClient();

  // --- Reactive state ---
  let messages = $state<ChatMessage[]>([]);
  let streamingContent = $state<Record<string, string>>({});
  let isLoading = $state(false);
  let tokenUsage = $state<TokenUsage | null>(null);
  let quotaWarning = $state('');
  let followupSuggestions = $state<string[]>([]);
  let allAgents = $state<AgentInfo[]>([]);
  let activityStatus = $state('');

  // --- Internal tracking ---
  let pendingTools = new Map<string, { idx: number; startTime: number }>();
  let phaseStartTime = 0;
  let usageClearTimer: ReturnType<typeof setTimeout> | null = null;
  let activeSessionKey: string | undefined = config.sessionKey;

  // --- Fluid streaming: decouple render cadence from bursty network arrival ---
  // Incoming chunks accumulate in pendingStream; a requestAnimationFrame loop drains
  // them into the displayed streamingContent at a steady character rate (scaling with
  // backlog so it never falls behind), producing a smooth typewriter flow.
  let pendingStream: Record<string, string> = {};
  let rafHandle: number | null = null;

  function drainPending() {
    rafHandle = null;
    let hasMore = false;
    for (const aid of Object.keys(pendingStream)) {
      const pending = pendingStream[aid];
      if (!pending) { delete pendingStream[aid]; continue; }
      const n = Math.max(2, Math.floor(pending.length / 8));
      streamingContent[aid] = (streamingContent[aid] || '') + pending.slice(0, n);
      const rest = pending.slice(n);
      if (rest) { pendingStream[aid] = rest; hasMore = true; }
      else delete pendingStream[aid];
    }
    if (hasMore) schedulePump();
  }

  function schedulePump() {
    if (rafHandle != null) return;
    if (typeof requestAnimationFrame === 'undefined') { flushPending(); return; }
    rafHandle = requestAnimationFrame(drainPending);
  }

  // Immediately move buffered text to the display (on completion/reset) so nothing is lost.
  function flushPending(aid?: string) {
    const keys = aid ? [aid] : Object.keys(pendingStream);
    for (const k of keys) {
      if (pendingStream[k]) {
        streamingContent[k] = (streamingContent[k] || '') + pendingStream[k];
        delete pendingStream[k];
      }
    }
  }

  function resetStreaming() {
    if (rafHandle != null && typeof cancelAnimationFrame !== 'undefined') {
      cancelAnimationFrame(rafHandle);
    }
    rafHandle = null;
    pendingStream = {};
    streamingContent = {};
  }

  // --- Event filtering ---
  function isMyEvent(data: any): boolean {
    if (activeSessionKey) {
      return !data.session_id || data.session_id === activeSessionKey;
    }
    return data.agentId === agentId || data.originAgentId === agentId;
  }

  // --- Event handlers ---

  function handleChatStream(data: any) {
    if (!isMyEvent(data)) return;
    if (data.done) return;
    const aid = data.agentId || agentId;
    if (aid === agentId && !isLoading) { isLoading = true; phaseStartTime = Date.now(); }
    let chunk = data.chunk || data.content || '';
    // Extract "Working on:" status lines — show as activity indicator, not chat text
    const STATUS_RE = /\n?_Working[^_]*_\n?/g;
    const statusMatch = chunk.match(STATUS_RE);
    if (statusMatch) {
      activityStatus = statusMatch[statusMatch.length - 1].replace(/_/g, '').trim();
      chunk = chunk.replace(STATUS_RE, '');
    }
    if (!chunk) return;
    // Buffer the chunk; the rAF drain renders it smoothly (no spurts).
    pendingStream[aid] = (pendingStream[aid] || '') + chunk;
    schedulePump();
  }

  function handleChatComplete(data: any) {
    if (!isMyEvent(data)) return;
    const aid = data.agentId || agentId;
    // Flush any buffered streamed text before finalizing so nothing is lost.
    flushPending(aid);
    const content = streamingContent[aid];
    const attachments = artifactsToAttachments(data.artifacts);
    const workItems = artifactsToWorkItems(data.artifacts);
    if (content || attachments.length || workItems.length) {
      const isDelegate = aid !== agentId;
      const delegateAgent = isDelegate ? allAgents.find(a => a.id === aid) : null;
      // Finalize in place: the streamed text IS the final segment —
      // chat_complete carries no replacement content (a final payload that
      // re-carried the whole turn made earlier segments render twice).
      messages = [...messages, {
        id: 'msg-' + Date.now(),
        type: 'assistant' as const,
        content: content || '',
        time: formatTime(Date.now()),
        ...(delegateAgent ? {
          delegateAgentId: delegateAgent.id,
          delegateAgentName: delegateAgent.name,
        } : {}),
        ...(attachments.length ? { attachments } : {}),
        ...(workItems.length ? { workItems } : {}),
      }];
      delete streamingContent[aid];
      config.onResponseComplete?.(content || '');
    }
    if (aid === agentId) {
      isLoading = false;
      phaseStartTime = 0;
      activityStatus = '';
      pendingTools.clear();
      if (usageClearTimer) clearTimeout(usageClearTimer);
      usageClearTimer = setTimeout(() => { tokenUsage = null; }, 5000);
    }
  }

  function handleChatMessage(data: any) {
    if (!isMyEvent(data)) return;
    const aid = data.agentId || agentId;
    flushPending(aid);
    const content = data.content || data.text || streamingContent[aid] || '';
    if (!content) return;
    const isDelegate = aid !== agentId;
    const delegateAgent = isDelegate ? allAgents.find(a => a.id === aid) : null;
    messages = [...messages, {
      id: data.id || 'msg-' + Date.now(),
      type: 'assistant' as const,
      content,
      html: data.html || undefined,
      time: formatTime(data.createdAt || Date.now()),
      ...(delegateAgent ? {
        delegateAgentId: delegateAgent.id,
        delegateAgentName: delegateAgent.name,
      } : {}),
    }];
    delete streamingContent[aid];
    if (aid === agentId) isLoading = false;
    config.onResponseComplete?.(content);
  }

  function handleThinking(data: any) {
    if (!isMyEvent(data)) return;
    const aid = data.agentId || agentId;
    if (aid === agentId && !isLoading) { isLoading = true; phaseStartTime = Date.now(); }
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

  function handleToolStart(data: any) {
    if (!isMyEvent(data)) return;
    const aid = data.agentId || agentId;
    if (aid === agentId && !isLoading) { isLoading = true; phaseStartTime = Date.now(); }

    // Commit pending streaming text before the tool so tool groups from
    // different agentic loop turns are separated by assistant text.
    flushPending(aid);
    const pendingText = streamingContent[aid];
    if (pendingText) {
      const isDelegate = aid !== agentId;
      const delegateAgent = isDelegate ? allAgents.find(a => a.id === aid) : null;
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
      delete streamingContent[aid];
    }

    let request: Record<string, unknown> = {};
    try {
      request = typeof data.input === 'string' ? JSON.parse(data.input) : (data.input || {});
    } catch { /* keep empty */ }
    const idx = messages.length;
    messages = [...messages, {
      type: 'tool' as const,
      name: toolDisplayName(data.tool || 'tool', request),
      status: 'running',
      duration: '...',
      request,
      response: '',
    }];
    if (data.tool_id) {
      pendingTools.set(data.tool_id, { idx, startTime: Date.now() });
    }
    activityStatus = toolActivityLabel(data.tool || '');
  }

  function handleToolResult(data: any) {
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
        status: data.is_error ? 'error' : 'success',
        duration,
        response: typeof data.result === 'string' ? data.result : JSON.stringify(data.result, null, 2),
      } as ChatMessage;
      messages = updated;
      pendingTools.delete(data.tool_id);
    } else {
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

  function handleFollowupSuggestions(data: any) {
    if (!isMyEvent(data)) return;
    if (Array.isArray(data.suggestions)) {
      followupSuggestions = data.suggestions;
    }
  }

  function handleUsage(data: any) {
    if (!isMyEvent(data)) return;
    tokenUsage = {
      input: data.input_tokens || 0,
      output: data.output_tokens || 0,
      cacheRead: data.cache_read_input_tokens || 0,
      cacheCreation: data.cache_creation_input_tokens || 0,
      overhead: data.overhead_tokens || 0,
    };
    if (usageClearTimer) clearTimeout(usageClearTimer);
  }

  function handleQuotaWarning(data: any) {
    if (!isMyEvent(data)) return;
    quotaWarning = data.message || data.text || '';
  }

  function handleAskRequest(data: any) {
    if (!isMyEvent(data)) return;
    const requestId = data.request_id as string;
    if (!requestId) return;
    messages = [...messages, {
      type: 'ask' as const,
      requestId,
      prompt: data.prompt as string,
      widgets: (data.widgets ?? [{ type: 'confirm', options: ['Yes', 'No'] }]) as AskWidgetDef[],
    }];
  }

  function handleSubagentProgress(data: any) {
    const op = data.current_operation as string | undefined;
    if (!op) return;
    // Find the last running tool that is a spawn (sub-agent) and update its statusText
    const updated = [...messages];
    for (let i = updated.length - 1; i >= 0; i--) {
      const msg = updated[i];
      if (msg.type === 'tool' && msg.status === 'running') {
        updated[i] = { ...msg, statusText: op } as ChatMessage;
        messages = updated;
        return;
      }
    }
  }

  function handleSessionReset(data: any) {
    if (!isMyEvent(data)) return;
    if (data.success) {
      messages = [];
      resetStreaming();
    }
  }

  function handleChatCancelled(data: any) {
    if (!isMyEvent(data)) return;
    isLoading = false;
    resetStreaming();
    phaseStartTime = 0;
    activityStatus = '';
  }

  // --- Subscribe to WS events ---
  const unsubs: (() => void)[] = [];
  unsubs.push(ws.on('chat_stream', handleChatStream));
  unsubs.push(ws.on('chat_complete', handleChatComplete));
  unsubs.push(ws.on('chat_message', handleChatMessage));
  unsubs.push(ws.on('chat_cancelled', handleChatCancelled));
  unsubs.push(ws.on('thinking', handleThinking));
  unsubs.push(ws.on('tool_start', handleToolStart));
  unsubs.push(ws.on('tool_result', handleToolResult));
  unsubs.push(ws.on('followup_suggestions', handleFollowupSuggestions));
  unsubs.push(ws.on('usage', handleUsage));
  unsubs.push(ws.on('quota_warning', handleQuotaWarning));
  unsubs.push(ws.on('ask_request', handleAskRequest));
  unsubs.push(ws.on('subagent_progress', handleSubagentProgress));
  unsubs.push(ws.on('session_reset', handleSessionReset));
  unsubs.push(ws.on('ghost_text', (data: any) => {
    if (typeof window !== 'undefined') {
      window.dispatchEvent(new CustomEvent('nebo:ghost_text', { detail: data }));
    }
  }));

  // --- Actions ---

  function send(text: string, options?: SendOptions & { attachments?: UploadedAttachment[] }) {
    if (!options?.silent) {
      messages = [...messages, {
        id: 'msg-' + Date.now(),
        type: 'user' as const,
        content: text,
        time: formatTime(Date.now()),
        ...(options?.attachments?.length ? { attachments: options.attachments } : {}),
      }];
    }

    // Marketplace install code: the install modal owns all feedback. Open it
    // immediately and send the code to the backend, but DON'T engage the chat
    // "working" spinner — no agent reply streams back, so the spinner would hang
    // for the entire install and never clear.
    if (dispatchInstallStart(text)) {
      ws.send('chat', { prompt: text.trim(), agent_id: agentId, ...(activeSessionKey ? { session_id: activeSessionKey } : {}) });
      return;
    }

    isLoading = true;
    followupSuggestions = [];
    phaseStartTime = Date.now();
    pendingTools.clear();

    const payload: Record<string, unknown> = {
      prompt: text,
      agent_id: agentId,
      ...(options?.extraPayload || {}),
    };
    if (activeSessionKey) payload.session_id = activeSessionKey;
    if (config.channel) payload.channel = config.channel;
    if (options?.attachments?.length) payload.attachments = options.attachments;
    ws.send('chat', payload);
  }

  function stop() {
    const payload: Record<string, unknown> = {};
    if (activeSessionKey) payload.session_id = activeSessionKey;
    else payload.agent_id = agentId;
    ws.send('cancel', payload);
    isLoading = false;
    resetStreaming();
    phaseStartTime = 0;
  }

  function newThread() {
    messages = [];
    resetStreaming();
    isLoading = false;
    if (config.sessionKey) {
      ws.send('rotate_chat', { session_id: config.sessionKey });
    }
  }

  function submitAsk(requestId: string, value: string) {
    ws.send('ask_response', { request_id: requestId, value });
    messages = messages.map(msg =>
      msg.type === 'ask' && msg.requestId === requestId
        ? { ...msg, response: value }
        : msg
    );
  }

  function edit(msgIndex: number, newContent: string) {
    messages = messages.slice(0, msgIndex);
    send(newContent);
  }

  function redo(msgIndex: number) {
    let userContent = '';
    for (let i = msgIndex - 1; i >= 0; i--) {
      if (messages[i]?.type === 'user') {
        userContent = (messages[i] as { content: string }).content;
        break;
      }
    }
    if (!userContent) return;
    messages = messages.slice(0, msgIndex);
    send(userContent);
  }

  function prependMessages(msgs: ChatMessage[]) {
    messages = [...msgs, ...messages];
  }

  function clearMessages() {
    messages = [];
    resetStreaming();
  }

  function setMessages(msgs: ChatMessage[]) {
    messages = msgs;
  }

  function setAllAgents(agents: AgentInfo[]) {
    allAgents = agents;
  }

  function dismissWarning() {
    quotaWarning = '';
  }

  function destroy() {
    unsubs.forEach(fn => fn());
    if (usageClearTimer) clearTimeout(usageClearTimer);
  }

  // --- Public API ---
  // Getters provide reactive reads; Svelte 5 tracks $state access through them.

  return {
    get messages(): ChatMessage[] {
      // Merge committed messages with in-progress streaming entries
      const extra: ChatMessage[] = [];
      for (const [aid, content] of Object.entries(streamingContent)) {
        if (content) {
          const isDelegate = aid !== agentId;
          const delegateAgent = isDelegate ? allAgents.find(a => a.id === aid) : null;
          extra.push({
            id: `streaming-${aid}`,
            type: 'assistant' as const,
            content,
            time: '',
            ...(delegateAgent ? {
              delegateAgentId: delegateAgent.id,
              delegateAgentName: delegateAgent.name,
            } : {}),
          });
        }
      }
      return [...messages, ...extra];
    },
    get isLoading() { return isLoading; },
    set isLoading(v: boolean) { isLoading = v; },
    get tokenUsage() { return tokenUsage; },
    get quotaWarning() { return quotaWarning; },
    get followupSuggestions() { return followupSuggestions; },
    get activityStatus() { return activityStatus; },
    get allAgents() { return allAgents; },

    send,
    stop,
    newThread,
    submitAsk,
    edit,
    redo,
    clearMessages,
    setMessages,
    prependMessages,
    setAllAgents,
    setSessionKey(key: string) {
      if (key !== activeSessionKey) {
        activeSessionKey = key;
        isLoading = false;
        activityStatus = '';
        pendingTools.clear();
      }
    },
    dismissWarning,
    dismissFollowups() { followupSuggestions = []; },
    destroy,
  };
}
