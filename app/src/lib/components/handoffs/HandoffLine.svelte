<!--
  HandoffLine — one hand-off in the sender's chat, in the one-line call
  format: "→ {Employee}: {ask}", and under it what is happening to it now —
  working, done with what came back, failed with why, stopped. Clicking it
  opens the receiving employee's work. A failure stays visible.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import { page } from '$app/stores';
  import { handoffs, trackHandoffs, receiverHref, statusLine } from '$lib/stores/handoffs';

  let {
    to,
    ask,
    handoffId = '',
    fallbackHref,
  }: {
    /** The receiving employee's name, as the call's receipt named it. */
    to: string;
    /** What was asked, as sent. */
    ask: string;
    /** The hand-off record the call made (absent on calls from before it). */
    handoffId?: string;
    /** Where it opens when there is no record to read. */
    fallbackHref?: string;
  } = $props();

  $effect(() => {
    if (handoffId) trackHandoffs([handoffId]);
  });
  const h = $derived(handoffId ? ($handoffs[handoffId] ?? null) : null);
  const href = $derived(h ? receiverHref(h, $page.url) : fallbackHref);
  const status = $derived(h ? statusLine(h, $t) : '');
  const oneLine = (s: string) => s.replace(/\s+/g, ' ').trim();
  const line = $derived($t('handoff.toAsk', { values: { name: h?.toName || to, ask: oneLine(h?.ask || ask) } }));
</script>

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
