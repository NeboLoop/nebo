<!--
  VerificationHistory — every step of a verification, in order, as it goes:
  each review round (its number and time, what changed since the round
  before, each reviewer with its state and what it found, the project's own
  checks with their state, time and, when one failed, its last lines of
  output, what was fixed and what was checked and held), then catching up
  with the owner's project, the landing and the live checks.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import type { RoundView, VerificationView } from '$lib/api/neboComponents';
  import { duration, lensWord, severityWord, stepWord, whyWords, type Why } from '$lib/stores/verifications';

  let { verification }: { verification: VerificationView } = $props();

  type Finding = { id: string; severity: string; lens?: string; kind?: string; title: string; evidence: string };
  type Held = { checked: string };

  const list = <T,>(v: unknown): T[] => (Array.isArray(v) ? (v as T[]) : []);
  const bySeverity = (r: RoundView, s: string) => list<Finding>(r.findings).filter((f) => f.severity === s).length;
  const seconds = (r: RoundView) => (r.finishedAt ? Math.max(0, r.finishedAt - r.startedAt) : null);
  const result = (r: RoundView) => whyWords(r.result as Why | null | undefined, $t);
  const ofLens = (r: RoundView, lens: string) => list<Finding>(r.findings).filter((f) => f.lens === lens);
  const reviewerState = (state: string, count: number) =>
    state === 'found'
      ? $t('verify.reviewer.found', { values: { count } })
      : $t(['waiting', 'running', 'clean', 'failed'].includes(state) ? `verify.reviewer.${state}` : 'verify.reviewer.running');
</script>

<ol class="verify-history">
  {#each verification.history as r (r.round)}
    <li class="verify-history-step">
      <div class="verify-history-head">
        <span class="verify-history-title">
          {#if r.kind === 'review'}
            {$t('verify.round.title', { values: { n: verification.history.filter((s) => s.kind === 'review' && s.round <= r.round).length } })}
          {:else}
            {stepWord(r.kind, $t)}
          {/if}
        </span>
        {#if seconds(r) !== null}<span class="verify-history-meta">{duration(seconds(r) ?? 0, $t)}</span>{/if}
      </div>
      {#if r.kind === 'review'}
        {#if r.files.length && verification.history.findIndex((s) => s.kind === 'review') !== verification.history.indexOf(r)}
          <div class="verify-history-label">{$t('verify.round.since')}</div>
          <ul class="verify-files">
            {#each r.files as f (f.path)}
              <li class="verify-file">
                <span class="verify-file-path">{f.path}</span>
                <span class="verify-file-added">+{f.added}</span>
                <span class="verify-file-removed">−{f.removed}</span>
              </li>
            {/each}
          </ul>
        {/if}
        {#if r.checkRuns.length}
          <div class="verify-history-label">{$t('verify.round.projectChecks')}</div>
          {#each r.checkRuns as c, i (i)}
            <div class="verify-run verify-run-{c.state}">
              <span class="verify-run-label">{c.label}</span>
              <span class="verify-run-state">{$t(`verify.check.${c.state}`)}</span>
              {#if c.durationMs}<span class="verify-history-meta">{duration(c.durationMs / 1000, $t)}</span>{/if}
            </div>
            {#if c.tail}<pre class="verify-run-tail">{c.tail}</pre>{/if}
          {/each}
        {:else if seconds(r) !== null}
          <div class="verify-history-line">{$t('verify.round.noChecks')}</div>
        {/if}
        {#if r.reviewers.length}
          <div class="verify-history-label">{$t('verify.round.reviewers')}</div>
          {#each r.reviewers as rv (rv.lens)}
            <div class="verify-reviewer verify-reviewer-{rv.state}">
              <span class="verify-reviewer-lens">{lensWord(rv.lens, $t)}</span>
              <span class="verify-reviewer-state">{reviewerState(rv.state, rv.findings)}</span>
            </div>
            {#each ofLens(r, rv.lens) as f (f.id)}
              <div class="verify-finding verify-finding-{f.severity}">
                <span class="verify-finding-severity">{severityWord(f.severity, $t)}</span>
                <span class="verify-finding-title">{f.title}</span>
                {#if f.evidence?.trim()}<span class="verify-finding-where">{f.evidence}</span>{/if}
              </div>
            {/each}
          {/each}
        {/if}
        {#if seconds(r) !== null}
          <div class="verify-history-line">
            {$t('verify.round.findings', { values: { blockers: bySeverity(r, 'blocker'), majors: bySeverity(r, 'major'), minors: bySeverity(r, 'minor') } })}
          </div>
        {/if}
        {#if list<string>(r.fixed).length}
          <div class="verify-history-line">{$t('verify.round.fixed', { values: { count: list<string>(r.fixed).length } })}</div>
        {/if}
        {#if list<Held>(r.verifiedOk).length}
          <div class="verify-history-line verify-history-held">{$t('verify.round.held')}</div>
          {#each list<Held>(r.verifiedOk) as h, i (i)}
            <div class="verify-held">✓ {h.checked}</div>
          {/each}
        {/if}
      {:else if r.checkRuns.length}
        {#each r.checkRuns as c, i (i)}
          <div class="verify-run verify-run-{c.state}">
            <span class="verify-run-label">{c.label}</span>
            <span class="verify-run-state">{$t(`verify.check.${c.state}`)}</span>
          </div>
        {/each}
      {/if}
      {#if result(r)}
        <div class="verify-history-line">{result(r)}</div>
      {/if}
    </li>
  {/each}
</ol>
