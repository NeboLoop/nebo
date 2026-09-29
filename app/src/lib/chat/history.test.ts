import { describe, expect, it } from 'vitest';
import { parseMessages, lastRunError } from './history';

// A tool row carries the outcome and duration the live stream showed; the
// reloaded timeline must read the same ("Ran shell · <1s"), not fall back to
// the static "shell: exec" name. Older rows without them still map.
describe('parseMessages', () => {
  const call = { id: 'c1' };
  const base = {
    id: 'a1', role: 'assistant', content: '', createdAt: 0,
    toolCalls: JSON.stringify([call]),
    metadata: JSON.stringify({ toolCalls: [{ name: 'os', input: { resource: 'shell', action: 'exec', command: 'ls' } }], contentBlocks: [{ type: 'tool', toolCallIndex: 0 }] }),
  };
  const result = (extra: Record<string, unknown>) => ({
    id: 't1', role: 'tool', content: '', createdAt: 0,
    toolResults: JSON.stringify([{ tool_call_id: 'c1', content: 'ok', ...extra }]),
  });

  it('carries the persisted outcome and duration onto the tool', () => {
    const msgs = parseMessages([base, result({ outcome: 'Ran shell', duration_ms: 412 })] as never);
    const tool = (msgs[0] as { tools?: { outcome?: string; durationMs?: number }[] }).tools?.[0];
    expect(tool?.outcome).toBe('Ran shell');
    expect(tool?.durationMs).toBe(412);
  });

  // Stored, each iteration is its own row. Reloaded, they read like live:
  // consecutive tool-only rows share one bubble, narration after tools opens
  // the next, and a user row ends the turn.
  it('merges consecutive tool-only rows into one bubble like the live view', () => {
    const toolRow = (id: string, callId: string) => ({
      id, role: 'assistant', content: '', createdAt: 0,
      toolCalls: JSON.stringify([{ id: callId }]),
      metadata: JSON.stringify({ toolCalls: [{ name: 'os', input: { resource: 'shell', action: 'exec' } }], contentBlocks: [{ type: 'tool', toolCallIndex: 0 }] }),
    });
    const textRow = (id: string, content: string) => ({ id, role: 'assistant', content, createdAt: 0 });
    const userRow = { id: 'u1', role: 'user', content: 'continue', createdAt: 0 };
    const msgs = parseMessages([
      toolRow('a1', 'c1'), toolRow('a2', 'c2'), toolRow('a3', 'c3'),
      textRow('a4', 'Installed.'),
      toolRow('a5', 'c4'),
      userRow,
      toolRow('a6', 'c5'),
    ] as never);
    const shape = msgs.map((m) => m.type === 'assistant' ? `${m.content || '(tools)'}:${m.tools?.length ?? 0}` : m.type);
    expect(shape).toEqual(['(tools):3', 'Installed.:1', 'user', '(tools):1']);
  });

  // A result the server cut to a preview says so, and the row keeps the call
  // id — that pair is what lets the open row fetch the rest instead of the
  // page carrying every byte of every tool result.
  it('marks a cut result and keeps the call id to fetch the rest', () => {
    const msgs = parseMessages([base, result({ truncated: true, total_chars: 51234 })] as never);
    const tool = (msgs[0] as { tools?: { truncated?: boolean; toolId?: string }[] }).tools?.[0];
    expect(tool?.truncated).toBe(true);
    expect(tool?.toolId).toBe('c1');
  });

  it('leaves a whole result unmarked', () => {
    const msgs = parseMessages([base, result({})] as never);
    const tool = (msgs[0] as { tools?: { truncated?: boolean }[] }).tools?.[0];
    expect(tool?.truncated).toBeUndefined();
  });

  it('maps rows written before outcomes were persisted', () => {
    const msgs = parseMessages([base, result({})] as never);
    const tool = (msgs[0] as { tools?: { outcome?: string; label?: string }[] }).tools?.[0];
    expect(tool?.outcome).toBeUndefined();
    expect(tool?.label).toBeTruthy();
  });
});

// A team post relayed into a member's thread is stored as the envelope the
// model read, with metadata.teamPost derived by the list into {teamId,
// teamName, from, text}. The bubble carries that object; a row whose
// teamPost is still the bare `true` (derivation found no envelope) reads as
// an ordinary user message.
describe('parseMessages team posts', () => {
  const envelope = '[Team "Content & SEO" — rank for our services]\n[Post from Alma]\n\nDraft the outline.';
  const row = (teamPost: unknown) => ({
    id: 'u1', role: 'user', content: envelope, createdAt: 0,
    metadata: JSON.stringify({ teamPost, teamId: 'team-1' }),
  });

  it('carries the derived team post onto the user message', () => {
    const tp = { teamId: 'team-1', teamName: 'Content & SEO', from: 'Alma', text: 'Draft the outline.' };
    const [msg] = parseMessages([row(tp)] as never);
    expect(msg.type).toBe('user');
    expect((msg as { teamPost?: unknown }).teamPost).toEqual(tp);
  });

  // Who the owner is was decided on the server: the bubble reads `fromOwner`
  // and never re-tests the name (rows from before it was sent keep the
  // name comparison as their fallback).
  it('carries the server\'s fromOwner onto the bubble', () => {
    const tp = { teamId: 'team-1', teamName: 'Content & SEO', from: 'Owner', fromOwner: true, text: 'Draft the outline.' };
    const [msg] = parseMessages([row(tp)] as never);
    expect((msg as { teamPost?: { fromOwner?: boolean } }).teamPost?.fromOwner).toBe(true);
  });

  it('leaves an underived team post as a plain user message', () => {
    const [msg] = parseMessages([row(true)] as never);
    expect((msg as { teamPost?: unknown }).teamPost).toBeUndefined();
  });
});

