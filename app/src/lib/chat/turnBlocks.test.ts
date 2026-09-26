import { describe, expect, it } from 'vitest';
import {
  turnBlocks,
  turnProse,
  noteLabel,
  type TurnSegment,
} from './turnBlocks';
import { parseMessages } from './history';

type Tool = { name: string; status?: string };
const shown = (tools: Tool[] | undefined) =>
  (tools ?? []).filter((t) => t.status !== 'error');
const work = (tools: Tool[] | undefined) => (tools ?? []).length > 0;
const shape = (segs: TurnSegment<Tool>[]) =>
  turnBlocks(segs, 'k', shown, work).map((b) =>
    b.kind === 'prose'
      ? `prose:${b.text}`
      : `group:${b.steps.map((s) => (s.kind === 'note' ? `note(${s.lines.join('|')})` : s.tool.name)).join(',')}`,
  );

// The server's verdict decides, not the segment's position: a paragraph the
// model wrote between calls that it kept stays prose where it was written.
describe('turnBlocks', () => {
  it('keeps a shown segment as prose though tools follow it', () => {
    expect(
      shape([
        {
          content: 'That worked — the problem is the metadata.',
          fold: 'shown',
          tools: [{ name: 'delete' }],
        },
        { content: 'Done: all ten rebuilt.' },
      ]),
    ).toEqual([
      'prose:That worked — the problem is the metadata.',
      'group:delete',
      'prose:Done: all ten rebuilt.',
    ]);
  });

  it('turns a folded segment into a note row in the group', () => {
    expect(
      shape([
        {
          content: 'Looking at your calendar.',
          fold: 'shown',
          tools: [{ name: 'calendar' }],
        },
        {
          content: 'Let me check the workflows.',
          fold: 'folded',
          tools: [{ name: 'list' }, { name: 'read' }],
        },
        { content: 'Here is the plan.' },
      ]),
    ).toEqual([
      'prose:Looking at your calendar.',
      'group:calendar,note(Let me check the workflows.),list,read',
      'prose:Here is the plan.',
    ]);
  });

  it('renders trailing undecided text as prose, and a verdict-less segment with tools as a note', () => {
    expect(
      shape([
        { content: 'Checking.', tools: [{ name: 'read' }] },
        { content: 'Still writing the ans' },
      ]),
    ).toEqual(['group:note(Checking.),read', 'prose:Still writing the ans']);
  });

  // The server's end-of-turn net shows the longest folded segment; that
  // verdict arrives like any other and the segment reads as prose.
  it('shows the segment the safety net unfolded', () => {
    const blocks = turnBlocks(
      [
        {
          content: 'Now the next file.',
          fold: 'folded',
          tools: [{ name: 'read' }],
        },
        {
          content: 'Checking the rest of the invoices now.',
          fold: 'shown',
          tools: [{ name: 'read' }],
        },
      ],
      'k',
      shown,
      work,
    );
    expect(turnProse(blocks)).toBe('Checking the rest of the invoices now.');
  });

  it('labels a note by its first line, at most 200 characters', () => {
    expect(noteLabel('First line\nsecond')).toBe('First line');
    expect(noteLabel('x'.repeat(250))).toBe(`${'x'.repeat(200)}…`);
  });
});

describe('parseMessages carries the stored verdict', () => {
  it('puts each text block’s fold on its bubble', () => {
    const row = (id: string, content: string, fold: string) => ({
      id,
      role: 'assistant',
      content,
      createdAt: 0,
      toolCalls: JSON.stringify([{ id: `c-${id}` }]),
      metadata: JSON.stringify({
        toolCalls: [{ name: 'os', input: {} }],
        contentBlocks: [
          { type: 'text', text: content, fold },
          { type: 'tool', toolCallIndex: 0 },
        ],
      }),
    });
    const msgs = parseMessages([
      row('a1', 'That worked.', 'shown'),
      row('a2', 'Let me check.', 'folded'),
    ] as never);
    expect(msgs.map((m) => (m.type === 'assistant' ? m.fold : null))).toEqual([
      'shown',
      'folded',
    ]);
  });
});
