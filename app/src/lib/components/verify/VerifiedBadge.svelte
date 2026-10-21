<!--
  VerifiedBadge — code work in the conversation: one line, the size of a tool
  row ("Checking · round 2 ›", then "Verified ›" or "2 issues left ›"). While
  it is checked it sits on the status line above the composer, beside the
  running tasks (`statusLine`); finished, on the reply that reported it. Its
  detail lives in the sheet it opens: what is being checked, what the loop is
  doing now, every round with each reviewer and each of the project's checks
  as they go, how long it has taken, and how it ended.
-->
<script lang="ts">
  import { onDestroy } from 'svelte';
  import { t } from 'svelte-i18n';
  import type { VerificationView } from '$lib/api/neboComponents';
  import ShelfModal from '$lib/components/ui/ShelfModal.svelte';
  import { badgeWords, duration, elapsed, lineWords, phaseWords, whyWords, type Why } from '$lib/stores/verifications';
  import VerificationHistory from './VerificationHistory.svelte';

  let { verification, statusLine = false }: { verification: VerificationView; statusLine?: boolean } = $props();

  let open = $state(false);
  let now = $state(Math.floor(Date.now() / 1000));
  const live = $derived(verification.badge.state === 'checking' || verification.badge.state === 'waiting');
  const words = $derived(badgeWords(verification, $t));
  const line = $derived(lineWords(verification, $t));
  const phase = $derived(phaseWords(verification, $t));
  const took = $derived(duration(elapsed(verification, now), $t));

  type Finding = { id: string; severity: string; title: string; evidence: string };
  const stillOpen = $derived(
    verification.badge.state === 'verified' ? [] : (Array.isArray(verification.open) ? (verification.open as Finding[]) : []),
  );
  const added = $derived(verification.changed.reduce((n, f) => n + f.added, 0));
  const removed = $derived(verification.changed.reduce((n, f) => n + f.removed, 0));

  // The time so far ticks while the sheet is open on work still going.
  let tick: ReturnType<typeof setInterval> | null = null;
  $effect(() => {
    if (open && live && !tick) {
      now = Math.floor(Date.now() / 1000);
      tick = setInterval(() => (now = Math.floor(Date.now() / 1000)), 1000);
    }
    if ((!open || !live) && tick) {
      clearInterval(tick);
      tick = null;
    }
  });
  onDestroy(() => tick && clearInterval(tick));
</script>

<button
  type="button"
  class={statusLine ? 'bg-status-tasks verify-line-status' : `verify-line verify-line-${verification.badge.state}`}
  aria-haspopup="dialog"
  title={$t('verify.badge.openHint')}
  onclick={() => {
    now = Math.floor(Date.now() / 1000);
    open = true;
  }}
>
  <span class="verify-line-text {live ? 'activity-live' : ''}">{line}</span>
  <span class="verify-line-chevron">&rsaquo;</span>
</button>

<ShelfModal
  {open}
  title={words.label}
  subtitle={$t(live ? 'verify.sheet.elapsed' : 'verify.sheet.took', { values: { project: verification.badge.project, time: took } })}
  onclose={() => (open = false)}
>
  <div class="verify-sheet">
    {#if phase}<p class="verify-sheet-phase">{phase}</p>{/if}
    <p class="verify-badge-sentence">{words.detail}</p>
    {#if verification.badge.landed}
      <p class="verify-sheet-outcome">{whyWords({ code: 'landed', args: { commit: verification.badge.landed.slice(0, 10) } } as Why, $t)}</p>
    {/if}

    {#if verification.changed.length}
      <section class="verify-sheet-section">
        <h3 class="verify-sheet-heading">{$t('verify.sheet.changed')}</h3>
        <div class="verify-sheet-sub">
          {$t('verify.sheet.files', { values: { count: verification.changed.length, project: verification.badge.project } })}
          <span class="verify-file-added">+{added}</span> <span class="verify-file-removed">−{removed}</span>
        </div>
        <ul class="verify-files">
          {#each verification.changed as f (f.path)}
            <li class="verify-file">
              <span class="verify-file-path">{f.path}</span>
              <span class="verify-file-added">+{f.added}</span>
              <span class="verify-file-removed">−{f.removed}</span>
            </li>
          {/each}
        </ul>
      </section>
    {/if}

    <VerificationHistory {verification} />

    {#if stillOpen.length}
      <section class="verify-sheet-section">
        <h3 class="verify-sheet-heading">{$t('verify.sheet.open')}</h3>
        {#each stillOpen as f (f.id)}
          <div class="verify-finding verify-finding-{f.severity}">
            <span class="verify-finding-severity">{$t(['blocker', 'major', 'minor'].includes(f.severity) ? `verify.severity.${f.severity}` : 'verify.severity.other')}</span>
            <span class="verify-finding-title">{f.title}</span>
            {#if f.evidence?.trim()}<span class="verify-finding-where">{f.evidence}</span>{/if}
          </div>
        {/each}
      </section>
    {/if}
  </div>
</ShelfModal>
