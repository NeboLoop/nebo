// One assistant turn as the thread shows it: the text the employee wrote
// between its calls, the groups of calls themselves, and the answer.
//
// Text between calls is always shown, as a compact note the reader opens
// (`note`); the answer — the last text, with no calls after it — reads as
// prose. The server gives each text segment a verdict once a call follows
// it: `shown` is a note in the thread; `folded` is a word-for-word repeat of
// text the turn already shows, and sits as a note row inside the group
// (rows stored before this rule folded more; they read the same way and
// open the same way). A segment with no verdict is the answer when it ran
// no calls, and a note when it did (stored before verdicts existed).

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
  | { kind: 'note'; key: string; text: string }
  | { kind: 'group'; key: string; steps: TurnStep<T>[]; tools: T[] };

/**
 * The turn's blocks in order. `shown` picks the calls a reader sees;
 * `work` says whether a segment ran calls that belong in a group at all (a
 * coworker send is an event, not work). `ended`: the turn is over — when it
 * ended without an answer, its last note reads as the answer, so a turn
 * always leaves something to read.
 */
export function turnBlocks<T>(
  segs: TurnSegment<T>[],
  keyId: string,
  shown: (tools: T[] | undefined) => T[],
  work: (tools: T[] | undefined) => boolean,
  ended = false,
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
      if (seg.fold === 'folded') {
        const g = openGroup();
        const prev = g.steps[g.steps.length - 1];
        if (prev?.kind === 'note') prev.lines.push(text);
        else
          g.steps.push({ kind: 'note', key: `${keyId}-n${si}`, lines: [text] });
      } else {
        group = null;
        const between = seg.fold === 'shown' || work(seg.tools);
        blocks.push(
          between
            ? { kind: 'note', key: `${keyId}-n${si}`, text }
            : { kind: 'prose', key: `${keyId}-p${si}`, text: seg.content },
        );
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
  if (ended && !blocks.some((b) => b.kind === 'prose')) {
    for (let i = blocks.length - 1; i >= 0; i--) {
      const b = blocks[i];
      if (b.kind === 'note') {
        blocks[i] = { kind: 'prose', key: b.key, text: b.text };
        break;
      }
    }
  }
  return blocks;
}

/** The turn's words, for copying: every note (folded ones too) and the
 *  answer, in order. */
export function turnText<T>(blocks: TurnBlock<T>[]): string {
  return blocks
    .flatMap((b) =>
      b.kind === 'group'
        ? b.steps.flatMap((s) => (s.kind === 'note' ? s.lines : []))
        : [b.text],
    )
    .join('\n\n');
}

/** A note as plain words: markdown emphasis, code ticks and heading or
 *  quote marks drop; every other character stays, so names like
 *  `update_employee` or `*.xlsx` read as written. `oneLine` joins its lines
 *  for the compact row; the opened note keeps them. */
export function noteText(text: string, oneLine = true): string {
  const plain = text
    .replace(/\*\*(?=\S)([\s\S]*?\S)\*\*/g, '$1')
    .replace(/`([^`\n]+)`/g, '$1')
    .replace(/^[ \t]*(?:#{1,6}[ \t]+|>[ \t]?)/gm, '');
  return (oneLine ? plain.replace(/\s+/g, ' ') : plain.replace(/\n{3,}/g, '\n\n')).trim();
}
