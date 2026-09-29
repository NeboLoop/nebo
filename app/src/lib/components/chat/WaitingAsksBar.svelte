<!--
  WaitingAsksBar — what waits on the owner's answer, pinned at the top of the
  chat he is in, from every employee: who is waiting, on what, and its
  answers to tap. A question in another conversation opens there. Answered
  anywhere — here, on the phone, out loud on a call — it leaves everywhere.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import type { WaitingAsk } from '$lib/api/neboComponents';
  import { waitingAsks, answerWaiting, askPath } from '$lib/stores/waitingAsks';
  import { goto } from '$lib/nav';

  let { sessionKey = '' }: { sessionKey?: string } = $props();

  // This chat's own permission asks keep their card above the composer.
  const shown = $derived($waitingAsks.filter((a) => !(a.kind === 'permission' && a.sessionKey === sessionKey)));

  let busy = $state<string | null>(null);
  let failed = $state<string | null>(null);

  async function answer(ask: WaitingAsk, i: number) {
    if (busy) return;
    busy = ask.id;
    failed = null;
    try {
      await answerWaiting(ask, i);
    } catch {
      failed = ask.id;
    } finally {
      busy = null;
    }
  }
</script>

{#if shown.length > 0}
  <div class="waiting-asks-bar" role="region" aria-label={$t('chat.waitingAsksLabel')}>
    {#each shown as ask (ask.id)}
      <div class="waiting-ask">
        <p class="waiting-ask-line">
          <span class="waiting-ask-who">{$t('chat.waitingOnYourAnswer', { values: { name: ask.employee } })}</span>
          {ask.question}
        </p>
        <div class="waiting-ask-actions">
          {#if ask.answerable}
            {#each ask.options as option, i (option)}
              <button type="button" class="btn btn-xs rounded-full {i === 0 ? 'btn-primary' : 'btn-ghost'}" disabled={busy === ask.id} onclick={() => answer(ask, i)}>{option}</button>
            {/each}
          {/if}
          {#if ask.sessionKey !== sessionKey}
            <button type="button" class="btn btn-xs btn-ghost rounded-full" onclick={() => goto(askPath(ask))}>{$t('chat.waitingOpen')}</button>
          {/if}
        </div>
        {#if failed === ask.id}
          <p class="waiting-ask-error">{$t('permissionAsk.failed')}</p>
        {/if}
      </div>
    {/each}
  </div>
{/if}
