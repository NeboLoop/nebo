/**
 * Unified chat controller — ONE way to manage chat state across all surfaces.
 *
 * Handles: WS event subscription, message accumulation, streaming, tool tracking,
 * token usage, quota warnings, ask widgets, and all
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
  /** Stable container id — same across every version of this document. */
  id: string;
  /** Same as id; the document container this version belongs to. */
  documentId: string;
  title: string;
  kind: 'document' | 'code' | 'table' | 'slides';
  /** 1-based version number of this write (legacy artifacts are version 1). */
  version: number;
  url: string;
  /** Source file behind a compiled artifact (e.g. the .jsx behind a .html) —
   *  the viewer offers a Preview/Code toggle instead of two separate items. */
  codeUrl?: string;
}

/** One tool invocation inside an assistant reply's timeline. Tools live ON the
 *  reply they belong to (the message's `tools[]`), never as sibling messages —
 *  so they can't be orphaned or reordered. Mirrors NeboLoop's ToolUse. */
export interface ToolUse {
  toolId?: string;
  name: string;
  status: 'running' | 'success' | 'error';
  request: Record<string, unknown>;
  response: string;
  /** Human activity label (gerund), from the start phase. */
  label?: string;
  /** Past-tense outcome, from the result phase. */
  outcome?: string;
  /** Live sub-step text (e.g. "Initialized sub-agent"). */
  statusText?: string;
  startedAt?: number;
  durationMs?: number;
  /** Structured rendering payload from the backend (ToolResult.payload).
   *  Known kinds render as rich cards (e.g. search_results). */
  payload?: { kind: string; [k: string]: unknown };
  /** Live deep-research panel snapshot (research_progress events) — replaced
   *  whole on every update; the final state comes from the result payload. */
  research?: { kind: string; [k: string]: unknown };
}

export type ChatMessage =
  | { type: 'user'; content: string; time?: string; id?: string; attachments?: UploadedAttachment[] }
  | { type: 'thinking'; content: string; duration: string }
  | { type: 'ask'; requestId: string; prompt: string; widgets: AskWidgetDef[]; response?: string }
  | { type: 'assistant'; content: string; html?: string; time?: string; delegateAgentId?: string; delegateAgentName?: string; id?: string; attachments?: UploadedAttachment[]; workItems?: WorkItem[]; tools?: ToolUse[]; streaming?: boolean };

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

/** Kind by extension. Mirrors the backend's artifact_kind(). */
function kindForExt(ext: string): WorkItem['kind'] {
  if (ext === 'csv' || ext === 'xlsx' || ext === 'xls') return 'table';
  if (ext === 'pptx' || ext === 'ppt') return 'slides';
  if (['js', 'ts', 'jsx', 'tsx', 'py', 'rs', 'go', 'json', 'sh', 'css'].includes(ext)) return 'code';
  return 'document';
}

/** Map run-produced DOCUMENT artifacts to "Work" items (reports/sheets/code → clickable
 *  cards that open + render in the Work panel). Media is excluded (rendered inline).
 *  Each artifact is a versioned object `{ documentId, filename, kind, version, url }`;
 *  a legacy bare string (pre-versioning chats) is tolerated as a single version-1 doc. */
