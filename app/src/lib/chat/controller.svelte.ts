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
import { sendInstallCode } from '$lib/marketplace/installCodes';
import { formatTime } from '$lib/time';
import { get } from 'svelte/store';
import { t } from 'svelte-i18n';

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
  | { type: 'user'; content: string; time?: string; id?: string; attachments?: UploadedAttachment[]; pending?: boolean }
  | { type: 'thinking'; content: string; duration: string }
  | { type: 'ask'; requestId: string; prompt: string; widgets: AskWidgetDef[]; response?: string; cancelled?: boolean }
  | { type: 'assistant'; content: string; time?: string; delegateAgentId?: string; delegateAgentName?: string; id?: string; attachments?: UploadedAttachment[]; workItems?: WorkItem[]; tools?: ToolUse[]; streaming?: boolean };

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
      // fileId stays empty: these are LOCAL run outputs served straight from
      // /api/v1/files — a non-empty fileId routes through the comm-files
      // proxy (for loop uploads) and double-prefixes the URL into a 404.
      return { fileId: '', filename, mimeType, size: 0, url };
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
    // The queued message's own completion: the first turn is still running.
    if (data.stop_reason === QUEUED_INTO_RUNNING_TURN) return;
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
      // The turn a queued message waited on is over: it is in the thread now.
      messages = messages.map((m) => (m.type === 'user' && m.pending ? { ...m, pending: false } : m));
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
        durationMs: typeof data.duration_ms === 'number' ? data.duration_ms : (started ? Date.now() - started : undefined),
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

  // Where the session's tokens went (context_stats, one per turn).
  let contextStats = $state<{ files: number; filesReread: number; redundantReads: number; compactionPasses: number; evictions: number; spilledResults: number } | null>(null);
  function handleContextStats(data: any) {
    if (!isMyEvent(data)) return;
    contextStats = {
      files: data.files || 0,
      filesReread: data.files_reread || 0,
      redundantReads: data.redundant_reads || 0,
      compactionPasses: data.compaction_passes || 0,
      evictions: data.evictions || 0,
      spilledResults: data.spilled_results || 0,
    };
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

  // Sent while the employee was still working: the server appended it to the
  // thread for the running turn's next step. The message itself shows as
  // pending (italic) until that turn completes; the employee says nothing
  // about it, and the spinner stays because the first turn is still running.
  const QUEUED_INTO_RUNNING_TURN = 'queued_into_running_turn';
  function setLastUserPending(pending: boolean) {
    for (let i = messages.length - 1; i >= 0; i--) {
      const m = messages[i];
      if (m.type === 'user') {
        if (!!m.pending === pending) return;
        messages[i] = { ...m, pending };
        messages = [...messages];
        return;
      }
    }
  }

  function handleChatError(data: any) {
    if (!isMyEvent(data)) return;
    if (data.stop_reason === QUEUED_INTO_RUNNING_TURN) {
      setLastUserPending(true);
      return;
    }
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
    // The same question can reach the page twice: the live event, the thread's
    // history response, and a reconnect replay all carry it. One card.
    if (messages.some((m) => m.type === 'ask' && m.requestId === requestId)) return;
    messages = [...messages, {
      type: 'ask' as const,
      requestId,
      prompt: data.prompt as string,
      widgets: (data.widgets ?? [{ type: 'options', multiSelect: false, options: ['Yes', 'No'] }]) as AskWidgetDef[],
    }];
    // The run is parked on the owner: the last tool's activity line ("browsing
    // the marketplace") would otherwise sit under the card as if still running.
    activityStatus = get(t)('chat.waitingForYou');
  }

  /** A question the run was already parked on when this thread opened (the
   *  history response's `pendingAsk`): rendered exactly like the live event. */
  function showPendingAsk(ask: { requestId: string; prompt: string; widgets?: unknown }) {
    handleAskRequest({
      session_id: activeSessionKey,
      agentId,
      request_id: ask.requestId,
      prompt: ask.prompt,
      widgets: ask.widgets,
    });
  }

  function handleSubagentProgress(data: any) {
    if (!isMyEvent(data)) return;
    const op = data.current_operation as string | undefined;
    if (!op) return;
    // The delegate's current step IS this turn's live status: without it a
    // long delegated job reads as "Still working: agent" for minutes, and a
    // page that was reloaded mid-run has no running tool row to annotate.
    const steps = Number(data.tool_count) || 0;
    activityStatus = steps > 0 ? get(t)('chat.subagentStep', { values: { op, n: steps } }) : op;
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
    // A question nobody answered dies with the run: the card says so instead
    // of offering choices the tool will never read.
    messages = messages.map((m) =>
      m.type === 'ask' && m.response == null && !m.cancelled ? { ...m, cancelled: true } : m
    );
  }

  // --- Subscribe to WS events ---
  const unsubs: (() => void)[] = [];
  // A chat frame can pass the `readyState === OPEN` check and still be discarded
  // when that socket closes milliseconds later: no throw, and the wire protocol has
  // no ack to catch it. Without this the spinner and the stop button run forever
  // against a server that never received the message (observed on cloud bots,
  // 2026-08-26). ponytail: surface the failure rather than auto-resending, because
  // replaying an unacked chat risks a duplicate turn, worse than an honest error.
  const DELIVERY_TIMEOUT_MS = 30_000;
  let deliveryTimer: ReturnType<typeof setTimeout> | null = null;
  let sentAtDisruptions = 0;

  function clearDeliveryTimer() {
    if (deliveryTimer) {
      clearTimeout(deliveryTimer);
      deliveryTimer = null;
    }
  }

  /** Register a server handler. ANY inbound event proves the last send landed. */
  function onServer(type: string, fn: (data: any) => void) {
    return ws.on(type, (data: any) => {
      clearDeliveryTimer();
      fn(data);
    });
  }

  unsubs.push(onServer('chat_stream', handleChatStream));
  unsubs.push(onServer('chat_complete', handleChatComplete));
  unsubs.push(onServer('chat_message', handleChatMessage));
  unsubs.push(onServer('chat_cancelled', handleChatCancelled));
  unsubs.push(onServer('thinking', handleThinking));
  unsubs.push(onServer('tool_start', handleToolStart));
  unsubs.push(onServer('tool_result', handleToolResult));

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
  unsubs.push(onServer('research_progress', handleResearchProgress));
  unsubs.push(onServer('usage', handleUsage));
  unsubs.push(onServer('context_stats', handleContextStats));
  unsubs.push(onServer('quota_warning', handleQuotaWarning));
  unsubs.push(onServer('chat_error', handleChatError));
  unsubs.push(onServer('ask_request', handleAskRequest));
  unsubs.push(onServer('subagent_progress', handleSubagentProgress));
  unsubs.push(onServer('session_reset', handleSessionReset));

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

    // Marketplace install code: sendInstallCode opens the modal (which owns all
    // feedback) and delivers the code; DON'T engage the chat "working" spinner —
    // no agent reply streams back, so the spinner would hang and never clear.
    if (sendInstallCode(text, agentId, activeSessionKey)) return;

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

    clearDeliveryTimer();
    sentAtDisruptions = ws.getDisruptionCount?.() ?? 0;
    armDeliveryTimer();
  }

  // Silence alone is not failure: a long tool run (a subagent, a big build)
  // legitimately streams nothing for minutes. Only a socket that BROKE after
  // the send can have discarded the frame — queued frames survive and flush
  // on reconnect. So on timeout: broke since send → honest error; still
  // quiet on a healthy socket → keep waiting (live false alarm 2026-09-01:
  // "connection dropped" toasts over a healthy "using agent…" run).
  function armDeliveryTimer() {
    deliveryTimer = setTimeout(() => {
      deliveryTimer = null;
      if (!isLoading) return;
      const broke = (ws.getDisruptionCount?.() ?? 0) !== sentAtDisruptions;
      if (broke) {
        setError('Message not delivered. The connection dropped, so send it again.');
      } else {
        armDeliveryTimer();
      }
    }, DELIVERY_TIMEOUT_MS);
  }

  function stop() {
    const payload: Record<string, unknown> = {};
    if (activeSessionKey) payload.session_id = activeSessionKey;
    else payload.agent_id = agentId;
    ws.send('cancel', payload);
    clearDeliveryTimer();
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
    // Answered: the run is working again; its next tool_start names what it does.
    activityStatus = '';
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
    clearDeliveryTimer();
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
    get contextStats() { return contextStats; },
    get quotaWarning() { return quotaWarning; },
    get chatError() { return chatError; },
    get activityStatus() { return activityStatus; },
    set activityStatus(v: string) { activityStatus = v; },
    get allAgents() { return allAgents; },

    send,
    stop,
    newThread,
    submitAsk,
    showPendingAsk,
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
        clearDeliveryTimer();
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
