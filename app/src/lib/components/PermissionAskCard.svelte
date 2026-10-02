<!--
  PermissionAskCard — the ONE ask card. A single step of an employee's work
  waits on the owner: who wants to do what, why it asked, and three answers.
  A `send_check` card asks instead whether a send whose outcome never came
  back went out: It went out · It didn't go out.
  The same card sits in the chat whose own flow raised it, the Inbox and the
  dashboard; the first answer anywhere wins. Answered, it collapses to a
  one-line receipt: Allowed · always, Allowed · this once, Declined, or No
  longer needed.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import type { PermissionAskCard } from '$lib/api/neboComponents';
  import { answerAsk, type AskAnswer } from '$lib/stores/permissionAsks';

  let { ask, via }: { ask: PermissionAskCard; via: 'chat' | 'inbox' } = $props();

  let busy = $state(false);
  let failed = $state(false);
  let settled = $state<PermissionAskCard | null>(null);
  const shown = $derived(settled ?? ask);
  const sentence = $derived(shown.sentence.charAt(0).toUpperCase() + shown.sentence.slice(1));
  const sendCheck = $derived(shown.kind === 'send_check');

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

<div class="permission-ask-card">
  <p class="permission-ask-title">{$t(sendCheck ? 'permissionAsk.sendCheckTitle' : 'permissionAsk.title', { values: { name: shown.employee } })}</p>
  <p class="permission-ask-sentence">{sentence}. <span class="permission-ask-reason">{shown.reason}</span></p>
  {#if shown.status === 'open' && sendCheck}
    <div class="permission-ask-actions">
      <button type="button" class="btn btn-primary btn-sm rounded-full" disabled={busy} onclick={() => answer('sent')}>{$t('permissionAsk.sent')}</button>
      <button type="button" class="btn btn-ghost btn-sm rounded-full" disabled={busy} onclick={() => answer('not_sent')}>{$t('permissionAsk.notSent')}</button>
    </div>
  {:else if shown.status === 'answered'}
    <div class="permission-ask-settled"><span class="badge badge-primary badge-sm">{$t(shown.answer === 'sent' ? 'permissionAsk.sent' : 'permissionAsk.notSent')}</span></div>
  {:else if shown.status === 'open'}
    <div class="permission-ask-actions">
      {#if shown.allowAlways}
        <button type="button" class="btn btn-primary btn-sm rounded-full" disabled={busy} onclick={() => answer('allow_always')}>{$t('permissionAsk.allowAlways')}</button>
      {/if}
      {#if shown.thisOnce}
        <button type="button" class="btn btn-sm rounded-full" disabled={busy} onclick={() => answer('this_once')}>{$t('permissionAsk.thisOnce')}</button>
      {/if}
      <button type="button" class="btn btn-ghost btn-sm rounded-full" disabled={busy} onclick={() => answer('no')}>{$t('permissionAsk.no')}</button>
    </div>
  {:else if shown.status === 'allowed'}
    <div class="permission-ask-settled"><span class="badge badge-primary badge-sm">{$t(shown.answer === 'allow_always' ? 'permissionAsk.allowedAlways' : 'permissionAsk.allowedOnce')}</span></div>
  {:else if shown.status === 'declined'}
    <div class="permission-ask-settled"><span class="badge badge-ghost badge-sm">{$t('permissionAsk.declined')}</span></div>
  {:else}
    <div class="permission-ask-settled"><span class="badge badge-ghost badge-sm">{$t('permissionAsk.withdrawn')}</span></div>
  {/if}
  {#if failed}
    <p class="permission-ask-error">{$t('permissionAsk.failed')}</p>
  {/if}
</div>
