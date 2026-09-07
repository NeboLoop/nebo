<!--
  The case inspector, read-only: what the engine knows about this employee's
  cases — who each one is for, what it waits on and since when, what came in
  and what went out, what needs attention — with the ledger's receipts beside
  every turn's words. REST on load and on demand; never polled.
-->
<script lang="ts">
  import { page } from '$app/stores';
  import { t } from 'svelte-i18n';
  import { listCases, getCase } from '$lib/api/nebo';
  import type { CaseSummary, CaseDetail } from '$lib/api/neboComponents';

  let cases = $state<CaseSummary[]>([]);
  let detail = $state<CaseDetail | null>(null);
  let loading = $state(false);
  let error = $state('');

  const agentId = $derived($page.params.agentId ?? '');

  const when = (ts?: number) => (ts ? new Date(ts * 1000).toLocaleString() : '—');
  const short = (s?: string, n = 90) => (s ? (s.length > n ? `${s.slice(0, n)}…` : s) : '—');
  const stateClass = (s: string) =>
    s === 'waiting'
      ? 'badge-info'
      : s === 'running' || s === 'queued'
        ? 'badge-warning'
        : s === 'done'
          ? 'badge-success'
          : s === 'failed'
            ? 'badge-error'
            : 'badge-ghost';
  const receiptClass = (s: string) => (s === 'completed' ? 'badge-success' : s === 'pending' ? 'badge-warning' : 'badge-error');

  async function load() {
    loading = true;
    error = '';
    try {
      cases = (await listCases(agentId, 200)).cases;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  async function open(id: string) {
    try {
      detail = await getCase(id);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  $effect(() => {
    if (agentId) void load();
  });
</script>

<div class="flex h-full flex-col gap-4 overflow-y-auto p-4">
  <div class="flex items-center gap-3">
    <h1 class="text-lg font-semibold">{$t('cases.title')}</h1>
    <span class="text-sm text-base-content/60">{agentId}</span>
    <button class="btn btn-sm btn-ghost ml-auto" onclick={() => void load()} disabled={loading}>
      {$t('cases.refresh')}
    </button>
  </div>

  {#if error}
    <div class="alert alert-error text-sm">{error}</div>
  {/if}

  {#if !loading && cases.length === 0}
    <p class="text-sm text-base-content/60">{$t('cases.empty')}</p>
  {:else}
    <div class="overflow-x-auto">
      <table class="table table-xs table-zebra">
        <thead>
          <tr>
            <th>{$t('cases.case')}</th>
            <th>{$t('cases.subject')}</th>
            <th>{$t('cases.state')}</th>
            <th>{$t('cases.waitingFor')}</th>
            <th>{$t('cases.waitSince')}</th>
            <th>{$t('cases.wakeAt')}</th>
            <th>{$t('cases.lastInbound')}</th>
            <th>{$t('cases.lastOutbound')}</th>
            <th>{$t('cases.lastTransition')}</th>
            <th>{$t('cases.attention')}</th>
          </tr>
        </thead>
        <tbody>
          {#each cases as c (c.id)}
            <tr class="cursor-pointer hover" class:bg-base-200={detail?.case.id === c.id} onclick={() => void open(c.id)}>
              <td class="font-mono">{c.id.slice(0, 8)} <span class="text-base-content/60">{c.case_type}</span></td>
              <td>{c.aliases.join(', ') || c.subject_id.slice(0, 8)}</td>
              <td>
                <span class="badge badge-sm {stateClass(c.state)}">{c.state}</span>
                {#if c.result}<span class="ml-1 text-base-content/70">{c.result}</span>{/if}
              </td>
              <td>{c.waiting_for ? `${c.waiting_for.on} — ${short(c.waiting_for.reason, 60)}` : '—'}</td>
              <td>{when(c.waiting_for?.since)}</td>
              <td>{when(c.waiting_for?.wake_at)}</td>
              <td title={c.last_inbound?.text}>{when(c.last_inbound?.at)}</td>
              <td title={c.last_outbound?.result}>{c.last_outbound ? `${when(c.last_outbound.at)} · ${c.last_outbound.provider}` : '—'}</td>
              <td>{when(c.last_transition)}</td>
              <td class="text-error" title={c.attention}>{short(c.attention, 50)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}

  {#if detail}
    {@const c = detail.case}
    <div class="card bg-base-100 shadow-sm">
      <div class="card-body gap-3 p-4">
        <h2 class="card-title text-base">
          <span class="font-mono">{c.id}</span>
          <span class="badge badge-sm {stateClass(c.state)}">{c.state}</span>
          {#if c.result}<span class="text-sm text-base-content/70">{c.result}</span>{/if}
        </h2>
        <dl class="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 text-sm">
          <dt class="text-base-content/60">{$t('cases.owner')}</dt>
          <dd>{c.owner}</dd>
          <dt class="text-base-content/60">{$t('cases.subject')}</dt>
          <dd>{c.aliases.join(', ')} <span class="font-mono text-base-content/50">{c.subject_id}</span></dd>
          <dt class="text-base-content/60">{$t('cases.waitingFor')}</dt>
          <dd>{c.waiting_for ? `${c.waiting_for.on} — ${c.waiting_for.reason}` : '—'}</dd>
          <dt class="text-base-content/60">{$t('cases.waitSince')}</dt>
          <dd>{when(c.waiting_for?.since)}</dd>
          <dt class="text-base-content/60">{$t('cases.wakeAt')}</dt>
          <dd>{when(c.waiting_for?.wake_at)}</dd>
          <dt class="text-base-content/60">{$t('cases.lastInbound')}</dt>
          <dd>{when(c.last_inbound?.at)} {short(c.last_inbound?.text, 160)}</dd>
          <dt class="text-base-content/60">{$t('cases.lastOutbound')}</dt>
          <dd>{c.last_outbound ? `${when(c.last_outbound.at)} · ${c.last_outbound.provider} #${c.last_outbound.id} · ${short(c.last_outbound.result, 120)}` : '—'}</dd>
          <dt class="text-base-content/60">{$t('cases.lastTransition')}</dt>
          <dd>{when(c.last_transition)}</dd>
          <dt class="text-base-content/60">{$t('cases.attention')}</dt>
          <dd class="text-error">{c.attention ?? '—'}</dd>
          {#if c.previous_case}
            <dt class="text-base-content/60">{$t('cases.previousCase')}</dt>
            <dd><button class="link font-mono" onclick={() => void open(c.previous_case ?? '')}>{c.previous_case}</button></dd>
          {/if}
        </dl>

        <h3 class="mt-2 text-sm font-semibold">{$t('cases.turns')}</h3>
        <ul class="flex flex-col gap-2 text-sm">
          {#each detail.turns as turn (turn.id)}
            <li class="rounded border border-base-300 p-2">
              <div class="flex flex-wrap items-center gap-2">
                <span class="font-mono">{turn.id.slice(0, 8)}</span>
                <span class="badge badge-sm {stateClass(turn.state)}">{turn.state}</span>
                <span class="text-base-content/60">{when(turn.started_at)} → {when(turn.ended_at)}</span>
                {#if turn.model}<span class="badge badge-sm badge-ghost">{$t('cases.model')}: {turn.model}</span>{/if}
              </div>
              {#if turn.output}
                <pre class="mt-1 whitespace-pre-wrap break-words font-mono text-xs text-base-content/80">{turn.output}</pre>
              {/if}
              <div class="mt-1 text-xs">
                <span class="text-base-content/60">{$t('cases.receipts')}:</span>
                {#if turn.receipts.length === 0}
                  <span class="text-base-content/60">{$t('cases.noReceipts')}</span>
                {:else}
                  {#each turn.receipts as r (r.id)}
                    <span class="badge badge-sm {receiptClass(r.state)} mr-1" title={r.result}>
                      {r.provider} #{r.id} {r.state}{r.to ? ` → ${r.to}` : ''}{r.reference ? ` (${r.reference})` : ''}
                    </span>
                  {/each}
                {/if}
              </div>
            </li>
          {/each}
        </ul>

        <h3 class="mt-2 text-sm font-semibold">{$t('cases.history')}</h3>
        <ol class="flex flex-col gap-1 text-xs">
          {#each detail.history as h (h.id)}
            <li class="flex gap-2">
              <span class="shrink-0 text-base-content/50">{when(h.at)}</span>
              <span class="shrink-0 font-mono" class:text-error={h.kind === 'needs_attention' || h.kind === 'turn_failed'}>{h.kind}</span>
              <span class="break-words">{h.text}</span>
            </li>
          {/each}
        </ol>

        <h3 class="mt-2 text-sm font-semibold">{$t('cases.waits')}</h3>
        <ol class="flex flex-col gap-1 text-xs">
          {#each detail.waits as w (w.id)}
            <li class="flex gap-2" class:text-base-content={!w.superseded_at} class:text-base-content-50={!!w.superseded_at}>
              <span class="font-mono">#{w.id}</span>
              <span>{w.on}</span>
              <span>{when(w.since)} → {when(w.wake_at)}</span>
              <span class="break-words">{w.reason}</span>
              {#if w.superseded_at}<span class="text-base-content/50">({when(w.superseded_at)})</span>{/if}
            </li>
          {/each}
        </ol>
      </div>
    </div>
  {/if}
</div>
