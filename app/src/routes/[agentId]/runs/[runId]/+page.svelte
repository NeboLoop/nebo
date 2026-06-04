<script lang="ts">
  import { getContext } from 'svelte';
  import { page } from '$app/stores';
  import type { AgentPageContext, AgentRun, WorkflowActivity } from '$lib/types/agentPage';
  import type { WorkflowRun, WorkflowActivityResult, PendingTask } from '$lib/api/neboComponents';

  const ctx = getContext<AgentPageContext>('agentPage');
  const agentId = $derived(ctx.agentId);
  const runs = $derived(ctx.runs);
  const config = $derived(ctx.config);

  const runId = $derived($page.params.runId);
  const selectedRun = $derived(runs.find((r: AgentRun) => r.id === runId) ?? null);

  let runDetail = $state<WorkflowRun | null>(null);
  let activities = $state<WorkflowActivityResult[]>([]);
  let taskItems = $state<Record<string, PendingTask[]>>({});
  let loading = $state(false);

  // Track which activities and steps are expanded — multiple can be open
  let expandedActivities = $state<Record<string, boolean>>({});
  let expandedSteps = $state<Record<string, boolean>>({});

  $effect(() => {
    const rid = runId;
    const aid = agentId;
    if (!rid || !aid) return;
    loading = true;
    runDetail = null;
    activities = [];
    expandedActivities = {};
    expandedSteps = {};

    import('$lib/api/nebo').then(api =>
      api.getRun(`agent:${aid}`, rid)
    ).then(res => {
      if (res?.run) runDetail = res.run;
      if (res?.activities) activities = res.activities;
      if (res?.taskItems && typeof res.taskItems === 'object') taskItems = res.taskItems as Record<string, PendingTask[]>;
    }).catch(err => {
      console.warn('[nebo] Failed to load run detail:', err);
    }).finally(() => {
      loading = false;
    });
  });

  // Real-time task_updated events — update step status live during execution
  $effect(() => {
    const rid = runId;
    if (!rid || typeof window === 'undefined') return;

    function handleTaskUpdated(e: Event) {
      const data = (e as CustomEvent).detail;
      if (!data?.listId || !data.listId.startsWith(`run:${rid}:`)) return;
      // Extract activityId from listId format: "run:{runId}:{activityId}"
      const parts = data.listId.split(':');
      if (parts.length < 3) return;
      const activityId = parts.slice(2).join(':');

      // Update the task item in our local state
      const items = taskItems[activityId] ?? [];
      const idx = items.findIndex((t: PendingTask) => t.id === data.taskId);
      if (idx >= 0) {
        items[idx] = { ...items[idx], status: data.status };
      } else {
        // Task not yet in our list — add a placeholder
        items.push({ id: data.taskId, taskType: 'tracking', status: data.status, sessionKey: data.listId, prompt: '', listId: data.listId, seq: data.seq, createdAt: 0 } as PendingTask);
      }
      taskItems = { ...taskItems, [activityId]: [...items] };
    }

    window.addEventListener('nebo:task_updated', handleTaskUpdated);
    return () => window.removeEventListener('nebo:task_updated', handleTaskUpdated);
  });

  const workflowDef = $derived.by(() => {
    if (!selectedRun) return null;
    return config.workflows[selectedRun.workflowName] ?? null;
  });

  function getActivityDef(activityId: string): WorkflowActivity | null {
    if (!workflowDef?.activities) return null;
    return workflowDef.activities.find(a => a.id === activityId) ?? null;
  }

  // Merge API activity results with workflow definition for running workflows.
  // The engine only writes results after each activity completes, so for in-progress
  // runs we synthesize entries from the workflow definition.
  const mergedActivities = $derived.by((): WorkflowActivityResult[] => {
    // If we have real results, use them
    if (activities.length > 0) return activities;
    // For running workflows with no results yet, synthesize from the definition
    if (!runDetail || runDetail.status !== 'running' || !workflowDef?.activities) return [];
    const currentAct = runDetail.currentActivity;
    let reachedCurrent = false;
    return workflowDef.activities.map((def) => {
      let status: string;
      if (def.id === currentAct) {
        status = 'running';
        reachedCurrent = true;
      } else if (!reachedCurrent && currentAct) {
        // Activities before the current one must have completed (no record means inline engine)
        status = 'completed';
      } else {
        status = 'pending';
      }
      return {
        id: 0,
        runId: runDetail!.id,
        activityId: def.id,
        status,
        startedAt: runDetail!.startedAt,
        completedAt: undefined,
      } as WorkflowActivityResult;
    });
  });

  // Parse run output into per-activity result sections
  const activityOutputs = $derived.by((): Record<string, string> => {
    const out = runDetail?.output;
    if (!out) return {};
    const map: Record<string, string> = {};
    const regex = /\[Activity '([^']+)' result\]:\s*/g;
    const matches = [...out.matchAll(regex)];
    for (let i = 0; i < matches.length; i++) {
      const start = matches[i].index! + matches[i][0].length;
      const end = i + 1 < matches.length ? matches[i + 1].index! : out.length;
      map[matches[i][1]] = out.substring(start, end).trim();
    }
    return map;
  });

  // Run inputs (e.g., event payload)
  const runInputs = $derived.by((): string | null => {
    const inp = runDetail?.inputs;
    if (!inp) return null;
    try {
      const parsed = typeof inp === 'string' ? JSON.parse(inp) : inp;
      if (typeof parsed === 'object' && Object.keys(parsed).length > 0) {
        return JSON.stringify(parsed, null, 2);
      }
    } catch { /* ignore */ }
    return typeof inp === 'string' && inp.length > 0 ? inp : null;
  });

  const duration = $derived.by(() => {
    if (runDetail?.startedAt && runDetail?.completedAt) {
      const secs = runDetail.completedAt - runDetail.startedAt;
      if (secs >= 60) return `${Math.floor(secs / 60)}m ${Math.round(secs % 60)}s`;
      return `${Math.round(secs)}s`;
    }
    return selectedRun?.duration ?? '—';
  });

  function formatActivityDuration(act: WorkflowActivityResult): string {
    if (!act.startedAt || !act.completedAt) return act.completedAt ? '—' : 'running...';
    const secs = act.completedAt - act.startedAt;
    if (secs >= 60) return `${Math.floor(secs / 60)}m ${Math.round(secs % 60)}s`;
    if (secs > 0) return `${Math.round(secs)}s`;
    return '<1s';
  }

  function statusNorm(status: string): string {
    if (status === 'completed' || status === 'success') return 'success';
    if (status === 'failed') return 'failed';
    if (status === 'running') return 'running';
    if (status === 'pending') return 'pending';
    return status;
  }

  function triggerIcon(type: string): string {
    if (type === 'schedule') return '↻';
    if (type === 'event') return '⚡';
    if (type === 'heartbeat') return '♥';
    return '▶';
  }

  function toggleActivity(id: string) {
    expandedActivities[id] = !expandedActivities[id];
  }

  function toggleStep(key: string) {
    expandedSteps[key] = !expandedSteps[key];
  }

  let triggerLoading = $state(false);
  let cancelLoading = $state(false);

  async function cancelWorkflow() {
    const wfId = runDetail?.workflowId;
    const rid = runId;
    if (!wfId || !rid || cancelLoading) return;
    cancelLoading = true;
    try {
      const api = await import('$lib/api/nebo');
      await api.cancelRun(wfId, rid);
      ctx.refreshRuns?.();
      // Re-fetch run detail to reflect cancelled status
      const res = await api.getRun(`agent:${agentId}`, rid);
      if (res?.run) runDetail = res.run;
      if (res?.activities) activities = res.activities;
    } catch (err) {
      console.warn('[nebo] Failed to cancel run:', err);
    } finally {
      cancelLoading = false;
    }
  }

  async function triggerWorkflow() {
    if (!selectedRun || triggerLoading) return;
    triggerLoading = true;
    try {
      const api = await import('$lib/api/nebo');
      // Forward original run inputs so the re-run has the same event payload
      const originalInputs: Record<string, unknown> = {};
      if (runDetail?.inputs) {
        try {
          const parsed = typeof runDetail.inputs === 'string' ? JSON.parse(runDetail.inputs) : runDetail.inputs;
          if (parsed && typeof parsed === 'object') Object.assign(originalInputs, parsed);
        } catch { /* ignore */ }
      }
      await api.runAgentWorkflow(agentId, selectedRun.workflowName, { inputs: originalInputs });
      // Refresh the runs list
      ctx.refreshRuns?.();
    } catch (err) {
      console.warn('[nebo] Failed to trigger workflow:', err);
    } finally {
      triggerLoading = false;
    }
  }
