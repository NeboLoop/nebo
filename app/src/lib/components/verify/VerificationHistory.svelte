<!--
  VerificationHistory — every step of a verification, in order: each review
  round (its number, its reviewers' lenses, its findings by severity, what
  was fixed, what was checked and held, the project's checks, its time and
  cost), then catching up with the owner's project, the landing and the
  live checks.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import type { RoundView, VerificationView } from '$lib/api/neboComponents';
  import { cost, lensWord, severityWord, stepWord, whyWords, type Why } from '$lib/stores/verifications';

  let { verification }: { verification: VerificationView } = $props();

  type Finding = { id: string; severity: string; kind?: string; title: string; evidence: string };
  type Held = { checked: string };
  type CheckRun = { label: string; passed: boolean };

  const list = <T,>(v: unknown): T[] => (Array.isArray(v) ? (v as T[]) : []);
  const bySeverity = (r: RoundView, s: string) => list<Finding>(r.findings).filter((f) => f.severity === s).length;
  const seconds = (r: RoundView) => (r.finishedAt ? Math.max(0, r.finishedAt - r.startedAt) : null);
  const lensNames = (r: RoundView) => list<string>(r.lenses).map((l) => lensWord(l, $t)).join(', ');
  const result = (r: RoundView) => whyWords(r.result as Why | null | undefined, $t);
</script>

<ol class="verify-history">
  {#each verification.history as r (r.round)}
    <li class="verify-history-step">
      <div class="verify-history-head">
        <span class="verify-history-title">
          {#if r.kind === 'review'}
            {$t('verify.round.title', { values: { n: r.round } })}
          {:else}
            {stepWord(r.kind, $t)}
          {/if}
        </span>
        <span class="verify-history-meta">
          {#if seconds(r) !== null}{$t('verify.round.time', { values: { seconds: seconds(r) ?? 0 } })}{/if}
          {#if r.kind === 'review'}{seconds(r) !== null ? ' · ' : ''}{cost(r.costMicrocents)}{/if}
        </span>
      </div>
      {#if r.kind === 'review'}
        <div class="verify-history-line">{$t('verify.round.lenses', { values: { lenses: lensNames(r) } })}</div>
        <div class="verify-history-line">
          {$t('verify.round.findings', { values: { blockers: bySeverity(r, 'blocker'), majors: bySeverity(r, 'major'), minors: bySeverity(r, 'minor') } })}
        </div>
        {#each list<Finding>(r.findings) as f (f.id)}
          <div class="verify-finding verify-finding-{f.severity}">
            <span class="verify-finding-severity">{severityWord(f.severity, $t)}</span>
            <span class="verify-finding-title">{f.title}</span>
            {#if f.evidence?.trim()}<span class="verify-finding-where">{f.evidence}</span>{/if}
          </div>
        {/each}
        {#if list<string>(r.fixed).length}
          <div class="verify-history-line">{$t('verify.round.fixed', { values: { count: list<string>(r.fixed).length } })}</div>
        {/if}
        {#if list<Held>(r.verifiedOk).length}
          <div class="verify-history-line verify-history-held">{$t('verify.round.held')}</div>
          {#each list<Held>(r.verifiedOk) as h, i (i)}
            <div class="verify-held">✓ {h.checked}</div>
          {/each}
        {/if}
      {/if}
      {#if list<CheckRun>(r.checks).length}
        <div class="verify-history-line">
          {$t('verify.round.checks')}
          {#each list<CheckRun>(r.checks) as c, i (i)}
            <span class="verify-check verify-check-{c.passed ? 'passed' : 'failed'}">{c.label}</span>
          {/each}
        </div>
      {:else if r.kind === 'review'}
        <div class="verify-history-line">{$t('verify.round.noChecks')}</div>
      {/if}
      {#if result(r)}
        <div class="verify-history-line">{result(r)}</div>
      {/if}
    </li>
  {/each}
</ol>
<div class="verify-history-total">{$t('verify.total', { values: { cost: cost(verification.costMicrocents) } })}</div>
