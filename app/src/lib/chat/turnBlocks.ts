// One assistant turn as the thread shows it: prose paragraphs and, between
// them, groups of the work the employee did (tool calls and the notes it wrote
// about them). The server gives each text segment a verdict once a tool call
// follows it — `shown` stays prose, `folded` becomes a note row in the group —
// and the thread renders that verdict, live and reloaded alike. A segment with
// no verdict is prose when it ran no tools (the answer still streaming, the
// last word of the turn); a segment that ran tools without one was stored
// before verdicts existed and reads as it always did, as a note.

export type Fold = 'shown' | 'folded';

export interface TurnSegment<T> {
  content: string;
  tools?: T[];
  fold?: Fold;
}

export type TurnStep<T> =
  | { kind: 'note'; key: string; lines: string[] }
  | { kind: 'tool'; key: string; tool: T };

export type TurnBlock<T> =
  | { kind: 'prose'; key: string; text: string }
  | { kind: 'group'; key: string; steps: TurnStep<T>[]; tools: T[] };

/** How many of a live group's latest rows show under its summary line. */
export const LIVE_PREVIEW_ROWS = 3;

/**
 * The turn's blocks in order. `shown` picks the calls a reader sees (failed
 * retries leave out); `work` says whether a segment ran calls that belong in
 * the group at all (a coworker send is an event, not work).
 */
export function turnBlocks<T>(
  segs: TurnSegment<T>[],
  keyId: string,
  shown: (tools: T[] | undefined) => T[],
  work: (tools: T[] | undefined) => boolean,
): TurnBlock<T>[] {
  const blocks: TurnBlock<T>[] = [];
  let group: Extract<TurnBlock<T>, { kind: 'group' }> | null = null;
  const openGroup = () => {
    if (!group) {
      group = {
        kind: 'group',
        key: `${keyId}-g${blocks.length}`,
        steps: [],
        tools: [],
      };
      blocks.push(group);
    }
    return group;
  };
  segs.forEach((seg, si) => {
    const text = seg.content?.trim() ?? '';
    if (text) {
      const folded =
        seg.fold === 'folded' || (seg.fold === undefined && work(seg.tools));
      if (folded) {
        const g = openGroup();
        const prev = g.steps[g.steps.length - 1];
        if (prev?.kind === 'note') prev.lines.push(text);
        else
          g.steps.push({ kind: 'note', key: `${keyId}-n${si}`, lines: [text] });
      } else {
        group = null;
        blocks.push({
          kind: 'prose',
          key: `${keyId}-p${si}`,
          text: seg.content,
        });
      }
    }
    const calls = shown(seg.tools);
    if (calls.length) {
      const g = openGroup();
      calls.forEach((tool, ti) => {
        g.steps.push({ kind: 'tool', key: `${keyId}-${si}-${ti}`, tool });
        g.tools.push(tool);
      });
    }
  });
  return blocks;
}

/** The turn's prose, for copying: every shown paragraph, in order. */
export function turnProse<T>(blocks: TurnBlock<T>[]): string {
  return blocks
    .flatMap((b) => (b.kind === 'prose' ? [b.text] : []))
    .join('\n\n');
}

/** A note row's label: its first line, at most 200 characters. */
export function noteLabel(text: string): string {
  const first = text.split('\n').find((l) => l.trim()) ?? '';
  return first.length > 200 ? `${first.slice(0, 200)}…` : first;
}