</script>

<div class="flex-1 flex flex-col bg-base-100 min-w-0 min-h-0">
  {#if loading}
    <div class="flex-1 flex items-center justify-center">
      <span class="loading loading-spinner loading-md text-base-content/30"></span>
    </div>
  {:else if selectedRun}
    <!-- Header -->
    <div class="h-11 px-[18px] border-b border-base-content/10 flex items-center gap-2 shrink-0">
      <a href="/{agentId}/runs" class="w-6 h-6 rounded flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none text-base-content/50 no-underline" title="Back">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="15 18 9 12 15 6"/></svg>
      </a>
      <span class="text-sm font-semibold truncate">{selectedRun.workflowName}</span>
      <span class="py-0 px-1.5 rounded text-xs font-medium shrink-0 {selectedRun.status === 'success' ? 'bg-success/10 text-success' : selectedRun.status === 'failed' ? 'bg-error/10 text-error' : selectedRun.status === 'running' ? 'bg-warning/10 text-warning' : selectedRun.status === 'exited' ? 'bg-info/10 text-info' : selectedRun.status === 'cancelled' ? 'bg-warning/10 text-warning' : 'bg-base-200 text-base-content/50'}">
        {selectedRun.status === 'success' ? 'Completed' : selectedRun.status === 'failed' ? 'Failed' : selectedRun.status === 'running' ? 'Running' : selectedRun.status === 'exited' ? 'Exited' : selectedRun.status === 'cancelled' ? 'Cancelled' : 'Skipped'}
      </span>
      {#if selectedRun.status === 'running'}
        <button
          class="ml-auto py-0.5 px-2 rounded border border-error/30 bg-error/10 text-xs font-medium text-error cursor-pointer hover:bg-error/20 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
          disabled={cancelLoading}
          onclick={cancelWorkflow}
        >
          {#if cancelLoading}
            <span class="loading loading-spinner loading-xs"></span>
          {:else}
            Stop
          {/if}
        </button>
      {/if}
    </div>

    <div class="flex-1 overflow-y-auto p-5 select-text">
      <!-- Run metadata -->
      <div class="flex items-center gap-3 mb-4 text-xs text-base-content/50">
        <span>{triggerIcon(selectedRun.trigger ?? '')} <span class="capitalize">{selectedRun.trigger}</span></span>
        <span class="font-mono">{duration}</span>
        <span class="font-mono">{selectedRun.date}</span>
      </div>

      <!-- Error/reason banner for cancelled/failed/exited runs -->
      {#if selectedRun.error}
        <div class="mb-4 p-3 rounded-lg border {selectedRun.status === 'failed' ? 'border-error/30 bg-error/5' : selectedRun.status === 'exited' ? 'border-info/30 bg-info/5' : 'border-warning/30 bg-warning/5'}">
          <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">{selectedRun.status === 'cancelled' ? 'Reason' : selectedRun.status === 'exited' ? 'Exit Reason' : 'Error'}</div>
          <div class="text-xs text-base-content/70">{selectedRun.error}</div>
        </div>
      {/if}

      <!-- Run inputs (event payload, etc.) -->
      {#if runInputs}
        <div class="mb-4">
          <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">Input</div>
          <div class="p-3 rounded-lg border border-base-300 bg-base-200/30">
            <pre class="text-xs text-base-content/70 whitespace-pre-wrap font-mono m-0">{runInputs}</pre>
          </div>
        </div>
      {/if}

      <!-- Activities timeline -->
      <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-3">Activities</div>
      {#if mergedActivities.length > 0}
        <div class="flex flex-col">
          {#each mergedActivities as activity, idx}
            {@const st = statusNorm(activity.status)}
            {@const def = getActivityDef(activity.activityId)}
            {@const isExpanded = expandedActivities[activity.activityId] ?? false}
            {@const actOutput = activityOutputs[activity.activityId] ?? ''}
            <div class="flex gap-3">
              <!-- Stepper dot + line -->
              <div class="flex flex-col items-center shrink-0">
                <div class="w-6 h-6 rounded-full flex items-center justify-center text-xs shrink-0 {st === 'success' ? 'bg-success/15 text-success' : st === 'failed' ? 'bg-error/15 text-error' : st === 'running' ? 'bg-warning/15 text-warning' : 'bg-base-200 text-base-content/40'}">
                  {#if st === 'success'}
                    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
                  {:else if st === 'failed'}
                    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
                  {:else if st === 'running'}
                    <span class="loading loading-spinner loading-xs"></span>
                  {:else}
                    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><line x1="5" y1="12" x2="19" y2="12"/></svg>
                  {/if}
                </div>
                {#if idx < mergedActivities.length - 1}
                  <div class="w-px flex-1 min-h-4 {st === 'success' ? 'bg-success/30' : st === 'failed' ? 'bg-error/30' : 'bg-base-300'}"></div>
                {/if}
              </div>

              <!-- Activity content -->
              <div class="flex-1 min-w-0 pb-4">
                <!-- Activity header — click to toggle (stays open independently) -->
                <button
                  class="w-full text-left flex items-center gap-2 cursor-pointer bg-transparent border-none p-0"
                  onclick={() => toggleActivity(activity.activityId)}
                >
                  <span class="text-sm font-medium">{activity.activityId}</span>
                  <span class="text-xs text-base-content/50 font-mono">{formatActivityDuration(activity)}</span>
                  <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="text-base-content/30 ml-auto transition-transform {isExpanded ? 'rotate-90' : ''}"><polyline points="9 6 15 12 9 18"/></svg>
                </button>

                {#if activity.error}
                  <!-- Red only for genuine failures; a clean early-exit reason is informational. -->
                  <div class="text-xs mt-0.5 {st === 'failed' ? 'text-error' : 'text-base-content/60'}">{activity.error}</div>
                {/if}

                <!-- Expanded activity detail -->
                {#if isExpanded}
                  <div class="mt-3 flex flex-col gap-3">
                    <!-- Intent -->
                    {#if def?.intent}
                      <div>
                        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">Intent</div>
                        <div class="text-sm text-base-content/70">{def.intent}</div>
                      </div>
                    {/if}

                    <!-- Skills -->
                    {#if def?.skills && def.skills.length > 0}
                      <div>
                        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">Skills</div>
                        <div class="flex flex-wrap gap-1">
                          {#each def.skills as skill}
                            <span class="py-0.5 px-1.5 rounded bg-base-200 font-mono text-xs">{skill}</span>
                          {/each}
                        </div>
                      </div>
                    {/if}

                    <!-- Steps — each with input/output from task_items -->
                    {#if def?.steps && def.steps.length > 0}
                      {@const stepTasks = taskItems[activity.activityId] ?? []}
                      <div>
                        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">Steps</div>
                        <div class="flex flex-col gap-3">
                          {#each def.steps as step, sIdx}
                            {@const task = stepTasks.find(t => t.seq === sIdx + 1)}
                            {@const stepStatus = task?.status ?? (sIdx === 0 && st === 'running' ? 'in_progress' : 'pending')}
                            <div class="rounded-lg border border-base-300 bg-base-200/20 p-3">
                              <div class="flex items-center gap-2 mb-2">
                                <div class="w-5 h-5 rounded-full flex items-center justify-center text-xs shrink-0 font-mono font-medium {stepStatus === 'completed' ? 'bg-success/15 text-success' : stepStatus === 'failed' ? 'bg-error/15 text-error' : stepStatus === 'in_progress' ? 'bg-warning/15 text-warning' : 'bg-base-200 text-base-content/50'}">
                                  {#if stepStatus === 'completed'}
                                    <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
                                  {:else if stepStatus === 'failed'}
                                    <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
                                  {:else if stepStatus === 'in_progress'}
                                    <span class="loading loading-spinner loading-xs"></span>
                                  {:else}
                                    {sIdx + 1}
                                  {/if}
                                </div>
                                <span class="text-xs font-semibold uppercase tracking-wider text-base-content/50">Step {sIdx + 1}</span>
                                {#if task && (task.tokensInput ?? 0) + (task.tokensOutput ?? 0) > 0}
                                  {@const totalTok = (task.tokensInput ?? 0) + (task.tokensOutput ?? 0)}
                                  <span class="text-xs text-base-content/40 font-mono ml-auto">{totalTok >= 1000 ? (totalTok / 1000).toFixed(1) + 'k' : totalTok} tok</span>
                                {/if}
                                {#if task?.startedAt && task?.completedAt}
                                  {@const stepSecs = task.completedAt - task.startedAt}
                                  <span class="text-xs text-base-content/40 font-mono">{stepSecs >= 60 ? Math.floor(stepSecs / 60) + 'm ' + Math.round(stepSecs % 60) + 's' : stepSecs > 0 ? Math.round(stepSecs) + 's' : '<1s'}</span>
                                {/if}
                              </div>
                              <div class="ml-7">
                                <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-0.5">Input</div>
                                <div class="text-sm text-base-content/70 mb-2">{step}</div>
                                <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-0.5">Output</div>
                                {#if task?.output}
                                  <div class="text-xs text-base-content/70 whitespace-pre-wrap">{task.output}</div>
                                {:else if task?.lastError}
                                  <div class="text-xs text-error">{task.lastError}</div>
                                {:else if stepStatus === 'in_progress'}
                                  <div class="flex items-center gap-1.5">
                                    <span class="loading loading-spinner loading-xs text-warning"></span>
                                    <span class="text-xs text-base-content/50">Running...</span>
                                  </div>
                                {:else if stepStatus === 'completed' && !task?.output}
                                  <!-- Fallback: no task_item data, use activity-level output for last step -->
                                  {#if sIdx === def.steps.length - 1 && actOutput}
                                    <div class="text-xs text-base-content/70 whitespace-pre-wrap">{actOutput}</div>
                                  {:else}
                                    <div class="text-xs text-base-content/40">Completed (no output captured)</div>
                                  {/if}
                                {:else if stepStatus === 'pending'}
                                  <div class="text-xs text-base-content/40">—</div>
                                {:else}
                                  <div class="text-xs text-base-content/40">No output recorded</div>
                                {/if}
                              </div>
                            </div>
                          {/each}
                        </div>
                      </div>
                    {:else}
                      <!-- No steps defined — show activity-level result -->
                      <div>
                        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">Result</div>
                        {#if actOutput}
                          <div class="p-3 rounded-lg border border-base-300 bg-base-200/30">
                            <div class="text-xs text-base-content/70 whitespace-pre-wrap">{actOutput}</div>
                          </div>
                        {:else if activity.error}
                          <!-- Failures get the red treatment; clean exits read as info. -->
                          <div class="p-3 rounded-lg border {st === 'failed' ? 'border-error/30 bg-error/5' : 'border-info/30 bg-info/5'}">
                            <div class="text-xs {st === 'failed' ? 'text-error' : 'text-base-content/70'}">{activity.error}</div>
                          </div>
                        {:else if st === 'running'}
                          <div class="p-3 rounded-lg border border-warning/30 bg-warning/5 flex items-center gap-2">
                            <span class="loading loading-spinner loading-xs text-warning"></span>
                            <span class="text-xs text-base-content/50">Running...</span>
                          </div>
                        {:else}
                          <div class="p-3 rounded-lg border border-base-300 bg-base-200/30">
                            <div class="text-xs text-base-content/50">No output recorded.</div>
                          </div>
                        {/if}
                      </div>
                    {/if}

                    <!-- Token counts -->
                    {#if activity.tokensUsed && activity.tokensUsed > 0}
                      <div class="flex gap-3 text-xs text-base-content/50 font-mono">
                        <span>{activity.tokensUsed >= 1000 ? (activity.tokensUsed / 1000).toFixed(1) + 'k' : activity.tokensUsed} tokens</span>
                        {#if activity.attempts && activity.attempts > 1}
                          <span>&middot; {activity.attempts} attempts</span>
                        {/if}
                      </div>
                    {/if}
                  </div>
                {/if}
              </div>
            </div>
          {/each}
        </div>
      {:else}
        <div class="text-xs text-base-content/50">No activity records for this run.</div>
      {/if}

      <!-- Footer -->
      <div class="mt-4 pt-4 border-t border-base-300 flex gap-2">
        <button
          class="py-1.5 px-3 rounded-md border border-base-300 bg-base-100 text-sm cursor-pointer hover:bg-base-200 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
          disabled={triggerLoading}
          onclick={triggerWorkflow}
        >
          {#if triggerLoading}
            <span class="loading loading-spinner loading-xs"></span>
          {:else}
            {selectedRun.status === 'failed' ? 'Retry' : 'Run Again'}
          {/if}
        </button>
      </div>
    </div>
  {:else}
    <div class="flex-1 flex items-center justify-center">
      <div class="text-center">
        <div class="text-sm font-medium mb-1">Run not found</div>
        <a href="/{agentId}/runs" class="text-xs text-primary hover:underline">Back to runs</a>
      </div>
    </div>
  {/if}
</div>