// A checkpoint leaves ONE owner-visible system row flagged compactBoundary: it
// reads as a divider between the rows around it and ends the open assistant
// bubble. Any other system row stays out of the thread.
describe('parseMessages compact boundary', () => {
  const toolRow = (id: string, callId: string) => ({
    id, role: 'assistant', content: '', createdAt: 0,
    toolCalls: JSON.stringify([{ id: callId }]),
    metadata: JSON.stringify({ toolCalls: [{ name: 'os', input: { resource: 'shell', action: 'exec' } }], contentBlocks: [{ type: 'tool', toolCallIndex: 0 }] }),
  });
  const boundary = {
    id: 's1', role: 'system', content: 'Earlier conversation summarized', createdAt: 0,
    metadata: JSON.stringify({ compactBoundary: true, reason: 'threshold' }),
  };
  const plainSystem = { id: 's2', role: 'system', content: 'internal', createdAt: 0 };

  it('renders the boundary as one divider, drops plain system rows, and starts a new bubble after it', () => {
    const msgs = parseMessages([
      { id: 'u1', role: 'user', content: 'hi', createdAt: 0 },
      toolRow('a1', 'c1'),
      plainSystem,
      boundary,
      toolRow('a2', 'c2'),
    ] as never);
    const shape = msgs.map((m) => m.type === 'assistant' ? `${m.id}:${m.tools?.length ?? 0}` : m.type);
    expect(shape).toEqual(['user', 'a1-0:1', 'compactBoundary', 'a2-0:1']);
    expect(msgs[2]).toMatchObject({ type: 'compactBoundary', id: 's1' });
    expect(msgs[2]).not.toHaveProperty('cleared');
  });

  // The owner's /clear keeps every message and leaves the same marker with
  // reason "cleared": the thread shows everything, with the divider worded
  // for a clear where it happened.
  it('keeps the messages around a clear and marks the divider as cleared', () => {
    const cleared = {
      id: 's3', role: 'system', content: 'Cleared here. Earlier messages are kept but not used.', createdAt: 0,
      metadata: JSON.stringify({ compactBoundary: true, reason: 'cleared' }),
    };
    const msgs = parseMessages([
      { id: 'u1', role: 'user', content: 'before', createdAt: 0 },
      cleared,
      { id: 'u2', role: 'user', content: 'after', createdAt: 0 },
    ] as never);
    expect(msgs.map((m) => m.type)).toEqual(['user', 'compactBoundary', 'user']);
    expect(msgs[1]).toMatchObject({ type: 'compactBoundary', id: 's3', cleared: true });
  });
});

// A run that ended on an error leaves it in the thread (a system row flagged
// runError): no bubble, and the chat raises its error banner from it when the
// error is the newest row. A later row means the thread moved on.
describe('run error rows', () => {
  const user = { id: 'u1', role: 'user', content: 'Hi', createdAt: 0 };
  const failed = {
    id: 'e1', role: 'system', content: 'USAGE_LIMIT_EXCEEDED: no balance', createdAt: 1,
    metadata: JSON.stringify({ runError: true }),
  };

  it('never renders as a bubble', () => {
    expect(parseMessages([user, failed] as never).map((m) => m.type)).toEqual(['user']);
  });

  it('is the banner when it is the newest row', () => {
    expect(lastRunError([user, failed] as never)).toBe('USAGE_LIMIT_EXCEEDED: no balance');
    expect(lastRunError([failed] as never)).toBe('USAGE_LIMIT_EXCEEDED: no balance');
  });

  it('is not the banner once the thread moved on', () => {
    const later = { id: 'u2', role: 'user', content: 'Try again', createdAt: 2 };
    expect(lastRunError([user, failed, later] as never)).toBeNull();
    expect(lastRunError([user] as never)).toBeNull();
    expect(lastRunError([] as never)).toBeNull();
  });
});

// A colleague's message is its words, and the bubble knows whose they are
// (the server names the colleague; no label ever rides the words).
describe('a colleague message', () => {
  it('carries the colleague onto the bubble and the words alone', () => {
    const msgs = parseMessages([
      { id: 'u1', role: 'user', content: 'The invoice is sent.', createdAt: 0, metadata: JSON.stringify({ from: 'coworker', coworker: 'Top Coder', fromColleague: 'Top Coder' }) },
      { id: 'u2', role: 'user', content: 'thanks', createdAt: 0 },
    ] as never);
    const [colleague, owner] = msgs as { type: string; content: string; fromColleague?: string }[];
    expect(colleague.fromColleague).toBe('Top Coder');
    expect(colleague.content).toBe('The invoice is sent.');
    expect(owner.fromColleague).toBeUndefined();
  });
});
