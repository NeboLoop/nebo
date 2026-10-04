<!--
  PermissionAskCard — the ONE ask card. A single step of an employee's work
  waits on the owner: who wants to do what, why it asked, and three answers.
  A `send_check` card asks instead whether a send whose outcome never came
  back went out: It went out · It didn't go out.
  The same card sits in the chat whose own flow raised it, the Inbox and the
  dashboard; the first answer anywhere wins. A shell command's exact words
  sit under the sentence on one line, in full when tapped. Open, it is a tinted ask card
  (.ask-card); answered, it collapses in place to a muted one-line receipt:
  Allowed · always, Allowed · this once, Declined, or No longer needed,
  followed by what was asked. Tapping the receipt shows the ask read-only.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import type { PermissionAskCard } from '$lib/api/neboComponents';
  import { answerAsk, type AskAnswer } from '$lib/stores/permissionAsks';
  import AskReceipt from '$lib/components/chat/AskReceipt.svelte';

  let { ask, via }: { ask: PermissionAskCard; via: 'chat' | 'inbox' } = $props();

  let busy = $state(false);
  let failed = $state(false);
  let settled = $state<PermissionAskCard | null>(null);
  const shown = $derived(settled ?? ask);
  const sentence = $derived(shown.sentence.charAt(0).toUpperCase() + shown.sentence.slice(1));
  const sendCheck = $derived(shown.kind === 'send_check');
  const title = $derived($t(sendCheck ? 'permissionAsk.sendCheckTitle' : 'permissionAsk.title', { values: { name: shown.employee } }));
  /** How a settled ask ended, as its receipt's lead. */
  const receipt = $derived(
    shown.status === 'answered'
      ? shown.answer === 'sent' ? 'permissionAsk.sent' : 'permissionAsk.notSent'
      : shown.status === 'allowed'
        ? shown.answer === 'allow_always' ? 'permissionAsk.allowedAlways' : 'permissionAsk.allowedOnce'
        : shown.status === 'declined' ? 'permissionAsk.declined' : 'permissionAsk.withdrawn'
  );

  async function answer(a: AskAnswer) {
    if (busy) return;
    busy = true;
    failed = false;
    try {
      settled = await answerAsk(ask.id, a, via);
    } catch {
      failed = true;
    } finally {
      busy = false;
    }
  }
</script>

{#if shown.status === 'open'}
  <div class="ask-card permission-ask-card">
    <p class="permission-ask-title">{title}</p>
    <p class="permission-ask-sentence">{sentence}. <span class="permission-ask-reason">{shown.reason}</span></p>
    {#if shown.command}
      <details class="permission-ask-command"><summary class="permission-ask-command-line">{shown.command}</summary></details>
    {/if}
    {#if sendCheck}
      <div class="permission-ask-actions">
        <button type="button" class="btn btn-primary btn-sm rounded-full" disabled={busy} onclick={() => answer('sent')}>{$t('permissionAsk.sent')}</button>
        <button type="button" class="btn btn-ghost btn-sm rounded-full" disabled={busy} onclick={() => answer('not_sent')}>{$t('permissionAsk.notSent')}</button>
      </div>
    {:else}
      <div class="permission-ask-actions">
        {#if shown.allowAlways}
          <button type="button" class="btn btn-primary btn-sm rounded-full" disabled={busy} onclick={() => answer('allow_always')}>{$t('permissionAsk.allowAlways')}</button>
        {/if}
        {#if shown.thisOnce}
          <button type="button" class="btn btn-sm rounded-full" disabled={busy} onclick={() => answer('this_once')}>{$t('permissionAsk.thisOnce')}</button>
        {/if}
        <button type="button" class="btn btn-ghost btn-sm rounded-full" disabled={busy} onclick={() => answer('no')}>{$t('permissionAsk.no')}</button>
      </div>
    {/if}
    {#if failed}
      <p class="permission-ask-error">{$t('permissionAsk.failed')}</p>
    {/if}
  </div>
{:else}
  <AskReceipt lead={$t(receipt)} subject={sentence}>
    <p class="permission-ask-title">{title}</p>
    <p class="permission-ask-sentence">{sentence}. <span class="permission-ask-reason">{shown.reason}</span></p>
  </AskReceipt>
{/if}
