<!--
  PermissionAskCard — the ONE ask card. A single step of an employee's work
  waits on the owner: who wants to do what, why it asked, and three answers.
  A `send_check` card asks instead whether a send whose outcome never came
  back went out: It went out · It didn't go out.
  The same card sits in the chat whose own flow raised it, the Inbox and the
  dashboard; the first answer anywhere wins. The bot words it all: what it
  would do, why (the task it was on, and its own line), what about it needs
  the owner, a preview of what would go out under "Show details", and what
  "Allow always" would cover. Open, it is a tinted ask card
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
  const sentence = $derived(shown.what);
  const details = $derived(shown.details);
  const sendCheck = $derived(shown.kind === 'send_check');
  const title = $derived($t(sendCheck ? 'permissionAsk.sendCheckTitle' : 'permissionAsk.title', { values: { name: shown.employee } }));
  /** What about it needs the owner, in his language: the bot's key and
   *  values, its English line when it sends none (an older bot). */
  const reason = $derived(
    shown.reasonKey
      ? [$t(shown.reasonKey, { values: shown.reasonValues, default: shown.reason }), shown.warningKey ? $t(shown.warningKey) : '']
          .filter(Boolean)
          .join(' ')
      : shown.reason
  );
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
    {@render explained()}
    {#if sendCheck}
      <div class="permission-ask-actions">
        <button type="button" class="btn btn-primary btn-sm rounded-full" disabled={busy} onclick={() => answer('sent')}>{$t('permissionAsk.sent')}</button>
        <button type="button" class="btn btn-ghost btn-sm rounded-full" disabled={busy} onclick={() => answer('not_sent')}>{$t('permissionAsk.notSent')}</button>
      </div>
    {:else}
      <div class="permission-ask-actions">
        {#if shown.allowAlways}
          <button type="button" class="btn btn-primary btn-sm rounded-full" disabled={busy} title={shown.allowAlwaysCovers} onclick={() => answer('allow_always')}>{$t('permissionAsk.allowAlways')}</button>
        {/if}
        {#if shown.thisOnce}
          <button type="button" class="btn btn-sm rounded-full" disabled={busy} onclick={() => answer('this_once')}>{$t('permissionAsk.thisOnce')}</button>
        {/if}
        <button type="button" class="btn btn-ghost btn-sm rounded-full" disabled={busy} onclick={() => answer('no')}>{$t('permissionAsk.no')}</button>
      </div>
    {/if}
    {#if shown.allowAlways && shown.allowAlwaysCovers}
      <p class="permission-ask-covers">{shown.allowAlwaysCovers}</p>
    {/if}
    {#if failed}
      <p class="permission-ask-error">{$t('permissionAsk.failed')}</p>
    {/if}
  </div>
{:else}
  <AskReceipt lead={$t(receipt)} subject={sentence}>
    {@render explained()}
  </AskReceipt>
{/if}

<!-- Who wants to do what, why, what about it needs the owner, and what
     would go out. -->
{#snippet explained()}
  <p class="permission-ask-title">{title}</p>
  <p class="permission-ask-sentence">{sentence}</p>
  {#if shown.why}
    <p class="permission-ask-why"><span class="permission-ask-label">{$t('permissionAsk.why')}</span> {shown.why}</p>
  {/if}
  {#if shown.statedReason}
    <p class="permission-ask-why"><span class="permission-ask-label">{$t('permissionAsk.saidBy', { values: { name: shown.employee } })}</span> {shown.statedReason}</p>
  {/if}
  <p class="permission-ask-reason">{reason}</p>
  {#if details.length}
    <details class="permission-ask-details">
      <summary class="approval-ask-more">{$t('permissionAsk.showDetails')}</summary>
      <div class="approval-ask-facts">
        {#each details as d (d.kind)}
          <div class="approval-ask-fact">
            <span class="approval-ask-fact-label permission-ask-detail-label">{$t(`permissionAsk.detail.${d.kind}`)}</span>
            <span class="permission-ask-detail-value">{d.value}</span>
          </div>
        {/each}
      </div>
    </details>
  {:else if shown.command}
    <details class="permission-ask-command"><summary class="permission-ask-command-line">{shown.command}</summary></details>
  {/if}
{/snippet}
