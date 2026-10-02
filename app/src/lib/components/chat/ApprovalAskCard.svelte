<!--
  ApprovalAskCard — an approval a run waits on (a gated call, a suggested
  goal, a coworker's call), as a card in the chat that raised it: the same
  card as a permission ask, at the bottom beside the composer, never a dialog
  over the screen. Allow always · This once · No; answered anywhere, it
  collapses in place to its receipt.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import { answerApproval, type Approval } from '$lib/stores/approvals';

  let { approval }: { approval: Approval } = $props();
</script>

<div class="permission-ask-card">
  <p class="permission-ask-title">{$t('permissionAsk.title', { values: { name: approval.agent } })}</p>
  {#if approval.headline}
    <p class="permission-ask-sentence">{approval.headline}</p>
  {/if}
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
  {#if !approval.decision}
    <div class="permission-ask-actions">
      <button type="button" class="btn btn-primary btn-sm rounded-full" onclick={() => answerApproval(approval.requestId, 'always')}>{$t('permissionAsk.allowAlways')}</button>
      <button type="button" class="btn btn-sm rounded-full" onclick={() => answerApproval(approval.requestId, 'once')}>{$t('permissionAsk.thisOnce')}</button>
      <button type="button" class="btn btn-ghost btn-sm rounded-full" onclick={() => answerApproval(approval.requestId, 'deny')}>{$t('permissionAsk.no')}</button>
    </div>
  {:else if approval.decision === 'deny'}
    <div class="permission-ask-settled"><span class="badge badge-ghost badge-sm">{$t('permissionAsk.declined')}</span></div>
  {:else}
    <div class="permission-ask-settled"><span class="badge badge-primary badge-sm">{$t(approval.decision === 'always' ? 'permissionAsk.allowedAlways' : 'permissionAsk.allowedOnce')}</span></div>
  {/if}
</div>
