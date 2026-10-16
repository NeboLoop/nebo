<!--
  HandoffLine — one hand-off in the sender's chat, in the one-line call
  format: "→ {Employee}: {ask}", and under it what is happening to it now —
  working, done with what came back, failed with why, stopped. Clicking it
  opens the receiving employee's work. A failure stays visible.

  With `card` it is a project's card in the project's thread: "{Employee}:
  {task}" with where it stands as a chip (handed out, working, done,
  couldn't finish, stopped) and, under it, when it counts as done. What came
  back is the member's own next line in the thread, so the card does not
  repeat it.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import { page } from '$app/stores';
  import { cardState, handoffs, trackHandoffs, receiverHref, statusLine } from '$lib/stores/handoffs';

  let {
    to,
    ask,
    handoffId = '',
    fallbackHref,
    card = false,
    doneWhen = '',
  }: {
    /** The receiving employee's name, as the call's receipt named it. */
    to: string;
    /** What was asked, as sent. */
    ask: string;
    /** The hand-off record the call made (absent on calls from before it). */
    handoffId?: string;
    /** Where it opens when there is no record to read. */
    fallbackHref?: string;
    /** A project's card: the task with its state chip and `doneWhen`. */
    card?: boolean;
    /** When the card's work counts as done ('' when the lead gave none). */
    doneWhen?: string;
  } = $props();

  $effect(() => {
    if (handoffId) trackHandoffs([handoffId]);
  });
  const h = $derived(handoffId ? ($handoffs[handoffId] ?? null) : null);
  const href = $derived(h ? receiverHref(h, $page.url) : fallbackHref);
  const status = $derived(h ? statusLine(h, $t) : '');
  const oneLine = (s: string) => s.replace(/\s+/g, ' ').trim();
  const line = $derived($t('handoff.toAsk', { values: { name: h?.toName || to, ask: oneLine(h?.ask || ask) } }));
  // A card's task stays the lead's words; its state shows once it is read.
  const task = $derived($t('handoff.cardTask', { values: { name: h?.toName || to, task: oneLine(ask) } }));
  const state = $derived(h ? cardState(h) : null);
</script>

{#if card}
<a
  {href}
  class="handoff-card {href ? 'handoff-line-link' : ''}"
  title={href ? $t('handoff.openReceiver', { values: { name: h?.toName || to } }) : undefined}
>
  <span class="handoff-card-head">
    <span class="handoff-card-task">{task}</span>
    {#if state}
      <span class="handoff-card-state handoff-card-state-{state}">{$t(`handoff.card.${state}`)}</span>
    {/if}
  </span>
  {#if doneWhen}
    <span class="handoff-card-when">{$t('handoff.doneWhen', { values: { when: doneWhen } })}</span>
  {/if}
</a>
{:else}
<a
  {href}
  class="handoff-line {href ? 'handoff-line-link' : ''}"
  title={href ? $t('handoff.openReceiver', { values: { name: h?.toName || to } }) : undefined}
>
  <span class="handoff-line-ask">{line}</span>
  {#if status}
    <span class="handoff-line-status handoff-status handoff-status-{h?.status}"><span class="shrink-0" aria-hidden="true">⎿</span><span class="min-w-0 truncate">{status}</span></span>
  {/if}
</a>
{/if}
