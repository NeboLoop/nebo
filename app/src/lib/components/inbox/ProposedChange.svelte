<!--
  A change an employee proposes to itself (a learned skill, or a tuning of
  one of its own workflows), as the owner decides on it: what went wrong, the
  fix, and what happens now, in the employee's own words; then what changes,
  as the server worked it out from the change itself; then, for those who
  want it, the raw change in the code view. A proposal filed before it
  carried the three parts shows the rest under its one-line headline.
-->
<script lang="ts" module>
  import type { CodeLine } from '$lib/components/ui/CodeView.svelte';
  import type { ChangeExplanation } from '$lib/api/neboComponents';

  export type When = { type: string; cron?: string };
  export type Change =
    | { kind: 'stepAdded' | 'stepRemoved'; step: string; text: string }
    | { kind: 'stepRenamed'; before: string; after: string }
    | { kind: 'stepChanged'; step: string; before: string; after: string }
    | { kind: 'stepsReordered'; steps: string[] }
    | { kind: 'scheduleChanged'; before: When | null; after: When | null; timezone: string | null }
    | { kind: 'descriptionChanged'; before: string; after: string }
    | { kind: 'sectionAdded' | 'sectionRemoved'; section: string; text: string }
    | { kind: 'sectionChanged'; section: string; before: string; after: string }
    | { kind: 'settingChanged'; setting: string; before: string; after: string };
  export type Learning = {
    kind?: string;
    action?: string;
    target?: string;
    content?: string | null;
    explanation?: ChangeExplanation;
    changes?: Change[];
    rawChange?: { language: string; original: 'present' | 'new' | 'unavailable'; lines: CodeLine[] };
  };
</script>

