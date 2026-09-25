<!--
  PermissionAskCard — the ONE ask card. A single step of an employee's work
  waits on the owner: who wants to do what, why it asked, and three answers.
  The same card sits in the open chat, the Inbox and the dashboard; the first
  answer anywhere wins and `permission_ask_resolved` clears it everywhere.
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
  <p class="permission-ask-title">{$t('permissionAsk.title', { values: { name: shown.employee } })}</p>
  <p class="permission-ask-sentence">{sentence}. <span class="permission-ask-reason">{shown.reason}</span></p>
  {#if shown.status === 'open'}
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
    <p class="permission-ask-settled">{$t(shown.answer === 'allow_always' ? 'permissionAsk.allowedAlways' : 'permissionAsk.allowedOnce')}</p>
  {:else if shown.status === 'declined'}
    <p class="permission-ask-settled">{$t('permissionAsk.declined')}</p>
  {:else}
    <p class="permission-ask-settled">{$t('permissionAsk.withdrawn')}</p>
  {/if}
  {#if failed}
    <p class="permission-ask-error">{$t('permissionAsk.failed')}</p>
  {/if}
</div>
