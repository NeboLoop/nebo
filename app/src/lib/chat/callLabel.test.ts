import { describe, expect, it } from 'vitest';
import { baseName, callLine, groupSummary, isCallName, resultLine, type CallTool, type Tr } from './callLabel';
import en from '$lib/i18n/locales/en.json';
import ja from '$lib/i18n/locales/ja.json';

// A stand-in for `$t`: looks the key up in a locale file and fills in
// `{count}` (the ICU plurals here read their `other` branch).
function tr(locale: Record<string, unknown>): Tr {
  return (key, opts) => {
    const msg = key.split('.').reduce<unknown>((o, k) => (o as Record<string, unknown>)?.[k], locale);
    if (typeof msg !== 'string') return key;
    const values = opts?.values ?? {};
    const plural = msg.match(/^\{(\w+), plural,.*other \{(.*)\}\}$/);
    const body = plural ? plural[2].replace(/#/g, String(values[plural[1]])) : msg;
    return body.replace(/\{(\w+)\}/g, (_, k) => String(values[k] ?? ''));
  };
}
const EN = tr(en);
const JA = tr(ja);

const tool = (t: Partial<CallTool>): CallTool => ({ name: 'x', status: 'success', ...t });

describe('callLine', () => {
  it('words the kind and shows the value verbatim', () => {
    const read = tool({ name: 'read_file', call: { kind: 'read', params: { path: '/Users/me/Documents/請求書 2026.xlsx' } } });
    expect(callLine(read, EN)).toEqual({ label: 'Read', subject: '請求書 2026.xlsx', title: '/Users/me/Documents/請求書 2026.xlsx' });
    expect(callLine(read, JA).label).toBe('読み取り');
    const cmd = tool({ call: { kind: 'command', params: { command: 'ls -la\n  ~/Desktop', desc: 'List the desktop' } } });
    expect(callLine(cmd, EN)).toEqual({ label: 'Run', subject: 'ls -la ~/Desktop', title: 'List the desktop · ls -la\n  ~/Desktop' });
    const fetch = tool({ call: { kind: 'fetch', params: { url: 'https://example.com/a' } } });
    expect(callLine(fetch, JA)).toMatchObject({ label: '取得', subject: 'https://example.com/a', href: 'https://example.com/a' });
    expect(callLine(tool({ call: { kind: 'share', params: { count: '3' } } }), EN).subject).toBe('3 files');
  });

  it('words STRAP verbs and names services and tools as they are', () => {
    expect(callLine(tool({ call: { kind: 'action', params: { verb: 'delete', noun: 'reminders' } } }), JA)).toMatchObject({ label: '削除', subject: 'reminders' });
    expect(callLine(tool({ call: { kind: 'action', params: { action: 'frobnicate', noun: 'app' } } }), EN)).toMatchObject({ label: 'frobnicate', subject: 'app' });
    expect(callLine(tool({ call: { kind: 'mcp', params: { service: 'Google Drive', tool: 'list files', target: 'budget' } } }), EN)).toMatchObject({ label: 'Google Drive', subject: 'list files · budget' });
    expect(callLine(tool({ name: 'send_invoice', call: { kind: 'tool', params: { name: 'send invoice', target: 'ACME' } } }), EN)).toMatchObject({ label: 'send invoice', subject: 'ACME' });
  });

  // A row stored before calls had structured names reads by its stored line.
  it('reads an old row by the line it stored', () => {
    expect(callLine(tool({ name: 'search_web', outcome: 'Searched the web', request: { queries: ['vat uk'] } }), EN)).toEqual({ label: 'Searched the web', subject: 'vat uk', title: 'vat uk', href: undefined });
    expect(callLine(tool({ name: 'run_command', outcome: 'Ran `ls`', request: { command: 'ls' } }), EN).subject).toBe('ls');
    expect(callLine(tool({ name: 'mystery' }), EN).label).toBe('mystery');
  });
});

describe('resultLine', () => {
  it('says what a failed call met, in one line', () => {
    expect(resultLine(tool({ status: 'error', response: 'Error: permission denied\nat /x' }), EN)).toBe('permission denied');
    expect(resultLine(tool({ status: 'error', response: '' }), EN)).toBe('Failed');
  });

  it('counts search results and reads the first line of output', () => {
    const payload = { kind: 'search_results', groups: [{ results: [1, 2] }, { results: [3] }] };
    expect(resultLine(tool({ payload }), EN)).toBe('3 results');
    expect(resultLine(tool({ payload }), JA)).toBe('3 件の結果');
    expect(resultLine(tool({ response: '\n{\n  "rows": 212\n}' }), EN)).toBe('"rows": 212');
    expect(resultLine(tool({ status: 'running', statusText: 'Initialized\nhelper' }), EN)).toBe('Initialized helper');
  });
});

describe('groupSummary', () => {
  it('counts calls by kind, failed ones included', () => {
    const read = (status = 'success') => tool({ status, call: { kind: 'read', params: { path: '/a' } } });
    const run = tool({ call: { kind: 'command', params: { command: 'ls' } } });
    expect(groupSummary([read(), read(), read('error'), run], EN)).toBe('Read ×3 · Run');
    expect(groupSummary([read(), run], JA)).toBe('読み取り · 実行');
  });

  it('reads a group of one call as that call', () => {
    expect(groupSummary([tool({ call: { kind: 'command', params: { command: 'ls -la' } } })], EN)).toBe('Run ls -la');
    expect(groupSummary([tool({ status: 'error', call: { kind: 'read', params: { path: '/a/b.txt' } } })], EN)).toBe('Read b.txt');
  });

  it('names the call running now', () => {
    const running = tool({ status: 'running', call: { kind: 'command', params: { command: 'npm test' } } });
    expect(groupSummary([running], EN)).toBe('Run npm test…');
  });
});

describe('helpers', () => {
  it('takes a file name from either kind of path', () => {
    expect(baseName('/a/b/c.txt')).toBe('c.txt');
    expect(baseName('C:\\Users\\me\\d.pdf')).toBe('d.pdf');
    expect(baseName('/a/dir/')).toBe('dir');
  });

  it('accepts only a well-formed call name off the wire', () => {
    expect(isCallName({ kind: 'read', params: { path: '/a' } })).toBe(true);
    expect(isCallName({ kind: 'read' })).toBe(true);
    expect(isCallName({ params: {} })).toBe(false);
    expect(isCallName('read')).toBe(false);
  });
});
