import { describe, expect, it } from 'vitest';
import { parseMessages } from './history';

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
