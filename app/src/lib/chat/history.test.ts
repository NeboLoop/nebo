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

  it('maps rows written before outcomes were persisted', () => {
    const msgs = parseMessages([base, result({})] as never);
    const tool = (msgs[0] as { tools?: { outcome?: string; label?: string }[] }).tools?.[0];
    expect(tool?.outcome).toBeUndefined();
    expect(tool?.label).toBeTruthy();
  });
});
