<!--
  The one code view: raw text for the people who want to read it, in its own
  sheet (ShelfModal: a centered card on desktop, a slide-up drawer on a
  phone). Syntax-coloured from the theme's own tokens (shiki reads the
  grammar; the colours are app.css's `code-tok-*`, so light and dark follow
  the theme), line numbers, monospace, scrolls both ways, Copy in the header.

  `lines` is the text a line at a time. A line diff marks lines removed
  ("-", red) and added ("+", green); runs of unchanged lines between changes
  fold, and a tap opens them. Plain text is all " " lines, and nothing folds.
-->
<script lang="ts" module>
  export type CodeLine = { op: ' ' | '-' | '+'; text: string; oldLine?: number; newLine?: number };
</script>

<script lang="ts">
  import { t } from 'svelte-i18n';
  import Copy from 'lucide-svelte/icons/copy';
  import Check from 'lucide-svelte/icons/check';
  import ShelfModal from './ShelfModal.svelte';

  interface Props {
    open: boolean;
    title: string;
    onclose: () => void;
    lines: CodeLine[];
    /** A shiki language id (json, markdown, …); 'text' colours nothing. */
    language?: string;
    /** One line above the code, when something about it needs saying. */
    note?: string;
  }
  let { open, title, onclose, lines, language = 'text', note = '' }: Props = $props();

  /** Unchanged lines kept beside a change; a longer unchanged run folds. */
  const CONTEXT = 3;

  type Piece = { text: string; kind: string };
  let pieces = $state<Piece[][] | null>(null);

  // The colour class for a token, from the grammar's scopes (the innermost
  // that means something here).
  function kindOf(scopes: string[]): string {
    for (let i = scopes.length - 1; i >= 0; i--) {
      const s = scopes[i];
      if (s.startsWith('support.type.property-name') || s.startsWith('markup.heading') || s.startsWith('entity.name.section')) return 'key';
      if (s.startsWith('string') || s.startsWith('markup.inline.raw')) return 'string';
      if (s.startsWith('constant.numeric')) return 'number';
      if (s.startsWith('constant.language')) return 'literal';
      if (s.startsWith('punctuation')) return 'punct';
    }
    return '';
  }

  $effect(() => {
    if (!open) return;
    const text = lines.map((l) => l.text).join('\n');
    const lang = language;
    pieces = null;
    let cancelled = false;
    (async () => {
      if (lang === 'text') return;
      try {
        const { codeToTokens } = await import('shiki');
        const { tokens } = await codeToTokens(text, { lang: lang as 'json', theme: 'github-light', includeExplanation: 'scopeName' });
        if (cancelled) return;
        pieces = tokens.map((line) =>
          line.flatMap((tok) =>
            (tok.explanation ?? [{ content: tok.content, scopes: [] }]).map((part) => ({
              text: part.content,
              kind: kindOf(part.scopes.map((s) => s.scopeName)),
            })),
          ),
        );
      } catch {
        // Uncoloured is still readable.
      }
    })();
    return () => { cancelled = true; };
  });

  // Folding: which unchanged lines are far enough from any change to fold.
  const hasChanges = $derived(lines.some((l) => l.op !== ' '));
  let opened = $state<Set<number>>(new Set());
  $effect(() => { if (open) opened = new Set(); });

  type Row = { kind: 'line'; index: number } | { kind: 'fold'; start: number; count: number };
  const rows = $derived.by((): Row[] => {
    if (!hasChanges) return lines.map((_, index) => ({ kind: 'line', index }));
    const near = lines.map(() => false);
    lines.forEach((l, i) => {
      if (l.op === ' ') return;
      for (let j = Math.max(0, i - CONTEXT); j <= Math.min(lines.length - 1, i + CONTEXT); j++) near[j] = true;
    });
    const out: Row[] = [];
    let i = 0;
    while (i < lines.length) {
      if (near[i] || opened.has(i)) { out.push({ kind: 'line', index: i }); i++; continue; }
      const start = i;
      while (i < lines.length && !near[i] && !opened.has(i)) i++;
      out.push({ kind: 'fold', start, count: i - start });
    }
    return out;
  });

  function unfold(start: number, count: number) {
    const next = new Set(opened);
    for (let i = start; i < start + count; i++) next.add(i);
    opened = next;
  }

  let copied = $state(false);
  async function copy() {
    const text = hasChanges ? lines.map((l) => `${l.op}${l.text}`).join('\n') : lines.map((l) => l.text).join('\n');
    try {
      await navigator.clipboard.writeText(text);
      copied = true;
      setTimeout(() => (copied = false), 1500);
    } catch {
      // Clipboard refused (no permission): the text is still selectable.
    }
  }

  const opClass = (op: string) => (op === '+' ? 'code-line-add' : op === '-' ? 'code-line-del' : '');
</script>

<ShelfModal {open} {title} {onclose}>
  {#snippet actions()}
    <button type="button" class="btn btn-ghost btn-xs gap-1.5" onclick={copy}>
      {#if copied}<Check class="w-3.5 h-3.5 text-success" />{:else}<Copy class="w-3.5 h-3.5" />{/if}
      {$t('common.copy')}
    </button>
  {/snippet}
  <div class="flex-1 min-w-0 min-h-0 flex flex-col">
    {#if note}
      <div class="shrink-0 px-4 py-2 text-xs text-base-content/70 border-b border-base-300">{note}</div>
    {/if}
    <div class="code-view flex-1 min-h-0 overflow-auto">
      <table class="code-view-table">
        <tbody>
          {#each rows as row (row.kind === 'line' ? `l${row.index}` : `f${row.start}`)}
            {#if row.kind === 'line'}
              {@const l = lines[row.index]}
              <tr class={opClass(l.op)}>
                {#if hasChanges}<td class="code-view-num">{l.oldLine ?? ''}</td>{/if}
                <td class="code-view-num">{hasChanges ? (l.newLine ?? '') : row.index + 1}</td>
                {#if hasChanges}<td class="code-view-op">{l.op === ' ' ? '' : l.op}</td>{/if}
                <td class="code-view-text">{#if pieces?.[row.index]}{#each pieces[row.index] as p, k (k)}<span class={p.kind ? `code-tok-${p.kind}` : ''}>{p.text}</span>{/each}{:else}{l.text}{/if}</td>
              </tr>
            {:else}
              <tr class="code-view-fold">
                <td colspan="4">
                  <button type="button" class="code-view-fold-button" onclick={() => unfold(row.start, row.count)}>
                    {$t('proposedChange.unchangedLines', { values: { n: row.count } })}
                  </button>
                </td>
              </tr>
            {/if}
          {/each}
        </tbody>
      </table>
    </div>
  </div>
</ShelfModal>
