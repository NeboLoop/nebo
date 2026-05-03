<script lang="ts">
  import { onMount } from 'svelte';
  import { getContext } from 'svelte';
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import type { AgentPageContext, AgentRun, WorkflowActivity } from '$lib/types/agentPage';
  const ctx = getContext<AgentPageContext>('agentPage');
  const agentId = $derived(ctx.agentId);
  const runs = $derived(ctx.runs);
  const workflowRuns = $derived(ctx.workflowRuns);

  const runId = $derived($page.params.runId);

  // Derived: selected run and its workflow run detail
  const selectedRun = $derived(runs.find((r: AgentRun) => r.id === runId) ?? null);
  interface WFRun { id: string; triggerType: string; duration: string; startedAt: string; completedAt: string; tokens?: { input: number; output: number }; error?: string; activities?: WFActivity[]; workflowId?: string }
  interface WFActivity { id: string; status: string; duration: string; output?: string; error?: string }

  const selectedWorkflowRun = $derived.by((): WFRun | null => {
    if (!selectedRun?.workflowRunId) return null;
    return ((workflowRuns as WFRun[]).find((wr) => wr.id === selectedRun.workflowRunId)) ?? null;
  });

  // API-loaded workflow definitions (overrides mock when available)
  let apiWorkflows = $state<Record<string, any> | null>(null);

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const res = await api.listAgentWorkflows(agentId);
      if (res?.workflows) {
        const map: Record<string, any> = {};
        for (const w of res.workflows) {
          const wf = w as Record<string, unknown>;
          const key = String(wf.id || wf.slug || '');
          if (key) map[key] = wf;
        }
        apiWorkflows = map;
      }
    } catch { /* keep mock data */ }
  });

  // Derived: workflow definition for the selected run (for intent/skills/steps)
  const selectedWorkflowDef = $derived.by(() => {
    if (!selectedWorkflowRun) return null;
    // Try API-loaded workflows
    const wfId = selectedWorkflowRun.workflowId;
    if (apiWorkflows && wfId && apiWorkflows[wfId]) {
      return apiWorkflows[wfId];
    }
    return null;
  });

  let expandedActivities = $state<Record<string, boolean>>({});

  function toggleActivityDetail(actId: string) {
    expandedActivities[actId] = !expandedActivities[actId];
  }

  function triggerIcon(type: string): string {
    if (type === 'schedule') return '↻';
    if (type === 'event') return '⚡';
    return '▶';
  }

  function formatTokens(n: number): string {
    if (n >= 1000) return (n / 1000).toFixed(1) + 'k';
    return String(n);
  }
</script>

