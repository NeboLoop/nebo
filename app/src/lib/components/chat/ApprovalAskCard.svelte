<!--
  ApprovalAskCard — an approval a run waits on (a gated call, a suggested
  goal, a coworker's call), as a card in the chat that raised it: the same
  card as a permission ask, at the bottom beside the composer, never a dialog
  over the screen. Allow always · This once · No; answered anywhere, it
  collapses in place to its muted one-line receipt (AskReceipt).
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import { answerApproval, type Approval } from '$lib/stores/approvals';

  import AskReceipt from './AskReceipt.svelte';

  let { approval }: { approval: Approval } = $props();
  const title = $derived($t('permissionAsk.title', { values: { name: approval.agent } }));
</script>

{#snippet facts()}
  {#if approval.detailRows && approval.detailRows.length > 0}
    <div class="approval-ask-facts">
      {#each approval.detailRows as row (row.label)}
        <div class="approval-ask-fact">
          <span class="approval-ask-fact-label">{row.label}</span>
          <span class="approval-ask-fact-value">{row.value}</span>
        </div>
      {/each}
    </div>
  {/if}
  {#if approval.actionDetail}
    {#if approval.headline}
      <details>
        <summary class="approval-ask-more">{$t('components.approvalModal.technicalDetails')}</summary>
        <p class="approval-ask-detail">{approval.actionDetail}</p>
      </details>
    {:else}
      <p class="approval-ask-detail">{approval.actionDetail}</p>
    {/if}
  {/if}
{/snippet}

{#if !approval.decision}
  <div class="ask-card permission-ask-card">
    <p class="permission-ask-title">{title}</p>
    {#if approval.headline}
      <p class="permission-ask-sentence">{approval.headline}</p>
    {/if}
    {@render facts()}
    <div class="permission-ask-actions">
      <button type="button" class="btn btn-primary btn-sm rounded-full" onclick={() => answerApproval(approval.requestId, 'always')}>{$t('permissionAsk.allowAlways')}</button>
      <button type="button" class="btn btn-sm rounded-full" onclick={() => answerApproval(approval.requestId, 'once')}>{$t('permissionAsk.thisOnce')}</button>
      <button type="button" class="btn btn-ghost btn-sm rounded-full" onclick={() => answerApproval(approval.requestId, 'deny')}>{$t('permissionAsk.no')}</button>
    </div>
  </div>
{:else}
  <AskReceipt
    lead={$t(approval.decision === 'deny' ? 'permissionAsk.declined' : approval.decision === 'always' ? 'permissionAsk.allowedAlways' : 'permissionAsk.allowedOnce')}
    subject={approval.headline || title}
  >
    <p class="permission-ask-title">{title}</p>
    {#if approval.headline}
      <p class="permission-ask-sentence">{approval.headline}</p>
    {/if}
    {@render facts()}
  </AskReceipt>
{/if}
