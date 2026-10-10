// How a tool call reads in the thread: one line of a kind worded in the
// owner's language (`chat.call.*`) and the call's own value — a path, a
// command, a query, a URL — shown verbatim; and under it, one line of what
// came back. The server names each call `{kind, params}`
// (`tools::humanize::call`); a call without one (from a server before it)
// reads by the line it stored, as it always did.

import { stepMeta } from './stepMeta';

export interface CallName {
  kind: string;
  params?: Record<string, string>;
}

/** Whether a value off the wire is a call's structured name. */
export function isCallName(v: unknown): v is CallName {
  if (!v || typeof v !== 'object') return false;
  const c = v as { kind?: unknown; params?: unknown };
  return typeof c.kind === 'string' && (c.params === undefined || (typeof c.params === 'object' && c.params !== null));
}

export interface CallTool {
  name: string;
  status: string;
  request?: Record<string, unknown>;
  response?: string;
  label?: string;
  outcome?: string;
  statusText?: string;
  call?: CallName;
  payload?: { kind: string; [k: string]: unknown };
}

/** A translator: `$t` from svelte-i18n, or a stand-in in tests. */
export type Tr = (key: string, opts?: { values?: Record<string, string | number | boolean | Date | null | undefined> }) => string;

export interface CallLine {
  /** The kind, worded ("Read", "Run", "Search"). */
  label: string;
  /** The call's own value, verbatim ("notes.md", "ls -la"). */
  subject: string;
  /** Everything the row stands for, for its tooltip. */
  title: string;
  /** The page the call opened, as a link. */
  href?: string;
}

/** Kinds worded as `chat.call.<kind>` with the value in `path`. */
const FILE_KINDS = new Set(['read', 'write', 'edit', 'plan', 'convert']);
/** Kinds worded as `chat.call.<kind>`. */
const WORDED = new Set([...FILE_KINDS, 'share', 'command', 'search', 'fetch', 'request', 'browser', 'research', 'helper', 'task']);
/** STRAP verbs worded as `chat.callVerb.<verb>`. */
const VERBS = new Set(['create', 'read', 'list', 'search', 'update', 'delete', 'send', 'run', 'write', 'download', 'upload', 'open', 'stop', 'check', 'notify']);

/** The last part of a path: what a reader knows a file by. */
export function baseName(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, '');
  const i = Math.max(trimmed.lastIndexOf('/'), trimmed.lastIndexOf('\\'));
  return i >= 0 ? trimmed.slice(i + 1) || trimmed : trimmed;
}

const oneLine = (s: string) => s.replace(/\s+/g, ' ').trim();
const joined = (...parts: (string | undefined)[]) => parts.filter((p) => p && p.trim()).join(' · ');
const spaced = (...parts: (string | undefined)[]) => parts.filter((p) => p && p.trim()).join(' ');

