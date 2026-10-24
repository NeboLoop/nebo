// The code check's questions are worded in the owner's language on every
// card (the owner: every card or alert he sees is in his language). The
// English the bot sends is the fallback, and an option still answers with
// its English label.
import { describe, it, expect } from 'vitest';
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { wordedContext, wordedOption, wordedQuestion } from './askWords';

const dir = join(__dirname, '../i18n/locales');
const locale = (f: string) => JSON.parse(readFileSync(join(dir, f), 'utf8'));
const en = locale('en.json');

/** The placeholders a message fills in, by name (a plural's branches, such
 *  as `one {it}`, are words, not placeholders). */
const names = (m: string) =>
  [...new Set([...m.matchAll(/(?<!(?:zero|one|two|few|many|other) )\{(\w+)(?=[,}])/g)].map((x) => x[1]))].sort();

describe('the code check asks in the owner language', () => {
  it('every desktop locale words every question and choice, with the same placeholders', () => {
    const keys = Object.keys(en.verify.ask);
    expect(keys).toEqual(expect.arrayContaining(['stalled', 'reviewFailed', 'noChecks', 'ownerEdits', 'ownerEditsContext', 'cantLand', 'keepGoing', 'landAnyway', 'stopHere', 'keepInCopy', 'landAfterMine', 'keepMine']));
    for (const f of readdirSync(dir).filter((f) => f.endsWith('.json'))) {
      const ask = locale(f).verify?.ask ?? {};
      for (const k of keys) {
        expect(typeof ask[k], `${f} verify.ask.${k}`).toBe('string');
        expect(names(ask[k]), `${f} verify.ask.${k}`).toEqual(names(en.verify.ask[k]));
      }
    }
  });

  it('a keyed question, context and option are worded by their keys; the answer stays the label', () => {
    const tr = (key: string, opts?: { values?: Record<string, string | number>; default?: string }) =>
      `${key}${opts?.values ? JSON.stringify(opts.values) : ''}`;
    const words = {
      questionKey: 'verify.ask.reviewFailed',
      questionValues: { project: '/p', count: 1, tries: 3 },
      contextKey: 'verify.ask.ownerEditsContext',
      contextValues: { path: '/c' },
      optionKeys: ['verify.ask.keepGoing', null]
    };
    expect(wordedQuestion(tr, 'English', words)).toBe('verify.ask.reviewFailed{"project":"/p","count":1,"tries":3}');
    expect(wordedContext(tr, 'English', words)).toBe('verify.ask.ownerEditsContext{"path":"/c"}');
    expect(wordedOption(tr, ['Keep going', 'cents'], 'Keep going', words)).toBe('verify.ask.keepGoing');
    expect(wordedOption(tr, ['Keep going', 'cents'], 'cents', words)).toBe('cents');
    // An employee's own question shows as written.
    expect(wordedQuestion(tr, 'Which vendor?', undefined)).toBe('Which vendor?');
    expect(wordedOption(tr, ['Acme'], 'Acme', {})).toBe('Acme');
  });
});
