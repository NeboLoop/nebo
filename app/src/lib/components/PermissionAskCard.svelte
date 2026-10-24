<!--
  PermissionAskCard — a single step of an employee's work waits on the
  owner: who wants to do what, why it asked, and its answers, on the ONE
  decision card (DecisionCard): Allow once · Always allow (with what
  "always" covers; never on a critical target) · Decline. A `send_check`
  card asks instead whether a send whose outcome never came back went out:
  It went out · It didn't go out.
  The same card sits in the composer's place in the chat whose own flow
  raised it, the Inbox and the dashboard; the first answer anywhere wins.
  The bot words it all: what it would do, why (the task it was on, and its
  own line), what about it needs the owner, and what would go out under
  "Show details" with the command and its one-line risk. Answered, it
  collapses in place to a muted one-line receipt: Allowed · always, Allowed
  · this once, Declined, or No longer needed, followed by what was asked.
  Tapping the receipt shows the ask read-only.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import type { PermissionAskCard } from '$lib/api/neboComponents';
  import { answerAsk, type AskAnswer } from '$lib/stores/permissionAsks';
  import AskReceipt from '$lib/components/chat/AskReceipt.svelte';
  import DecisionCard from '$lib/components/DecisionCard.svelte';
  import { emphasized, permissionChoices, permissionReason, permissionRows } from '$lib/chat/decision';

  let { ask, via, onstop }: { ask: PermissionAskCard; via: 'chat' | 'inbox'; onstop?: () => void } = $props();

  let busy = $state(false);
  let failed = $state(false);
  let settled = $state<PermissionAskCard | null>(null);
  const shown = $derived(settled ?? ask);
  const sentence = $derived(shown.what);
  const sendCheck = $derived(shown.kind === 'send_check');
  /** Who wants it, the name in bold. */
  const line = $derived(emphasized($t, sendCheck ? 'permissionAsk.sendCheckTitle' : 'permissionAsk.title', { name: shown.employee }));
  /** What about it needs the owner, in his language. */
  const reason = $derived(permissionReason(shown, $t));
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
  <DecisionCard
    who={{ id: shown.agentId, name: shown.employee }}
    {line}
    action={sentence}
    {reason}
    {notes}
    rows={permissionRows(shown, $t)}
    choices={permissionChoices(shown, $t)}
    {busy}
    error={failed ? $t('permissionAsk.failed') : ''}
    onchoose={(id) => answer(id as AskAnswer)}
    {onstop}
  />
{:else}
  <AskReceipt lead={$t(receipt)} subject={sentence}>
    <p class="permission-ask-title">{line.map((s) => s.text).join('')}</p>
    <p class="permission-ask-sentence">{sentence}</p>
    {@render notes()}
    <p class="permission-ask-reason">{reason}</p>
    {#each permissionRows(shown, $t) as row, i (i)}
      <div class="approval-ask-fact">
        <span class="approval-ask-fact-label">{row.label}</span>
        <span class="permission-ask-detail-value">{row.value}</span>
      </div>
    {/each}
  </AskReceipt>
{/if}

<!-- The task it was on, and its own line about this call. -->
{#snippet notes()}
  {#if shown.why}
    <p class="permission-ask-why"><span class="permission-ask-label">{$t('permissionAsk.why')}</span> {shown.why}</p>
  {/if}
  {#if shown.statedReason}
    <p class="permission-ask-why"><span class="permission-ask-label">{$t('permissionAsk.saidBy', { values: { name: shown.employee } })}</span> {shown.statedReason}</p>
  {/if}
{/snippet}
