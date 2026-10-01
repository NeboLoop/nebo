<!--
  PendingAsksStrip — one small line at the top of a chat when several of its
  asks wait on the owner: how many, a click to go to the oldest, Decline all,
  and a close. The asks themselves are cards in the conversation; this never
  holds them and never covers the composer. Live 2026-10-01: a bar holding
  every pending ask could not be closed, and the owner said No to 30-some
  asks one by one before he could use the chat.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import type { PermissionAskCard } from '$lib/api/neboComponents';
  import { answerAsk } from '$lib/stores/permissionAsks';

  let { asks }: { asks: PermissionAskCard[] } = $props();

  /** The count the owner closed the strip at; a new ask brings it back. */
  let closedAt = $state<number | null>(null);
  let busy = $state(false);
  let failed = $state(false);

  const shown = $derived(asks.length >= 2 && (closedAt === null || asks.length > closedAt));

  function openOldest() {
    const oldest = asks[0];
    if (!oldest) return;
    document.getElementById(askCardId(oldest.id))?.scrollIntoView({ behavior: 'smooth', block: 'center' });
  }

  /** Every ask that takes a No gets one, through the card's own answer. */
  async function declineAll() {
    if (busy) return;
    busy = true;
    failed = false;
    for (const ask of asks.filter((a) => a.kind !== 'send_check')) {
      try {
        await answerAsk(ask.id, 'no', 'chat');
      } catch {
        failed = true;
      }
    }
    busy = false;
  }
</script>

<script module lang="ts">
  /** The element id an ask's card carries in the conversation. */
  export function askCardId(id: string): string {
    return `ask-card-${id}`;
  }
</script>

{#if shown}
  <div class="pending-asks-strip" role="region" aria-label={$t('chat.waitingAsksLabel')}>
    <button type="button" class="pending-asks-count" onclick={openOldest}>
      {failed ? $t('chat.declineAllFailed') : $t('chat.decisionsWaiting', { values: { n: asks.length } })}
    </button>
    <button type="button" class="btn btn-ghost btn-xs rounded-full" disabled={busy} onclick={declineAll}>{$t('chat.declineAll')}</button>
    <button type="button" class="btn btn-ghost btn-xs btn-square" aria-label={$t('chat.closeStrip')} onclick={() => (closedAt = asks.length)}>✕</button>
  </div>
{/if}
