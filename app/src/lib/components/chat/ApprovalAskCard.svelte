<!--
  ApprovalAskCard — an approval a run waits on (a gated call, a suggested
  goal, a coworker's call), on the ONE decision card (DecisionCard), the
  same card as a permission ask: in the composer's place in the chat that
  raised it, never a dialog over the screen. Allow once · Always allow ·
  Decline, its facts and the call itself folded behind "Show details";
  answered anywhere, it collapses in place to its muted one-line receipt
  (AskReceipt).
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import { answerApproval, type Approval } from '$lib/stores/approvals';
  import DecisionCard from '$lib/components/DecisionCard.svelte';
  import { approvalAction, approvalAgentId, approvalChoices, approvalRows, emphasized } from '$lib/chat/decision';

  import AskReceipt from './AskReceipt.svelte';

  let { approval, onstop }: { approval: Approval; onstop?: () => void } = $props();
  const title = $derived($t('permissionAsk.title', { values: { name: approval.agent } }));
  const action = $derived(approvalAction(approval, $t));
  const rows = $derived(approvalRows(approval, $t));
</script>

{#if !approval.decision}
  <DecisionCard
    who={{ id: approvalAgentId(approval), name: approval.agent }}
    line={emphasized($t, 'permissionAsk.title', { name: approval.agent })}
    {action}
    {rows}
    choices={approvalChoices($t)}
    onchoose={(id) => answerApproval(approval.requestId, id as 'once' | 'always' | 'deny')}
    {onstop}
  />
{:else}
  <AskReceipt
    lead={$t(approval.decision === 'deny' ? 'permissionAsk.declined' : approval.decision === 'always' ? 'permissionAsk.allowedAlways' : 'permissionAsk.allowedOnce')}
    subject={approval.headline || title}
  >
    <p class="permission-ask-title">{title}</p>
    {#if action}
      <p class="permission-ask-sentence">{action}</p>
    {/if}
    {#each rows as row, i (i)}
      <div class="approval-ask-fact">
        <span class="approval-ask-fact-label">{row.label}</span>
        <span class="approval-ask-fact-value">{row.value}</span>
      </div>
    {/each}
  </AskReceipt>
{/if}
