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

  // A command that watches the owner says so, whatever the model wrote.
  it('says what of the owner a command touches', () => {
    const shot = tool({ call: { kind: 'command', params: { command: 'screencapture -x a.png', desc: 'List files', reach: 'screen' } } });
    expect(callLine(shot, EN)).toMatchObject({ label: 'Capturing your screen', subject: 'screencapture -x a.png' });
    expect(callLine(shot, JA).label).toBe('画面をキャプチャ中');
    const app = (a?: string) => tool({ call: { kind: 'command', params: { command: 'osascript x', reach: 'app', ...(a ? { app: a } : {}) } } });
    expect(callLine(app('Safari'), EN).label).toBe('Controlling Safari');
    expect(callLine(app(), EN).label).toBe('Controlling your apps');
  });

  it('names a click and a look at the screen by what they touched', () => {
    const click = tool({ call: { kind: 'computer', params: { step: 'double_click', target: '(223,900)', app: 'Simulator' } } });
    expect(callLine(click, EN)).toMatchObject({ label: 'Computer', subject: 'double click · (223,900) · Simulator' });
  });

  it('reads the line the model wrote for a plugin call', () => {
    const send = tool({ name: 'quickbooks', call: { kind: 'tool', params: { name: 'quickbooks', target: 'invoice send', desc: 'Send invoice 1041' } } });
    expect(callLine(send, EN)).toMatchObject({ label: 'quickbooks', subject: 'Send invoice 1041' });
    expect(callLine(tool({ name: 'mystery_tool' }), EN).label).toBe('mystery tool');
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

// Live 2026-10-11: the owner read "working with ask_owner…" over the
// question his employee was asking him. A question reads as one: waiting
// on his answer while it runs, then what was asked and what he answered.
describe('a question to the owner', () => {
  const q = 'Are you flying from Salt Lake City, and on what dates?';
  const ask = (t: Partial<CallTool>) => tool({ name: 'ask_owner', call: { kind: 'ask', params: { question: q } }, ...t });

  it('waits on his answer while it runs, never naming the tool', () => {
    const running = ask({ status: 'running' });
    expect(callLine(running, EN)).toMatchObject({ label: 'Waiting for your answer', subject: '' });
    expect(groupSummary([running], EN)).toBe('Waiting for your answer…');
    expect(callLine(running, JA).label).not.toBe(callLine(running, EN).label);
    for (const t of [EN, JA]) expect(groupSummary([running], t)).not.toMatch(/ask[_ ]owner/);
  });

  it('once answered, reads as what was asked and what he said', () => {
    expect(callLine(ask({ response: JSON.stringify({ response: 'Yes, Oct 20 to 25', note: 'x' }) }), EN)).toMatchObject({ label: 'Asked', subject: q });
    expect(resultLine(ask({ response: JSON.stringify({ response: 'Yes, Oct 20 to 25', note: 'x' }) }), EN)).toBe('Yes, Oct 20 to 25');
    expect(resultLine(ask({ response: JSON.stringify({ picked: 'Marketing' }) }), EN)).toBe('Marketing');
    expect(resultLine(ask({ response: JSON.stringify({ picked: ['Mon', 'Wed'] }) }), EN)).toBe('Mon, Wed');
    expect(resultLine(ask({ response: JSON.stringify({ own_words: 'k', note: 'x' }) }), EN)).toBe('k');
    expect(resultLine(ask({ response: 'The owner skipped this question. Make a reasonable assumption.' }), EN)).toBe('Skipped');
  });
});
