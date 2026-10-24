<!--
  DecisionCard — the ONE card a decision is asked on: an employee's question,
  a permission ask, an approval, and the install and connect cards in the
  chat. The same card in the composer's place, the Inbox, the dashboard, the
  waiting-on-you popover and the asks relayed to the assistant
  ($lib/chat/decision). Centered: the asking employee's avatar (or the app's
  or plugin's icon), one line with only its key words bold, the bold action
  under it and the reason why it asks. The details fold behind "Show
  details" as label and value rows. The answers are full-width stacked
  buttons in one order; Stop sits alone in the corner, apart from them.
  Answered, each caller collapses it to its one-line receipt (AskReceipt).
-->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import { t } from 'svelte-i18n';
  import Check from 'lucide-svelte/icons/check';
  import AgentAvatar from '$lib/components/AgentAvatar.svelte';
  import type { DecisionChoice, DecisionRow, Segment } from '$lib/chat/decision';

  let {
    who = null,
    icon,
    line = [],
    title,
    action = '',
    reason = '',
    notes,
    body,
    rows = [],
    choices = [],
    multi = false,
    busy = false,
    error = '',
    onchoose,
    onstop,
    children
  }: {
    /** The asking employee, drawn as its avatar. */
    who?: { id: string; name: string; color?: string | null; shape?: string | null } | null;
    /** The app's or plugin's icon, in place of an avatar. */
    icon?: Snippet;
    /** The header line: its bold segments are the key words. */
    line?: Segment[];
    /** The header in place of `line` (a question in its own markdown). */
    title?: Snippet;
    /** What it would do, bold, under the line. */
    action?: string;
    /** Why it asks, in the owner's language. */
    reason?: string;
    /** More lines under the reason (the task it was on, its own words). */
    notes?: Snippet;
    /** Between the header and the answers (a question's context, steps). */
    body?: Snippet;
    rows?: DecisionRow[];
    choices?: DecisionChoice[];
    /** The choices toggle (several may be picked) instead of answering. */
    multi?: boolean;
    busy?: boolean;
    error?: string;
    onchoose?: (id: string) => void;
    /** Stop the employee's work. */
    onstop?: () => void;
    /** Under the answers (his own words, Submit, Skip). */
    children?: Snippet;
  } = $props();
</script>

<div class="ask-card decision-card" data-testid="decision-card">
  {#if onstop}
    <button type="button" class="decision-stop" onclick={onstop}>{$t('common.stop')}</button>
  {/if}
  <div class="decision-head">
    {#if icon}
      <div class="decision-icon">{@render icon()}</div>
    {:else if who}
      <AgentAvatar id={who.id} name={who.name} color={who.color ?? null} shape={who.shape ?? null} size="md" />
    {/if}
    {#if title}
      <div class="decision-line" dir="auto">{@render title()}</div>
    {:else if line.length}
      <p class="decision-line" dir="auto">{#each line as seg}{#if seg.bold}<b>{seg.text}</b>{:else}{seg.text}{/if}{/each}</p>
    {/if}
    {#if action}<p class="decision-action" dir="auto">{action}</p>{/if}
    {#if reason}<p class="decision-reason" dir="auto">{reason}</p>{/if}
    {#if notes}{@render notes()}{/if}
  </div>
  {#if body}{@render body()}{/if}
  {#if rows.length}
    <details class="decision-details">
      <summary class="decision-details-toggle">{$t('permissionAsk.showDetails')}</summary>
      <dl class="decision-rows">
        {#each rows as row, i (i)}
          <div class="decision-row">
            <dt class="decision-row-label">{row.label}</dt>
            <dd class="decision-row-value" class:decision-row-code={row.code} dir="auto">{row.value}</dd>
          </div>
        {/each}
      </dl>
    </details>
  {/if}
  {#if choices.length}
    <div class="decision-choices" role={multi ? 'group' : undefined}>
      {#each choices as c (c.id)}
        <button
          type="button"
          class="decision-choice"
          class:decision-choice-filled={c.tone === 'filled'}
          class:decision-choice-outlined={c.tone === 'outlined'}
          class:decision-choice-on={c.on}
          role={multi ? 'checkbox' : undefined}
          aria-checked={multi ? !!c.on : undefined}
          disabled={busy || c.disabled || c.working}
          onclick={() => onchoose?.(c.id)}
        >
          <span class="decision-choice-text">
            <span class="decision-choice-label" dir="auto">
              {#if c.working}<span class="loading loading-spinner loading-xs"></span>{/if}
              {c.label}
              {#if c.badge}<span class="decision-choice-badge">{c.badge}</span>{/if}
            </span>
            {#if c.description}<span class="decision-choice-description" dir="auto">{c.description}</span>{/if}
          </span>
          {#if c.on}<Check class="decision-choice-mark" />{/if}
        </button>
        {#if c.note}<p class="decision-choice-note" dir="auto">{c.note}</p>{/if}
      {/each}
    </div>
  {/if}
  {#if children}{@render children()}{/if}
  {#if error}<p class="decision-error">{error}</p>{/if}
</div>
