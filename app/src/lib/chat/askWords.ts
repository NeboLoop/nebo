/** A question Nebo itself asks (the code check's) carries the message keys
 *  its question and each option are worded by in the owner's language, with
 *  the values they fill in. The English it carries is the
 *  fallback, and an option always answers with its English label. A
 *  question an employee wrote has none and shows as written. */
export interface AskWords {
  questionKey?: string;
  questionValues?: Record<string, string | number>;
  optionKeys?: (string | null)[];
}

type Tr = (key: string, opts?: { values?: Record<string, string | number>; default?: string }) => string;

/** The question as the owner reads it. */
export function wordedQuestion(tr: Tr, text: string, w?: AskWords | null): string {
  return w?.questionKey ? tr(w.questionKey, { values: w.questionValues, default: text }) : text;
}

/** Option `label` of `labels` as he reads it; the answer stays `label`. */
export function wordedOption(tr: Tr, labels: string[], label: string, w?: AskWords | null): string {
  const key = w?.optionKeys?.[labels.indexOf(label)];
  return key ? tr(key, { default: label }) : label;
}
