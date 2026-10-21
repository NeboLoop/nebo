<!--
  VerifiedBadge — the badge on code work: Verified, Not verified, Checking or
  Waiting on you, with the one sentence that says what the record holds
  (never more). Opening it shows every round of its history.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import type { VerificationView } from '$lib/api/neboComponents';
  import { badgeWords } from '$lib/stores/verifications';
  import VerificationHistory from './VerificationHistory.svelte';

  let { verification }: { verification: VerificationView } = $props();

  let open = $state(false);
  const words = $derived(badgeWords(verification, $t));
  const detailId = $derived(`verify-detail-${verification.id}`);
</script>

<div class="verify-badge-wrap">
  <button
    type="button"
    class="verify-badge verify-badge-{verification.badge.state}"
    title={words.detail}
    aria-expanded={open}
    aria-controls={detailId}
    onclick={() => (open = !open)}
  >
    {#if verification.badge.state === 'verified'}
      <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/><polyline points="9 12 11 14 15 10"/></svg>
    {:else if verification.badge.state === 'not_verified'}
      <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/><line x1="12" y1="8" x2="12" y2="12"/><line x1="12" y1="16" x2="12.01" y2="16"/></svg>
    {:else}
      <span class="loading loading-spinner loading-xs" aria-hidden="true"></span>
    {/if}
    <span>{words.label}</span>
  </button>
  {#if open}
    <div class="verify-badge-detail" id={detailId}>
      <p class="verify-badge-sentence">{words.detail}</p>
      <VerificationHistory {verification} />
    </div>
  {/if}
</div>
