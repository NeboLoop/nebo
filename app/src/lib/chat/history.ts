// History reload for a thread: the persisted rows become the same ChatMessage
// shapes the live controller builds, so a reloaded thread reads like the live
// one (same bubbles, same tool timeline, same outcome words and durations).
import { toolDisplayName, artifactsToWorkItems, artifactsToAttachments } from '$lib/chat/controller.svelte';
import type { ChatMessage } from '$lib/chat/controller.svelte';
import { formatTime } from '$lib/time';
import type { ChatMessage as ApiChatMessage } from '$lib/api/neboComponents';

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


function parseToolInput(input: string | Record<string, unknown> | undefined): Record<string, unknown> {
  if (!input) return {};
  if (typeof input === 'string') {
    try { return JSON.parse(input); } catch { return {}; }
  }
  return input;
}

/** Parse raw API messages into ChatMessage[] for the controller. */
export function parseMessages(rawMessages: ApiChatMessage[]): ChatMessage[] {
  const result: ChatMessage[] = [];
  // Tool results live on the tool-role rows, keyed by tool_call_id; join them
  // up front so history reloads show real Request/Response like live streams.
  const resultsById = new Map<string, string>();
  const payloadsById = new Map<string, { kind: string; [k: string]: unknown }>();
  // The live stream's past-tense outcome ("Ran shell") and duration, persisted
  // with the result so a reloaded thread reads exactly like the live one.
  const outcomesById = new Map<string, string>();
  const durationsById = new Map<string, number>();
  for (const m of rawMessages) {
    if (!m.toolResults) continue;
    try {
      const arr = JSON.parse(m.toolResults);
      if (Array.isArray(arr)) {
        for (const r of arr) {
          if (r?.tool_call_id) {
            resultsById.set(
              r.tool_call_id,
              typeof r.content === 'string' ? r.content : JSON.stringify(r.content, null, 2)
            );
            if (r.payload && typeof r.payload === 'object' && r.payload.kind) {
              payloadsById.set(r.tool_call_id, r.payload);
            }
            if (typeof r.outcome === 'string' && r.outcome) outcomesById.set(r.tool_call_id, r.outcome);
            if (typeof r.duration_ms === 'number') durationsById.set(r.tool_call_id, r.duration_ms);
          }
        }
      }
    } catch {}
  }
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
    // The message-level toolCalls column carries the call ids (metadata
    // toolCalls doesn't) — index-aligned, both persisted in call order.
    let callIds: string[] = [];
    if (m.toolCalls) {
      try {
        const arr = JSON.parse(m.toolCalls);
        if (Array.isArray(arr)) callIds = arr.map((c) => c?.id ?? '');
      } catch {}
    }
    const pushTool = (target: AssistantMsg, tc: ToolCallMeta, callIdx: number) => {
      const request = parseToolInput(tc.input);
      const callId = callIds[callIdx] ?? '';
      (target.tools ??= []).push({
        // Raw name so the display formats the signature. The persisted outcome
        // is the same past-tense line the live stream showed; older rows without
        // one fall back to the static display name.
        name: tc.name || 'tool',
        label: toolDisplayName(tc.name || 'tool', request),
        ...(outcomesById.has(callId) ? { outcome: outcomesById.get(callId) } : {}),
        ...(durationsById.has(callId) ? { durationMs: durationsById.get(callId) } : {}),
        status: tc.status === 'error' ? 'error' : 'success',
        request,
        response: resultsById.get(callId) ?? '',
        ...(payloadsById.has(callId) ? { payload: payloadsById.get(callId) } : {}),
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
          if (tc) { if (!cur) cur = newBubble(''); pushTool(cur, tc, block.toolCallIndex); }
        }
      }
    } else if (toolCalls.length) {
      cur = newBubble(m.content || '');
      toolCalls.forEach((tc, i) => { pushTool(cur!, tc, i); });
    } else if (m.content) {
      cur = newBubble(m.content);
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