<script lang="ts">
  import { t } from 'svelte-i18n';
  import { parseMarkdown } from '$lib/markdown';
  import { describeCron } from '$lib/utils/schedule';
  import CodeView from '$lib/components/ui/CodeView.svelte';

  let { learning }: { learning: Learning } = $props();

  const isWorkflow = $derived(learning.kind === 'workflow');
  const raw = $derived(learning.rawChange);
  let rawOpen = $state(false);

  /** A staged SKILL.md, split for reading: the front-matter fields as a
   *  header, the body as markdown. Rendered, not edited: what you see is
   *  exactly what Approve writes. */
  function splitSkill(text: string): { meta: Array<[string, string]>; body: string } {
    const m = /^---\n([\s\S]*?)\n---\n?/.exec(text);
    if (!m) return { meta: [], body: text };
    const meta = m[1]
      .split('\n')
      .map((line) => line.split(/:\s*/, 2))
      .filter((kv): kv is [string, string] => kv.length === 2 && kv[0].trim() !== '')
      .map(([k, v]) => [k.trim(), v.replace(/^["']|["']$/g, '').trim()] as [string, string]);
    return { meta, body: text.slice(m[0].length) };
  }

  /** When a workflow runs, in words: its schedule on the owner's clock. */
  function whenWords(w: When | null, zone: string | null): string {
    if (!w) return $t('proposedChange.whenManual');
    if (w.type === 'manual') return $t('proposedChange.whenManual');
    if (w.type !== 'schedule' || !w.cron) return $t('proposedChange.whenOther');
    const words = describeCron(w.cron) ?? w.cron;
    const city = zone?.split('/').pop()?.replaceAll('_', ' ');
    return city ? $t('proposedChange.inZone', { values: { when: words, zone: city } }) : words;
  }

  const sectionName = (s: string) => s || $t('proposedChange.sectionOpening');

  const rawNote = $derived(
    raw?.original === 'new' ? $t('proposedChange.rawNew') : raw?.original === 'unavailable' ? $t('proposedChange.rawUnavailable') : '',
  );
</script>

<div class="proposed-change">
  {#if learning.explanation}
    <section>
      <h3 class="proposed-change-heading">{$t('proposedChange.whatWentWrong')}</h3>
      <p class="proposed-change-text">{learning.explanation.whatWentWrong}</p>
    </section>
    <section>
      <h3 class="proposed-change-heading">{$t('proposedChange.theFix')}</h3>
      <p class="proposed-change-text">{learning.explanation.theFix}</p>
    </section>
    <section>
      <h3 class="proposed-change-heading">{$t('proposedChange.whatHappensNow')}</h3>
      <p class="proposed-change-text">{learning.explanation.whatHappensNow}</p>
    </section>
  {/if}

  {#if learning.changes?.length}
    <section>
      <h3 class="proposed-change-heading">{$t('proposedChange.whatChanges')}</h3>
      <ul class="proposed-change-list">
        {#each learning.changes as c, i (i)}
          <li>
            {#if c.kind === 'scheduleChanged'}
              {#if c.before === null}
                <div>{$t('proposedChange.scheduleSet', { values: { after: whenWords(c.after, c.timezone) } })}</div>
              {:else}
                <div>{$t('proposedChange.scheduleChanged')}</div>
                <div class="proposed-change-old">{whenWords(c.before, c.timezone)}</div>
                <div class="proposed-change-new">{whenWords(c.after, c.timezone)}</div>
              {/if}
            {:else if c.kind === 'stepAdded'}
              <div>{$t('proposedChange.stepAdded', { values: { step: c.step } })}</div>
              {#if c.text}<div class="proposed-change-new">{c.text}</div>{/if}
            {:else if c.kind === 'stepRemoved'}
              <div>{$t('proposedChange.stepRemoved', { values: { step: c.step } })}</div>
              {#if c.text}<div class="proposed-change-old">{c.text}</div>{/if}
            {:else if c.kind === 'stepRenamed'}
              <div>{$t('proposedChange.stepRenamed', { values: { before: c.before, after: c.after } })}</div>
            {:else if c.kind === 'stepChanged'}
              <div>{$t('proposedChange.stepChanged', { values: { step: c.step } })}</div>
              <div class="proposed-change-old">{c.before}</div>
              <div class="proposed-change-new">{c.after}</div>
            {:else if c.kind === 'stepsReordered'}
              <div>{$t('proposedChange.stepsReordered', { values: { steps: c.steps.join(' → ') } })}</div>
            {:else if c.kind === 'descriptionChanged'}
              <div>{$t('proposedChange.descriptionChanged')}</div>
              <div class="proposed-change-old">{c.before}</div>
              <div class="proposed-change-new">{c.after}</div>
            {:else if c.kind === 'sectionAdded'}
              <div>{$t('proposedChange.sectionAdded', { values: { section: sectionName(c.section) } })}</div>
              {#if c.text}<div class="proposed-change-new">{c.text}</div>{/if}
            {:else if c.kind === 'sectionRemoved'}
              <div>{$t('proposedChange.sectionRemoved', { values: { section: sectionName(c.section) } })}</div>
              {#if c.text}<div class="proposed-change-old">{c.text}</div>{/if}
            {:else if c.kind === 'sectionChanged'}
              <div>{$t('proposedChange.sectionChanged', { values: { section: sectionName(c.section) } })}</div>
              <div class="proposed-change-old">{c.before}</div>
              <div class="proposed-change-new">{c.after}</div>
            {:else if c.kind === 'settingChanged'}
              <div>{$t('proposedChange.settingChanged', { values: { setting: c.setting } })}</div>
              {#if c.before}<div class="proposed-change-old">{c.before}</div>{/if}
              {#if c.after}<div class="proposed-change-new">{c.after}</div>{/if}
            {/if}
          </li>
        {/each}
      </ul>
    </section>
  {/if}

  {#if !isWorkflow && learning.content}
    {@const skill = splitSkill(learning.content)}
    <section>
      <h3 class="proposed-change-heading">{$t('inbox.stagedText', { values: { action: learning.action ?? '', target: learning.target ?? '' } })}</h3>
      <div class="rounded-lg border border-base-content/10 bg-base-100">
        {#if skill.meta.length > 0}
          <div class="px-5 py-3 border-b border-base-content/10 bg-base-200/50 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1">
            {#each skill.meta as [k, v] (k)}
              <span class="text-xs font-mono text-base-content/50">{k}</span>
              <span class="text-sm {k === 'name' ? 'font-semibold' : 'text-base-content/80'}">{v}</span>
            {/each}
          </div>
        {/if}
        <div class="px-5 py-4 prose prose-sm max-w-none [&>:first-child]:mt-0">
          {@html parseMarkdown(skill.body)}
        </div>
      </div>
    </section>
  {/if}

  {#if raw && raw.lines.length > 0}
    <button type="button" class="proposed-change-raw-link" onclick={() => (rawOpen = true)}>
      {$t('proposedChange.showRawChange')}
    </button>
    <CodeView
      open={rawOpen}
      title={$t('proposedChange.rawTitle')}
      onclose={() => (rawOpen = false)}
      lines={raw.lines}
      language={raw.language}
      note={rawNote}
    />
  {/if}
</div>
