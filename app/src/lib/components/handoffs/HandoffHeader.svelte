<!--
  HandoffHeader — the top line of a receiving employee's thread or case:
  who handed this work over and what they asked, with its status, and a way
  back to the conversation it came from. Reads the newest hand-off worked in
  `sessionKey`; renders nothing when no employee handed work into it.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import { goto } from '$lib/nav';
  import type { HandoffView } from '$lib/api/neboComponents';
  import { handoffs, handoffInto, statusLine } from '$lib/stores/handoffs';
  import { botName } from '$lib/stores/botName';

  let { sessionKey, onnavigate }: { sessionKey: string; onnavigate?: () => void } = $props();

  let id = $state('');
  $effect(() => {
    const key = sessionKey;
    id = '';
    if (key) void handoffInto(key).then((h) => { if (h && key === sessionKey) id = h.id; });
  });
  const h = $derived<HandoffView | null>(id ? ($handoffs[id] ?? null) : null);
  const fromName = $derived(h?.fromName || $botName || $t('common.agent'));
  const status = $derived(h ? statusLine(h, $t) : '');

  function back(e: MouseEvent) {
    if (!h?.senderLink) return;
    e.preventDefault();
    onnavigate?.();
    goto(h.senderLink);
  }
</script>

{#if h}
  <div class="handoff-header">
    <div class="handoff-header-line">
      <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="shrink-0"><path d="M19 12H5"/><path d="m12 19-7-7 7-7"/></svg>
      {#if h.senderLink}
        <a href={h.senderLink} class="handoff-header-from" title={$t('handoff.openSender', { values: { name: fromName } })} onclick={back}>
          {$t('handoff.fromAsk', { values: { name: fromName, ask: h.ask } })}
        </a>
      {:else}
        <span class="handoff-header-from">{$t('handoff.fromAsk', { values: { name: fromName, ask: h.ask } })}</span>
      {/if}
    </div>
    {#if status}
      <div class="handoff-status handoff-status-{h.status}">{status}</div>
    {/if}
  </div>
{/if}
