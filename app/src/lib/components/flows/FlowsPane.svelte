<!--
  Flows — the employee's automated sequences, in the work pane.

  This is the surface that used to live in Settings → Workflows. It moved here
  rather than being copied: two lists of the same thing, in two places, is the
  tech debt the house rules forbid. Everything that section did still works —
  activate/pause, trigger summary, activity chips, last fired, the canvas, and
  creating a new flow or a call tree.
-->
<script lang="ts">
  import { getContext } from 'svelte';
  import { t } from 'svelte-i18n';
  import { getActivityType } from '$lib/utils/workflowTypes';
  import type { AgentPageContext, WorkflowConfig, WorkflowActivity } from '$lib/types/agentPage';

  // Clicking a flow opens the visual builder: seeing the chain of steps and
  // the events between them is the whole point of having flows at all.
  let { onask }: { onask: (prompt: string) => void } = $props();

  const ctx = getContext<AgentPageContext>('agentPage');
  const entries = $derived(ctx.workflowEntries);
  const stats = $derived(ctx.workflowStats);

  // Verbatim from the settings section this replaced — a move should not
  // quietly change what the rows say.
  function triggerSummary(wf: WorkflowConfig): string {
    if (wf.trigger?.type === 'schedule') return wf.schedule || 'Scheduled';
    if (wf.trigger?.type === 'event') return `On ${wf.trigger.event || 'event'}`;
    if (wf.trigger?.type === 'watch') return `Watch: ${wf.trigger.event || wf.trigger.plugin || 'plugin'}`;
    if (wf.trigger?.type === 'heartbeat') return `Every ${wf.trigger.interval || '?'}`;
    return 'Manual trigger';
  }

  function formatLastFired(iso: string): string {
    const d = new Date(iso);
    return isNaN(d.getTime())
      ? iso
      : d.toLocaleString(undefined, { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' });
  }

</script>

<div class="flex flex-col h-full min-h-0">
  <div class="px-4 py-3 border-b border-base-content/8 flex items-start gap-2 shrink-0">
    <div class="flex-1 min-w-0">
      <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50">{$t('nav.flows')}</div>
      <div class="text-xs text-base-content/70 mt-0.5">
        {$t('agentSettings.automatedSequencesFor', { values: { name: ctx.agent?.name ?? '' } })}
      </div>
    </div>
  </div>

  <div class="flex-1 min-h-0 overflow-y-auto p-3 flex flex-col gap-2">
    <!-- Two across, not four: this is a 450px rail, not a settings page. -->
    {#if stats.totalRuns > 0}
      <div class="grid grid-cols-2 gap-2 mb-1">
        <div class="rounded-lg border border-base-300 bg-base-100 p-2 text-center">
          <div class="text-base font-semibold">{stats.totalRuns}</div>
          <div class="text-xs text-base-content/50">{$t('agentActivity.totalRuns')}</div>
        </div>
        <div class="rounded-lg border border-base-300 bg-base-100 p-2 text-center">
          <div class="text-base font-semibold {stats.failed > 0 ? 'text-error' : 'text-success'}">
            {stats.failed > 0 ? stats.failed : stats.completed}
          </div>
          <div class="text-xs text-base-content/50">
            {stats.failed > 0 ? $t('common.failed') : $t('common.completed')}
          </div>
        </div>
      </div>
    {/if}

    {#if entries.length === 0}
      <p class="text-center py-8 text-sm text-base-content/50">{$t('agentSettings.noWorkflows')}</p>
    {:else}
      {#each entries as [name, wf] (name)}
        {@const purchased = wf.source === 'marketplace'}
        <div class="rounded-lg border border-base-300 bg-base-100 overflow-hidden">
          <div class="flex items-start gap-2.5 p-3">
            <div class="w-[22px] h-[22px] rounded flex items-center justify-center text-sm shrink-0 mt-0.5 {wf.isActive !== false ? 'bg-primary/10 text-primary' : 'bg-base-200 text-base-content/40'}">
              {#if wf.trigger?.type === 'schedule'}&#8635;{:else if wf.trigger?.type === 'event'}&#9889;{:else if wf.trigger?.type === 'watch'}&#128065;{:else if wf.trigger?.type === 'heartbeat'}&#10084;{:else}&#9654;{/if}
            </div>

            <button class="flex-1 min-w-0 text-left cursor-pointer bg-transparent border-none p-0" onclick={() => ctx.openWorkflow(name, wf)}>
              <div class="flex items-center gap-1.5 flex-wrap">
                <span class="text-sm font-medium">{name}</span>
                {#if purchased}
                  <span class="py-0 px-1.5 rounded bg-base-200 text-xs font-mono">{$t('nav.marketplace')}</span>
                {/if}
                {#if wf.isActive === false}
                  <span class="py-0 px-1.5 rounded bg-base-200 text-xs text-base-content/50">{$t('common.paused')}</span>
                {/if}
              </div>
              {#if wf.description}
                <div class="text-xs text-base-content/70 mt-0.5 truncate">{wf.description}</div>
              {/if}
              <div class="flex items-center gap-1.5 mt-1.5 flex-wrap">
                <span class="text-xs text-base-content/50 font-mono">{triggerSummary(wf)}</span>
                <span class="text-xs text-base-content/30">&middot;</span>
                <span class="text-xs text-base-content/50 font-mono inline-flex items-center gap-1">{(wf.activities?.length ?? 0) === 1 ? $t('agentSettings.activityCountSingular', { values: { count: 1 } }) : $t('agentSettings.activityCount', { values: { count: wf.activities?.length ?? 0 } })}{#each [...new Set((wf.activities ?? []).map((a: WorkflowActivity) => a.type).filter(Boolean))] as ty}<span class="inline-block" title={getActivityType(ty).label}>{getActivityType(ty).icon}</span>{/each}</span>
                {#if wf.lastFired}
                  <span class="text-xs text-base-content/30">&middot;</span>
                  <span class="text-xs text-base-content/50 font-mono">{$t('agentSettings.lastFired', { values: { time: formatLastFired(wf.lastFired) } })}</span>
                {/if}
                {#if wf.emit}
                  <span class="text-xs text-base-content/30">&middot;</span>
                  <span class="text-xs text-accent/70 font-mono">&#8594; {wf.emit}</span>
                {/if}
              </div>
            </button>

            <input
              type="checkbox"
              class="toggle toggle-sm toggle-primary shrink-0 mt-1"
              checked={wf.isActive !== false}
              role="switch"
              aria-checked={wf.isActive !== false}
              onchange={() => ctx.toggleWorkflow(name)}
            />
          </div>
        </div>
      {/each}
    {/if}

    <button
      class="mt-1 w-full py-2.5 rounded-lg border border-dashed border-base-300 text-sm text-primary font-medium cursor-pointer bg-transparent hover:bg-base-200 transition-colors"
      onclick={() => onask(`Set up a new flow for me: `)}
    >Ask {ctx.agent?.name ?? 'your employee'} to set one up</button>
  </div>
</div>