export function artifactsToWorkItems(artifacts: unknown): WorkItem[] {
  if (!Array.isArray(artifacts)) return [];
  // Normalize objects + legacy strings into a single shape.
  const docs = artifacts
    .map((a): WorkItem | null => {
      if (a && typeof a === 'object' && 'documentId' in (a as Record<string, unknown>)) {
        const o = a as Record<string, unknown>;
        const url = String(o.url ?? '');
        if (!url) return null;
        const filename = String(o.filename ?? url.split('/').pop() ?? 'file');
        return {
          id: String(o.documentId),
          documentId: String(o.documentId),
          title: filename,
          kind: (o.kind as WorkItem['kind']) ?? kindForExt(urlExt(url)),
          version: Number(o.version ?? 1),
          url,
        };
      }
      if (typeof a === 'string' && a.length > 0 && !isMedia(a)) {
        const filename = a.split('/').pop() || 'file';
        return { id: a, documentId: a, title: filename, kind: kindForExt(urlExt(a)), version: 1, url: a };
      }
      return null;
    })
    .filter((w): w is WorkItem => w !== null);

  // Pair a compiled .html with its .jsx/.tsx source (same stem): ONE item with a
  // Preview/Code toggle, not two cards for the same deliverable.
  const stem = (f: string) => f.replace(/\.[^.]+$/, '');
  const fileExt = (f: string) => f.split('.').pop()?.toLowerCase() || '';
  const sourceFor = (d: WorkItem) =>
    docs.find((s) => ['jsx', 'tsx'].includes(fileExt(s.title)) && stem(s.title) === stem(d.title));
  const pairedUrls = new Set(
    docs.filter((d) => fileExt(d.title) === 'html').map((d) => sourceFor(d)?.url).filter(Boolean)
  );
  return docs
    .filter((d) => !pairedUrls.has(d.url))
    .map((d) => ({ ...d, codeUrl: fileExt(d.title) === 'html' ? sourceFor(d)?.url : undefined }));
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
  let isLoading = $state(false);
  let tokenUsage = $state<TokenUsage | null>(null);
  let quotaWarning = $state('');
  let chatError = $state('');
  let allAgents = $state<AgentInfo[]>([]);
  let activityStatus = $state('');

  // --- Internal tracking ---
  let phaseStartTime = 0;
  let usageClearTimer: ReturnType<typeof setTimeout> | null = null;
  let activeSessionKey: string | undefined = config.sessionKey;

  // --- In-progress reply tracking ---
  // The streaming reply is a REAL message in `messages` (not an ephemeral overlay),
  // so the tools it runs attach to it directly — they can never become orphaned
  // siblings. `replyId[aid]` is the id of the open reply bubble for an agent
  // (delegates stream under their own id). Mirrors NeboLoop's model.
  let replyId: Record<string, string> = {};
  let idSeq = 0;
  const nextId = () => `msg-${Date.now()}-${++idSeq}`;

  /** Index of `aid`'s open (streaming) reply bubble, or -1. Searches from the end
   *  (the open reply is always near the tail). */
  function replyIndex(aid: string): number {
    const id = replyId[aid];
    if (!id) return -1;
    for (let i = messages.length - 1; i >= 0; i--) {
      const m = messages[i];
      if (m.type === 'assistant' && m.id === id) return m.streaming ? i : -1;
    }
    return -1;
  }

  /** Open a fresh streaming reply bubble for `aid` and return its index. */
  function startReply(aid: string): number {
    const isDelegate = aid !== agentId;
    const delegateAgent = isDelegate ? allAgents.find((a) => a.id === aid) : null;
    const msg: ChatMessage = {
      id: nextId(),
      type: 'assistant',
      content: '',
      time: '',
      streaming: true,
      ...(delegateAgent ? { delegateAgentId: delegateAgent.id, delegateAgentName: delegateAgent.name } : {}),
    };
    messages = [...messages, msg];
    replyId[aid] = msg.id!;
    return messages.length - 1;
  }

  /** Ensure `aid` has an open reply bubble; create one if needed. */
  function ensureReply(aid: string): number {
    const idx = replyIndex(aid);
    return idx === -1 ? startReply(aid) : idx;
  }

  /** Finalize `aid`'s open reply (drop the streaming flag); drop it entirely if
   *  it produced nothing (no text, no tools, no attachments). */
  function finalizeReply(aid: string) {
    const idx = replyIndex(aid);
    delete replyId[aid];
    if (idx === -1) return;
    const m = messages[idx];
    if (m.type !== 'assistant') return;
    const empty = !m.content && !m.tools?.length && !m.attachments?.length && !m.workItems?.length;
    if (empty) {
      messages = messages.filter((_, i) => i !== idx);
    } else {
      messages[idx] = { ...m, streaming: false };
    }
  }

  // --- Fluid streaming: decouple render cadence from bursty network arrival ---
  // Incoming chunks accumulate in pendingStream; a requestAnimationFrame loop drains
  // them into the open reply bubble at a steady character rate (scaling with backlog
  // so it never falls behind), producing a smooth typewriter flow.
  let pendingStream: Record<string, string> = {};
  let rafHandle: number | null = null;

  function appendToReply(aid: string, text: string) {
    if (!text) return;
    const idx = ensureReply(aid);
    const m = messages[idx];
    if (m.type === 'assistant') messages[idx] = { ...m, content: m.content + text };
  }

  function drainPending() {
    rafHandle = null;
    let hasMore = false;
    for (const aid of Object.keys(pendingStream)) {
      const pending = pendingStream[aid];
      if (!pending) { delete pendingStream[aid]; continue; }
      const n = Math.max(2, Math.floor(pending.length / 8));
      appendToReply(aid, pending.slice(0, n));
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

  // Immediately move buffered text into the reply (on completion/reset) so nothing is lost.
  function flushPending(aid?: string) {
    const keys = aid ? [aid] : Object.keys(pendingStream);
    for (const k of keys) {
      if (pendingStream[k]) {
        appendToReply(k, pendingStream[k]);
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
    replyId = {};
    // Drop any abandoned streaming flag (stop/cancel/reset leave the partial reply
    // in place) so a future render gate can't pin a dead bubble as "still live".
    messages = messages.map((m) => (m.type === 'assistant' && m.streaming ? { ...m, streaming: false } : m));
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
    // Narration resuming after this reply already ran tools starts a FRESH bubble,
    // so each segment owns exactly the tools that followed it — the same grouping
    // history rebuilds from contentBlocks, and the way NeboLoop renders a turn.
    const idx = replyIndex(aid);
    if (idx !== -1) {
      const m = messages[idx];
      if (m.type === 'assistant' && m.tools?.length) finalizeReply(aid);
    }
    // Buffer the chunk; the rAF drain renders it smoothly (no spurts).
    pendingStream[aid] = (pendingStream[aid] || '') + chunk;
    schedulePump();
  }

  function handleChatComplete(data: any) {
    if (!isMyEvent(data)) return;
    const aid = data.agentId || agentId;
    // Flush any buffered streamed text into the open reply before finalizing.
    flushPending(aid);
    const attachments = artifactsToAttachments(data.artifacts);
    const workItems = artifactsToWorkItems(data.artifacts);
    // Run artifacts attach to the turn's open reply; if nothing streamed but a run
    // produced files, open a bubble to hold them. Then finalize in place — the
    // streamed text IS the final segment (chat_complete carries no replacement
    // content; re-carrying the whole turn made earlier segments render twice).
    let idx = replyIndex(aid);
    if (idx === -1 && (attachments.length || workItems.length)) idx = startReply(aid);
    if (idx !== -1) {
      const m = messages[idx];
      if (m.type === 'assistant') {
        messages[idx] = {
          ...m,
          time: formatTime(Date.now()),
          ...(attachments.length ? { attachments } : {}),
          ...(workItems.length ? { workItems } : {}),
        };
        const finalText = (messages[idx] as { content: string }).content;
        finalizeReply(aid);
        config.onResponseComplete?.(finalText || '');
      }
    }
    if (aid === agentId) {
      isLoading = false;
      phaseStartTime = 0;
      activityStatus = '';
      if (usageClearTimer) clearTimeout(usageClearTimer);
      usageClearTimer = setTimeout(() => { tokenUsage = null; }, 5000);
    }
  }

  function handleChatMessage(data: any) {
    if (!isMyEvent(data)) return;
    const aid = data.agentId || agentId;
    flushPending(aid);
    const content = data.content || data.text || '';
    const idx = replyIndex(aid);

    // An open streamed reply exists — finalize it IN PLACE (replace with the
    // complete content when provided). Never append a duplicate bubble.
    if (idx !== -1) {
      const m = messages[idx];
      if (m.type === 'assistant') {
        const workItems = artifactsToWorkItems(data.artifacts);
        messages[idx] = {
          ...m,
          ...(content ? { content } : {}),
          ...(data.html ? { html: data.html } : {}),
          time: formatTime(data.createdAt || Date.now()),
          ...(workItems.length ? { workItems } : {}),
        };
      }
      const finalText = messages[idx]?.type === 'assistant' ? (messages[idx] as { content: string }).content : '';
      finalizeReply(aid);
      if (aid === agentId) isLoading = false;
      if (finalText) config.onResponseComplete?.(finalText);
      return;
    }

    // No open reply: a complete (non-streamed) message — append it fresh.
    if (!content) { if (aid === agentId) isLoading = false; return; }
    const isDelegate = aid !== agentId;
    const delegateAgent = isDelegate ? allAgents.find(a => a.id === aid) : null;
    const workItems = artifactsToWorkItems(data.artifacts);
    messages = [...messages, {
      id: data.id || nextId(),
      type: 'assistant' as const,
      content,
      html: data.html || undefined,
      time: formatTime(data.createdAt || Date.now()),
      ...(workItems.length ? { workItems } : {}),
      ...(delegateAgent ? {
        delegateAgentId: delegateAgent.id,
        delegateAgentName: delegateAgent.name,
      } : {}),
    }];
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

    // Flush buffered narration into the open reply, then attach the tool TO that
    // reply's timeline — tools live on the message, never as sibling entries.
    flushPending(aid);
    const idx = ensureReply(aid);

    let request: Record<string, unknown> = {};
    try {
      request = typeof data.input === 'string' ? JSON.parse(data.input) : (data.input || {});
    } catch { /* keep empty */ }
    const m = messages[idx];
    if (m.type === 'assistant') {
      const tool: ToolUse = {
        toolId: data.tool_id,
        // Raw tool name so the display formats the signature (MCP → "slug · tool",
        // STRAP → "name · resource.action"); label + outcome come from the backend.
        name: data.tool || 'tool',
        label: data.label,
        status: 'running',
        request,
        response: '',
        startedAt: Date.now(),
      };
      messages[idx] = { ...m, tools: [...(m.tools ?? []), tool] };
    }
    // Prefer the backend's humanized label so the live indicator and the
    // persisted timeline speak the same vocabulary; static map is the fallback.
    activityStatus = data.label || toolActivityLabel(data.tool || '');
  }

  function handleToolResult(data: any) {
    if (!isMyEvent(data)) return;
    const toolId = data.tool_id as string | undefined;
    const status: ToolUse['status'] = data.is_error ? 'error' : 'success';
    const response = typeof data.result === 'string' ? data.result : JSON.stringify(data.result, null, 2);
    // Locate the matching running tool across reply bubbles — it may live in an
    // earlier, already-finalized segment of this same turn.
    for (let i = messages.length - 1; i >= 0; i--) {
      const m = messages[i];
      if (m.type !== 'assistant' || !m.tools?.length) continue;
      const ti = toolId
        ? m.tools.findIndex((t) => t.toolId === toolId)
        : m.tools.findIndex((t) => t.status === 'running');
      if (ti === -1) continue;
      const tools = [...m.tools];
      const started = tools[ti].startedAt;
      tools[ti] = {
        ...tools[ti],
        status,
        response,
        outcome: data.outcome,
        ...(data.payload && typeof data.payload === 'object' ? { payload: data.payload } : {}),
        durationMs: started ? Date.now() - started : undefined,
      };
      messages[i] = { ...m, tools };
      return;
    }
    // No matching start (replay/recovery): attach as a completed tool on the reply.
    const idx = ensureReply(data.agentId || agentId);
    const m = messages[idx];
    if (m.type === 'assistant') {
      messages[idx] = {
        ...m,
        tools: [...(m.tools ?? []), {
          toolId, name: data.tool_name || 'tool', status, outcome: data.outcome, request: {}, response,
        }],
      };
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

  function handleChatError(data: any) {
    if (!isMyEvent(data)) return;
    isLoading = false;
    resetStreaming();
    phaseStartTime = 0;
    activityStatus = '';
    chatError = data.error || 'Something went wrong.';
  }

  function handleAskRequest(data: any) {
    if (!isMyEvent(data)) return;
    const requestId = data.request_id as string;
    if (!requestId) return;
    messages = [...messages, {
      type: 'ask' as const,
      requestId,
      prompt: data.prompt as string,
      widgets: (data.widgets ?? [{ type: 'options', multiSelect: false, options: ['Yes', 'No'] }]) as AskWidgetDef[],
    }];
  }

  function handleSubagentProgress(data: any) {
    const op = data.current_operation as string | undefined;
    if (!op) return;
    // Update the last running tool's live sub-step text (e.g. "Initialized sub-agent").
    for (let i = messages.length - 1; i >= 0; i--) {
      const m = messages[i];
      if (m.type !== 'assistant' || !m.tools?.length) continue;
      const ti = m.tools.findIndex((t) => t.status === 'running');
      if (ti === -1) continue;
      const tools = [...m.tools];
      tools[ti] = { ...tools[ti], statusText: op };
      messages[i] = { ...m, tools };
      return;
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

  function handleResearchProgress(data: any) {
    if (!isMyEvent(data)) return;
    const snap = data?.data;
    if (!snap || typeof snap !== 'object') return;
    for (let i = messages.length - 1; i >= 0; i--) {
      const m = messages[i];
      if (m.type !== 'assistant' || !m.tools?.length) continue;
      const ti = m.tools.findIndex((t) => t.status === 'running' && t.name === 'agent');
      if (ti === -1) continue;
      const tools = [...m.tools];
      tools[ti] = { ...tools[ti], research: snap };
      messages[i] = { ...m, tools };
      return;
    }
  }
  unsubs.push(ws.on('research_progress', handleResearchProgress));
  unsubs.push(ws.on('usage', handleUsage));
  unsubs.push(ws.on('quota_warning', handleQuotaWarning));
  unsubs.push(ws.on('chat_error', handleChatError));
  unsubs.push(ws.on('ask_request', handleAskRequest));
  unsubs.push(ws.on('subagent_progress', handleSubagentProgress));
  unsubs.push(ws.on('session_reset', handleSessionReset));
  // ghost_text is consumed directly by ChatComposer via ws.on — no window re-dispatch.

  // --- Actions ---

  function send(text: string, options?: SendOptions & { attachments?: UploadedAttachment[] }) {
    chatError = '';
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
    phaseStartTime = Date.now();

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

  function restoreVersion(documentId: string, version: number) {
    ws.send('restore_version', {
      document_id: documentId,
      version,
      agent_id: agentId,
      ...(activeSessionKey ? { session_id: activeSessionKey } : {}),
    });
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

  function dismissError() {
    chatError = '';
  }

  function setError(message: string) {
    isLoading = false;
    resetStreaming();
    phaseStartTime = 0;
    activityStatus = '';
    chatError = message || 'Something went wrong.';
  }

  function destroy() {
    unsubs.forEach(fn => fn());
    if (usageClearTimer) clearTimeout(usageClearTimer);
  }

  // --- Public API ---
  // Getters provide reactive reads; Svelte 5 tracks $state access through them.

  return {
    // The in-progress reply (with its tool timeline) is already a real message in
    // `messages` — no ephemeral overlay to merge, so tools never render detached.
    get messages(): ChatMessage[] { return messages; },
    get isLoading() { return isLoading; },
    set isLoading(v: boolean) { isLoading = v; },
    get tokenUsage() { return tokenUsage; },
    get quotaWarning() { return quotaWarning; },
    get chatError() { return chatError; },
    get activityStatus() { return activityStatus; },
    get allAgents() { return allAgents; },

    send,
    stop,
    newThread,
    submitAsk,
    restoreVersion,
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
        resetStreaming();
      }
    },
    dismissWarning,
    dismissError,
    setError,
    destroy,
  };
}
