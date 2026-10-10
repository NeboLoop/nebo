import { describe, expect, it } from 'vitest';
import { turnBlocks, turnText, noteText, type TurnSegment } from './turnBlocks';
import { parseMessages } from './history';

type Tool = { name: string; status?: string };
// Every call is shown, failed ones too.
const shown = (tools: Tool[] | undefined) => tools ?? [];
const work = (tools: Tool[] | undefined) => (tools ?? []).length > 0;
const shape = (segs: TurnSegment<Tool>[], ended = false) =>
  turnBlocks(segs, 'k', shown, work, ended).map((b) =>
    b.kind === 'group'
      ? `group:${b.steps.map((s) => (s.kind === 'note' ? `note(${s.lines.join('|')})` : `${s.tool.name}${s.tool.status === 'error' ? '!' : ''}`)).join(',')}`
      : `${b.kind}:${b.text}`,
  );

describe('turnBlocks', () => {
  // Text between calls is a note in view, never a row hidden in a group;
  // the last text with no call after it is the answer.
  it('shows text between calls as notes and the answer as prose', () => {
    expect(
      shape([
        { content: 'The export has 212 rows.', fold: 'shown', tools: [{ name: 'read' }] },
        { content: 'So the March invoices are missing.', fold: 'shown', tools: [{ name: 'list' }, { name: 'read' }] },
        { content: 'Done: all ten rebuilt.' },
      ]),
    ).toEqual([
      'note:The export has 212 rows.',
      'group:read',
      'note:So the March invoices are missing.',
      'group:list,read',
      'prose:Done: all ten rebuilt.',
    ]);
  });

  it('reads Japanese the same way', () => {
    expect(
      shape([
        { content: '請求書を確認しました。3月分が2件足りません！', fold: 'shown', tools: [{ name: 'read_file' }] },
        { content: '不足分をまとめました。' },
      ]),
    ).toEqual(['note:請求書を確認しました。3月分が2件足りません！', 'group:read_file', 'prose:不足分をまとめました。']);
  });

  // A word-for-word repeat (and any row an older rule folded) sits in the
  // group as a note row the reader can open.
  it('keeps a folded segment as a note row in the group', () => {
    expect(
      shape([
        { content: 'Four clips in.', fold: 'shown', tools: [{ name: 'remember' }] },
        { content: 'Four clips in!', fold: 'folded', tools: [{ name: 'remember' }] },
        { content: 'Here is the plan.' },
      ]),
    ).toEqual(['note:Four clips in.', 'group:remember,note(Four clips in!),remember', 'prose:Here is the plan.']);
  });

  it('reads a verdict-less segment with calls as a note, and streaming text as prose', () => {
    expect(
      shape([
        { content: 'Checking.', tools: [{ name: 'read' }] },
        { content: 'Still writing the ans' },
      ]),
    ).toEqual(['note:Checking.', 'group:read', 'prose:Still writing the ans']);
  });

  // A step whose calls all failed still shows its group, each call in it.
  it('keeps failed calls in their group', () => {
    expect(
      shape([
        { content: 'Opening the sheet.', fold: 'shown', tools: [{ name: 'read', status: 'error' }, { name: 'read', status: 'error' }] },
        { content: 'The sheet could not be opened.' },
      ]),
    ).toEqual(['note:Opening the sheet.', 'group:read!,read!', 'prose:The sheet could not be opened.']);
  });

  // A turn that ended without an answer reads its last note as one.
  it('reads the last note of an unanswered turn as its answer once it ends', () => {
    const segs: TurnSegment<Tool>[] = [
      { content: 'Read the first file.', fold: 'shown', tools: [{ name: 'read' }] },
      { content: 'The second file holds the rest.', fold: 'shown', tools: [{ name: 'read' }] },
    ];
    expect(shape(segs)).toEqual(['note:Read the first file.', 'group:read', 'note:The second file holds the rest.', 'group:read']);
    expect(shape(segs, true)).toEqual(['note:Read the first file.', 'group:read', 'prose:The second file holds the rest.', 'group:read']);
  });

  // Copy takes every word the turn shows or folds, in order.
  it('copies notes, folded notes and the answer', () => {
    const blocks = turnBlocks(
      [
        { content: 'First finding.', fold: 'shown', tools: [{ name: 'read' }] },
        { content: 'First finding!', fold: 'folded', tools: [{ name: 'read' }] },
        { content: 'The answer.' },
      ],
      'k',
      shown,
      work,
    );
    expect(turnText(blocks)).toBe('First finding.\n\nFirst finding!\n\nThe answer.');
  });
});

describe('noteText', () => {
  it('drops emphasis and code marks but keeps names whole', () => {
    expect(noteText('**Done.** Updated `update_employee` for *.xlsx files')).toBe('Done. Updated update_employee for *.xlsx files');
    expect(noteText('# Heading\n> quoted\nnext_step_id')).toBe('Heading quoted next_step_id');
  });

  it('keeps lines when the note is opened', () => {
    expect(noteText('line one\n\n\n\nline_two', false)).toBe('line one\n\nline_two');
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