<div class="flex-1 flex flex-col bg-base-100 min-w-0 min-h-0">
  {#if selectedRun && selectedWorkflowRun}
    <!-- Header -->
    <div class="h-11 px-[18px] border-b border-base-content/10 flex items-center gap-2 shrink-0">
      <a href="/{agentId}/runs" class="w-6 h-6 rounded flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none text-base-content/50 no-underline" title="Back to overview">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="15 18 9 12 15 6"/></svg>
      </a>
      <span class="text-sm font-semibold truncate">{selectedRun.name}</span>
      <span class="py-0 px-1.5 rounded text-xs font-medium shrink-0 {selectedRun.status === 'success' ? 'bg-success/10 text-success' : selectedRun.status === 'failed' ? 'bg-error/10 text-error' : selectedRun.status === 'running' ? 'bg-warning/10 text-warning' : 'bg-base-200 text-base-content/50'}">
        {selectedRun.status === 'success' ? 'Completed' : selectedRun.status === 'failed' ? 'Failed' : selectedRun.status === 'running' ? 'Running' : 'Skipped'}
      </span>
    </div>

    <div class="flex-1 overflow-y-auto p-5">
      <!-- Metadata -->
      <div class="grid grid-cols-2 gap-x-6 gap-y-2 mb-5">
        <div>
          <div class="text-xs text-base-content/50">Trigger</div>
          <div class="text-sm font-medium flex items-center gap-1.5">
            <span class="text-base-content/50">{triggerIcon(selectedWorkflowRun.triggerType)}</span>
            <span class="capitalize">{selectedWorkflowRun.triggerType}</span>
          </div>
        </div>
        <div>
          <div class="text-xs text-base-content/50">Duration</div>
          <div class="text-sm font-medium font-mono">{selectedWorkflowRun.duration}</div>
        </div>
        <div>
          <div class="text-xs text-base-content/50">Started</div>
          <div class="text-sm font-mono">{selectedWorkflowRun.startedAt}</div>
        </div>
        <div>
          <div class="text-xs text-base-content/50">Completed</div>
          <div class="text-sm font-mono">{selectedWorkflowRun.completedAt}</div>
        </div>
        {#if selectedWorkflowRun.tokens}
          <div>
            <div class="text-xs text-base-content/50">Tokens in</div>
            <div class="text-sm font-mono">{formatTokens(selectedWorkflowRun.tokens.input)}</div>
          </div>
          <div>
            <div class="text-xs text-base-content/50">Tokens out</div>
            <div class="text-sm font-mono">{formatTokens(selectedWorkflowRun.tokens.output)}</div>
          </div>
        {/if}
      </div>

      <!-- Error banner -->
      {#if selectedWorkflowRun.error}
        <div class="flex items-start gap-2.5 p-3 rounded-lg border border-error/30 bg-error/5 mb-5">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-error shrink-0 mt-0.5"><circle cx="12" cy="12" r="10"/><line x1="12" y1="8" x2="12" y2="12"/><line x1="12" y1="16" x2="12.01" y2="16"/></svg>
          <div>
            <div class="text-sm font-medium text-error">Run failed</div>
            <div class="text-xs text-base-content/70 mt-0.5">{selectedWorkflowRun.error}</div>
          </div>
        </div>
      {/if}

      <!-- Activity Timeline -->
      <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-3">Activity Timeline</div>
      <div class="flex flex-col">
        {#each selectedWorkflowRun.activities ?? [] as activity, idx}
          {@const defActivity = selectedWorkflowDef?.activities?.find((a: WorkflowActivity) => a.id === activity.id)}
          <div class="flex gap-3">
            <!-- Vertical stepper line + icon -->
            <div class="flex flex-col items-center shrink-0">
              <div class="w-6 h-6 rounded-full flex items-center justify-center text-xs shrink-0 {activity.status === 'success' ? 'bg-success/15 text-success' : activity.status === 'failed' ? 'bg-error/15 text-error' : activity.status === 'running' ? 'bg-warning/15 text-warning' : 'bg-base-200 text-base-content/40'}">
                {#if activity.status === 'success'}
                  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
                {:else if activity.status === 'failed'}
                  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
                {:else if activity.status === 'running'}
                  <span class="loading loading-spinner loading-xs"></span>
                {:else}
                  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><line x1="5" y1="12" x2="19" y2="12"/></svg>
                {/if}
              </div>
              {#if idx < (selectedWorkflowRun.activities?.length ?? 0) - 1}
                <div class="w-px flex-1 min-h-4 {activity.status === 'success' ? 'bg-success/30' : activity.status === 'failed' ? 'bg-error/30' : 'bg-base-300'}"></div>
              {/if}
            </div>

            <!-- Activity content -->
            <div class="flex-1 min-w-0 pb-4">
              <button
                class="w-full text-left flex items-center gap-2 cursor-pointer bg-transparent border-none p-0"
                onclick={() => toggleActivityDetail(activity.id)}
              >
                <span class="text-sm font-medium">{activity.id}</span>
                <span class="text-xs text-base-content/50 font-mono">{activity.duration}</span>
                {#if defActivity}
                  <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="text-base-content/30 transition-transform {expandedActivities[activity.id] ? 'rotate-90' : ''}"><polyline points="9 6 15 12 9 18"/></svg>
                {/if}
              </button>

              <!-- One-line summary -->
              {#if activity.output}
                <div class="text-xs text-base-content/70 mt-0.5">{activity.output}</div>
              {/if}
              {#if activity.error}
                <div class="text-xs text-error mt-0.5">{activity.error}</div>
              {/if}

              <!-- Expanded: intent, skills, steps from workflow definition -->
              {#if expandedActivities[activity.id] && defActivity}
                <div class="mt-2 p-3 rounded-lg border border-base-300 bg-base-200/30">
                  {#if defActivity.intent}
                    <div class="text-xs text-base-content/50 mb-1">Intent</div>
                    <div class="text-sm mb-2">{defActivity.intent}</div>
                  {/if}
                  {#if defActivity.skills?.length > 0}
                    <div class="text-xs text-base-content/50 mb-1">Skills</div>
                    <div class="flex flex-wrap gap-1 mb-2">
                      {#each defActivity.skills as skill}
                        <span class="py-0.5 px-1.5 rounded bg-base-200 font-mono text-xs">{skill}</span>
                      {/each}
                    </div>
                  {/if}
                  {#if defActivity.steps?.length > 0}
                    <div class="text-xs text-base-content/50 mb-1">Steps</div>
                    <ol class="list-decimal list-inside text-sm text-base-content/70 flex flex-col gap-0.5">
                      {#each defActivity.steps as step}
                        <li>{step}</li>
                      {/each}
                    </ol>
                  {/if}
                </div>
              {/if}
            </div>
          </div>
        {/each}
      </div>

      <!-- Footer actions -->
      {#if selectedRun.status === 'failed'}
        <div class="mt-4 pt-4 border-t border-base-300 flex gap-2">
          <button class="py-1.5 px-3 rounded-md bg-base-content text-base-100 text-sm font-medium cursor-pointer border-none">Retry</button>
        </div>
      {:else}
        <div class="mt-4 pt-4 border-t border-base-300 flex gap-2">
          <button class="py-1.5 px-3 rounded-md border border-base-300 bg-base-100 text-sm cursor-pointer hover:bg-base-200 transition-colors">Run Now</button>
        </div>
      {/if}
    </div>
  {:else}
    <!-- Run not found -->
    <div class="flex-1 flex items-center justify-center">
      <div class="text-center">
        <div class="text-sm font-medium mb-1">Run not found</div>
        <a href="/{agentId}/runs" class="text-xs text-primary hover:underline">Back to runs</a>
      </div>
    </div>
  {/if}
</div>