/** A call's one line. */
export function callLine(tool: CallTool, tr: Tr): CallLine {
  const call = tool.call;
  const p = call?.params ?? {};
  const kind = call?.kind ?? '';
  const http = (u: string | undefined) => (u && /^https?:\/\//i.test(u) ? u : undefined);
  if (call && WORDED.has(kind)) {
    const label = tr(`chat.call.${kind}`);
    if (FILE_KINDS.has(kind) || (kind === 'share' && p.path)) {
      const path = p.path ?? '';
      return { label, subject: baseName(path), title: path };
    }
    switch (kind) {
      case 'share':
        return { label, subject: tr('chat.call.fileCount', { values: { count: Number(p.count ?? 0) } }), title: '' };
      case 'command': {
        const command = oneLine(p.command ?? '');
        return { label, subject: command, title: joined(p.desc, p.command) };
      }
      case 'search':
      case 'research':
        return { label, subject: p.query ?? '', title: p.query ?? '' };
      case 'fetch':
        return { label, subject: p.url ?? '', title: p.url ?? '', href: http(p.url) };
      case 'request': {
        const subject = spaced(p.method, p.url);
        return { label, subject, title: subject };
      }
      case 'browser': {
        const subject = p.url ?? (p.step ?? '').replace(/_/g, ' ');
        return { label, subject, title: subject, href: http(p.url) };
      }
      case 'helper':
        return { label, subject: p.desc ?? '', title: p.desc ?? '' };
      case 'task':
        return { label, subject: p.subject ?? '', title: p.subject ?? '' };
    }
  }
  if (call && kind === 'mcp') {
    const subject = joined(p.tool, p.target);
    return { label: p.service ?? tool.name, subject, title: joined(p.service, subject) };
  }
  if (call && kind === 'action') {
    const label = p.verb && VERBS.has(p.verb) ? tr(`chat.callVerb.${p.verb}`) : (p.action ?? '');
    const subject = joined(p.noun, p.target);
    return { label: label || tool.name, subject, title: subject };
  }
  if (call && kind === 'tool') {
    return { label: p.name ?? tool.name, subject: p.target ?? '', title: p.target ?? '' };
  }
  // A call stored without a structured name: the line the server stored for
  // it, and what it searched for or opened.
  const label = (tool.status === 'running' ? tool.label : tool.outcome ?? tool.label) || tool.name;
  const command = typeof tool.request?.command === 'string' ? oneLine(tool.request.command) : '';
  const meta = stepMeta(tool.request);
  const subject = meta?.text ?? command;
  return { label: oneLine(label), subject, title: subject, href: meta?.href };
}

/** The first line of a text that says something, at most `max` characters. */
function firstLine(text: string, max = 300): string {
  const line = text.split('\n').map((l) => l.trim()).find((l) => l && !/^[{}[\],]+$/.test(l)) ?? '';
  return line.length > max ? `${line.slice(0, max)}…` : line;
}

/** What came back, in one line: the error a failed call met, how many
 *  results a search found, the first line of the output. Empty while the
 *  call runs with nothing to say, or when it returned nothing. */
export function resultLine(tool: CallTool, tr: Tr): string {
  if (tool.status === 'running') return oneLine(tool.statusText ?? '');
  const response = tool.response ?? '';
  if (tool.status === 'error') return firstLine(response.replace(/^\s*error:\s*/i, '')) || tr('common.failed');
  const groups = tool.payload?.kind === 'search_results' ? (tool.payload.groups as { results?: unknown[] }[] | undefined) : undefined;
  if (Array.isArray(groups)) {
    const count = groups.reduce((n, g) => n + (Array.isArray(g.results) ? g.results.length : 0), 0);
    return tr('chat.call.resultCount', { values: { count } });
  }
  return firstLine(response);
}

/** A group's summary: each kind with how many calls it made, the first
 *  three ("Read ×3 · Run · Search") and how many more. The call running now
 *  reads instead, as its own line, and so does a group of one call ("Run
 *  ls -la"). How many failed is said beside it (`chat.call.failedCount`), so
 *  the cut never hides it. */
export function groupSummary(tools: CallTool[], tr: Tr): string {
  const running = tools.filter((t) => t.status === 'running');
  if (running.length) {
    const line = callLine(running[running.length - 1], tr);
    return `${spaced(line.label, line.subject)}…`;
  }
  if (tools.length === 1) {
    const line = callLine(tools[0], tr);
    return spaced(line.label, line.subject);
  }
  const counts = new Map<string, number>();
  for (const t of tools) {
    const label = callLine(t, tr).label;
    counts.set(label, (counts.get(label) ?? 0) + 1);
  }
  const parts = [...counts.entries()].map(([label, n]) => (n > 1 ? `${label} ×${n}` : label));
  const line = parts.slice(0, 3).join(' · ');
  return parts.length > 3 ? `${line} ${tr('chat.moreCount', { values: { count: parts.length - 3 } })}` : line;
}
