<script lang="ts">
  import { AGENT_COLORS } from '$lib/tokens.js';
  import { AGENTS, AGENT_ID_REVERSE } from '$lib/data.js';
  import { triggerGlyph, fmtTime } from '$lib/utils.js';
  import { getRecentRuns, getWorkflowDef, getScheduleAgents, snapTo15, userScheduleItems, updateUserItem, loadScheduleFromAPI } from '$lib/stores/schedule.js';
  import type { CalendarItem, WorkflowDefData } from '$lib/stores/schedule.js';

  interface CreateData { hour: number; date: Date }

  let {
    item = null,
    createData = null,
    onclose,
    onopencanvas,
    preview = $bindable(null),
  }: {
    item?: CalendarItem | null;
    createData?: CreateData | null;
    onclose?: () => void;
    onopencanvas?: (agentFull: string) => void;
    preview?: { agent: string; hour: number; dur: number; label: string } | null;
  } = $props();

  const c = $derived(item ? AGENT_COLORS[item.agent] : null);
  const agent = $derived(item ? AGENTS.find(x => x.id === item.agent) : null);

  const recentRuns = $derived(
    item?.agentFull && item?.workflowId
      ? getRecentRuns(item.agentFull, item.workflowId).slice(0, 5)
      : []
  );

  // Workflow definition — loaded from schedule store cache (populated by loadScheduleFromAPI)
  const workflowDef = $derived.by((): WorkflowDefData | null => {
    if (!item?.agentFull || !item?.workflowId) return null;
    return getWorkflowDef(item.agentFull, item.workflowId);
  });
  const workflowActivities = $derived(workflowDef?.activities ?? []);
  const workflowConnections = $derived(workflowDef?.connections ?? []);

  function statusIcon(status: string): string {
    if (status === 'success') return '✓';
    if (status === 'failed') return '✗';
    if (status === 'skipped') return '–';
    return '…';
  }

  function statusColor(status: string): string {
    if (status === 'success') return 'text-success';
    if (status === 'failed') return 'text-error';
    if (status === 'skipped') return 'text-warning';
    return 'text-base-content/50';
  }

  // ─── Editing state ────────────────────────────────────────────────
  let editing = $state(false);
  let editHour = $state(0);
  let editMinute = $state(0);
  let editDurMinutes = $state(30);
  let editDays = $state<number[]>([]);

  function startEditing() {
    if (!item) return;
    editing = true;
    editHour = Math.floor(item.hour);
    editMinute = Math.round((item.hour - Math.floor(item.hour)) * 60);
    editDurMinutes = Math.round(item.dur * 60);
    editDays = [...item.days];
  }

  function cancelEditing() { editing = false; }

  function saveEditing() {
    if (!item) return;
    const fractionalHour = editHour + editMinute / 60;
    updateUserItem(item.id, {
      hour: fractionalHour,
      dur: editDurMinutes / 60,
      days: editDays,
    });
    editing = false;
  }

  function toggleEditDay(d: number) {
    if (editDays.includes(d)) {
      editDays = editDays.filter(x => x !== d);
    } else {
      editDays = [...editDays, d].sort();
    }
  }

  // All items are editable from the schedule panel
  const isEditable = $derived(!!item);

  // ─── Create form state ───────────────────────────────────────────
  const schedAgents = $derived(getScheduleAgents($userScheduleItems));

  let formAgent = $state('');
  let formLabel = $state('');
  let formHour = $state(9);
  let formMinute = $state(0);
  let formDays = $state<number[]>([1, 2, 3, 4, 5]);
  let formTriggerType = $state<'schedule' | 'heartbeat'>('schedule');
  let formInterval = $state('30m');
  let formSaving = $state(false);
  let formError = $state('');

  const dayLabels = ['', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
  const intervalPresets = ['5m', '10m', '15m', '30m', '1h', '2h', '4h'];
  const editHours = Array.from({ length: 24 }, (_, i) => i);
  const editMinutes = [0, 15, 30, 45];

  $effect(() => {
    if (createData) {
      const snapped = snapTo15(createData.hour);
      formHour = Math.floor(snapped);
      formMinute = Math.round((snapped - Math.floor(snapped)) * 60);
      formAgent = schedAgents[0] || '';
      formLabel = '';
      formTriggerType = 'schedule';
      formInterval = '30m';
      formSaving = false;
      formError = '';
      const wd = createData.date.getDay() === 0 ? 7 : createData.date.getDay();
      formDays = [wd];
    }
  });

  function fmtHourLabel(h: number): string {
    if (h === 0) return '12 AM';
    if (h === 12) return '12 PM';
    return h < 12 ? `${h} AM` : `${h - 12} PM`;
  }

  function toggleDay(d: number) {
    if (formDays.includes(d)) {
      formDays = formDays.filter(x => x !== d);
    } else {
      formDays = [...formDays, d].sort();
    }
  }

  function setPreset(preset: string) {
    if (preset === 'daily') formDays = [1, 2, 3, 4, 5, 6, 7];
    else if (preset === 'weekdays') formDays = [1, 2, 3, 4, 5];
  }

  function recurrenceText(): string {
    const dayPart = formDays.length === 7 ? 'daily' :
      formDays.length === 5 && formDays.every((d, i) => d === i + 1) ? 'weekdays' :
      formDays.map(d => dayLabels[d]).join(', ');
    if (formTriggerType === 'heartbeat') return `every ${formInterval}, ${dayPart}`;
    return dayPart;
  }

  /** Build a cron expression from form values. ISO weekdays (Mon=1..Sun=7) → cron (Sun=0..Sat=6). */
  function buildCron(): string {
    const cronDays = formDays.map(d => d === 7 ? 0 : d).sort().join(',');
    return `${formMinute} ${formHour} * * ${cronDays}`;
  }

  async function handleSave() {
    if (!formAgent || !formLabel.trim() || formDays.length === 0) return;
    const agentFullId = AGENT_ID_REVERSE[formAgent];
    if (!agentFullId) { formError = 'Unknown agent'; return; }

    formSaving = true;
    formError = '';
    try {
      const api = await import('$lib/api/nebo');
      const bindingName = formLabel.trim().toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');

      if (formTriggerType === 'schedule') {
        await api.createAgentWorkflow(agentFullId, {
          bindingName,
          triggerType: 'schedule',
          triggerConfig: { cron: buildCron() },
          description: formLabel.trim(),
        });
      } else {
        await api.createAgentWorkflow(agentFullId, {
          bindingName,
          triggerType: 'heartbeat',
          triggerConfig: { interval: formInterval },
          description: formLabel.trim(),
        });
      }

      // Reload schedule data so the new event appears on the calendar
      await loadScheduleFromAPI();
      onclose?.();
    } catch (e: any) {
      formError = e?.message || 'Failed to create event';
    } finally {
      formSaving = false;
    }
  }

  const showCreate = $derived(createData && !item);
  const showDetail = $derived(item && !createData);
  const visible = $derived(showCreate || showDetail);

  // Push live preview to calendar view
  $effect(() => {
    if (showCreate && formAgent) {
      preview = {
        agent: formAgent,
        hour: formHour + formMinute / 60,
        dur: 0.25,
        label: formLabel || 'New event',
      };
    } else {
      preview = null;
    }
  });
</script>

{#if !visible}
  <!-- hidden -->
{:else if showCreate}
  <!-- ─── Create Form ─────────────────────────────────────────── -->
  <div class="w-80 border-l border-base-content/10 bg-base-100 p-5 flex flex-col gap-4 shrink-0 overflow-y-auto">
    <div class="flex items-center justify-between">
      <div class="text-sm font-semibold">Schedule a Task</div>
      <button class="w-6 h-6 rounded grid place-items-center hover:text-base-content hover:bg-base-200 cursor-pointer transition-colors text-lg leading-none" onclick={onclose}>×</button>
    </div>

    <!-- Agent -->
    <div class="flex flex-col gap-1">
      <span class="text-xs font-semibold uppercase tracking-wider text-base-content/50">Agent</span>
      <select class="select select-bordered select-sm w-full" bind:value={formAgent}>
        {#each schedAgents as id}
          {@const a = AGENTS.find(x => x.id === id)}
          {#if a}
            <option value={id}>{a.name}</option>
          {/if}
        {/each}
      </select>
    </div>

    <!-- What should the agent do? -->
    <div class="flex flex-col gap-1">
      <span class="text-xs font-semibold uppercase tracking-wider text-base-content/50">What should they do?</span>
      <input type="text" class="input input-bordered input-sm w-full" placeholder="e.g. Morning email summary" bind:value={formLabel} />
    </div>

    <!-- Trigger Type -->
    <div class="flex flex-col gap-1">
      <span class="text-xs font-semibold uppercase tracking-wider text-base-content/50">How often?</span>
      <div class="inline-flex bg-base-200/80 rounded-lg p-0.5 border border-base-content/5">
        <button
          class="px-3 py-1 rounded-md text-xs cursor-pointer transition-all {formTriggerType === 'schedule' ? 'bg-base-100 font-medium text-base-content shadow-sm' : 'text-base-content/70'}"
          onclick={() => formTriggerType = 'schedule'}
        >At a time</button>
        <button
          class="px-3 py-1 rounded-md text-xs cursor-pointer transition-all {formTriggerType === 'heartbeat' ? 'bg-base-100 font-medium text-base-content shadow-sm' : 'text-base-content/70'}"
          onclick={() => formTriggerType = 'heartbeat'}
        >On an interval</button>
      </div>
    </div>

    {#if formTriggerType === 'heartbeat'}
      <!-- Interval -->
      <div class="flex flex-col gap-1">
        <span class="text-xs font-semibold uppercase tracking-wider text-base-content/50">Every</span>
        <div class="flex flex-wrap gap-1">
          {#each intervalPresets as iv}
            <button
              class="px-2 py-0.5 rounded-md text-xs cursor-pointer border transition-colors {formInterval === iv ? 'bg-primary text-primary-content border-primary' : 'bg-base-200 border-base-300 hover:bg-base-300'}"
              onclick={() => formInterval = iv}
            >{iv}</button>
          {/each}
        </div>
      </div>
    {:else}
      <!-- Time -->
      <div class="flex flex-col gap-1">
        <span class="text-xs font-semibold uppercase tracking-wider text-base-content/50">At</span>
        <div class="flex items-center gap-1.5">
          <select class="select select-bordered select-sm flex-1" bind:value={formHour}>
            {#each editHours as h}
              <option value={h}>{fmtHourLabel(h)}</option>
            {/each}
          </select>
          <span class="text-base-content/50">:</span>
          <select class="select select-bordered select-sm w-16" bind:value={formMinute}>
            {#each editMinutes as m}
              <option value={m}>{String(m).padStart(2, '0')}</option>
            {/each}
          </select>
        </div>
      </div>
    {/if}

    <!-- Repeat -->
    <div class="flex flex-col gap-1.5">
      <span class="text-xs font-semibold uppercase tracking-wider text-base-content/50">Repeat on</span>
      <div class="flex gap-1">
        <button class="px-2 py-0.5 rounded-md text-xs cursor-pointer border transition-colors {formDays.length === 7 ? 'bg-primary text-primary-content border-primary' : 'bg-base-200 border-base-300 hover:bg-base-300'}" onclick={() => setPreset('daily')}>Daily</button>
        <button class="px-2 py-0.5 rounded-md text-xs cursor-pointer border transition-colors {formDays.length === 5 && formDays.every((d, i) => d === i + 1) ? 'bg-primary text-primary-content border-primary' : 'bg-base-200 border-base-300 hover:bg-base-300'}" onclick={() => setPreset('weekdays')}>Weekdays</button>
      </div>
      <div class="flex gap-0.5">
        {#each [1, 2, 3, 4, 5, 6, 7] as d}
          <button
            class="flex-1 h-7 rounded text-xs font-medium cursor-pointer border transition-colors {formDays.includes(d) ? 'bg-primary text-primary-content border-primary' : 'bg-base-200 border-base-300 hover:bg-base-300'}"
            onclick={() => toggleDay(d)}
          >{dayLabels[d]}</button>
        {/each}
      </div>
    </div>

    <!-- Summary -->
    <div class="rounded-lg bg-base-200/50 border border-base-300 px-3 py-2">
      <div class="text-xs text-base-content/70">
        {#if formTriggerType === 'heartbeat'}
          Runs every {formInterval}, {recurrenceText()}
        {:else}
          Runs at {fmtHourLabel(formHour)}{formMinute > 0 ? `:${String(formMinute).padStart(2, '0')}` : ''}, {recurrenceText()}
        {/if}
      </div>
    </div>

    <!-- Error -->
    {#if formError}
      <div class="text-xs text-error">{formError}</div>
    {/if}

    <!-- Save -->
    <button
      class="btn btn-primary btn-sm w-full"
      disabled={!formAgent || !formLabel.trim() || formDays.length === 0 || formSaving}
      onclick={handleSave}
    >
      {#if formSaving}
        <span class="loading loading-spinner loading-xs"></span>
      {/if}
      Schedule Task
    </button>
  </div>

{:else if showDetail && item}
  <!-- ─── Detail View ─────────────────────────────────────────── -->
  <div class="w-80 border-l border-base-content/10 bg-base-100 p-5 flex flex-col gap-3 shrink-0 overflow-y-auto">
    <!-- Header -->
    <div class="flex items-start gap-3">
      <span
        class="inline-flex items-center gap-1.5 mt-0.5 px-2 py-0.5 rounded-full text-xs font-medium shrink-0 {c?.fillClass} {c?.textClass}"
      >
        <span class="w-1.5 h-1.5 rounded-full {c?.dotClass}"></span>
        {agent?.name ?? item.agent}
      </span>
      <div class="flex-1"></div>
      <button class="w-6 h-6 rounded grid place-items-center hover:text-base-content hover:bg-base-200 cursor-pointer transition-colors text-lg leading-none" onclick={onclose}>×</button>
    </div>

    <!-- Full title (never truncated) -->
    <div class="text-sm font-semibold leading-snug">{item.label}</div>

    {#if editing}
      <!-- ─── Inline Edit ──────────────────────────────────── -->
      <div class="flex flex-col gap-3 pt-2 border-t border-base-content/5">
        <!-- Time -->
        <div class="flex flex-col gap-1">
          <span class="text-xs font-semibold uppercase tracking-wider text-base-content/50">Time</span>
          <div class="flex items-center gap-1.5">
            <select class="select select-bordered select-sm flex-1" bind:value={editHour}>
              {#each editHours as h}
                <option value={h}>{fmtHourLabel(h)}</option>
              {/each}
            </select>
            <span class="text-base-content/50">:</span>
            <select class="select select-bordered select-sm w-16" bind:value={editMinute}>
              {#each editMinutes as m}
                <option value={m}>{String(m).padStart(2, '0')}</option>
              {/each}
            </select>
          </div>
        </div>

        <!-- Days -->
        <div class="flex flex-col gap-1">
          <span class="text-xs font-semibold uppercase tracking-wider text-base-content/50">Repeat on</span>
          <div class="flex gap-0.5">
            {#each [1, 2, 3, 4, 5, 6, 7] as d}
              <button
                class="flex-1 h-7 rounded text-xs font-medium cursor-pointer border transition-colors {editDays.includes(d) ? 'bg-primary text-primary-content border-primary' : 'bg-base-200 border-base-300 hover:bg-base-300'}"
                onclick={() => toggleEditDay(d)}
              >{dayLabels[d]}</button>
            {/each}
          </div>
        </div>

        <div class="flex gap-2">
          <button class="btn btn-primary btn-sm flex-1" onclick={saveEditing}>Save</button>
          <button class="btn btn-ghost btn-sm" onclick={cancelEditing}>Cancel</button>
        </div>
      </div>
    {:else}
      <!-- ─── Read-only details ────────────────────────────── -->
      <div class="flex flex-col gap-3 text-sm">
        <!-- Description (from workflow definition) -->
        {#if workflowDef?.description && workflowDef.description !== item.label}
          <div class="text-xs text-base-content/70 leading-relaxed">{workflowDef.description}</div>
        {/if}

        <!-- Schedule + Edit -->
        <div class="flex items-center justify-between">
          <div class="flex items-center gap-2">
            <span class="text-xs text-base-content/70">{fmtTime(item.hour)} – {fmtTime(item.end)}</span>
            {#if item.recurrence}
              <span class="text-xs text-base-content/50">· {item.recurrence}</span>
            {/if}
          </div>
          {#if isEditable}
            <button
              class="text-xs text-primary hover:underline cursor-pointer"
              onclick={startEditing}
            >Edit</button>
          {/if}
        </div>

        <!-- Trigger + meta badges -->
        <div class="flex flex-wrap items-center gap-1.5">
          <span class="inline-flex items-center gap-1 px-2 py-0.5 rounded-md text-xs bg-base-200 text-base-content/70">
            {triggerGlyph(item.kind)}
            {item.triggerType === 'heartbeat' ? 'Interval' : item.kind === 'sched' ? 'Scheduled' : item.kind === 'event' ? 'Event-triggered' : 'Manual'}
          </span>
          {#if workflowDef?.emit}
            <span class="inline-flex items-center gap-1 px-2 py-0.5 rounded-md text-xs bg-accent/10 text-accent">
              Emits: {workflowDef.emit}
            </span>
          {/if}
          {#if workflowDef?.isActive === false}
            <span class="inline-flex items-center gap-1 px-2 py-0.5 rounded-md text-xs bg-warning/10 text-warning">
              Paused
            </span>
          {/if}
        </div>

        <!-- Last Fired (from workflow definition) -->
        {#if workflowDef?.lastFired}
          <div class="text-xs text-base-content/50 font-mono">Last fired: {workflowDef.lastFired}</div>
        {/if}

        <!-- Workflow Activities -->
        {#if workflowActivities.length > 0}
          <div class="pt-2 border-t border-base-content/5">
            <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">
              Activities ({workflowActivities.length})
            </div>
            {#each workflowActivities as activity, i}
              {@const runActivity = item.run?.activities?.find((a) => a.id === activity.id)}
              <div class="flex items-start gap-2 py-1.5 {i < workflowActivities.length - 1 ? 'border-b border-base-content/5' : ''}">
                {#if runActivity}
                  <span class="font-mono text-xs mt-0.5 shrink-0 {statusColor(runActivity.status)}">{statusIcon(runActivity.status)}</span>
                {:else}
                  <span class="font-mono text-xs mt-0.5 shrink-0 text-base-content/30">{i + 1}.</span>
                {/if}
                <div class="flex-1 min-w-0">
                  <div class="text-xs font-medium">{activity.id}</div>
                  {#if activity.intent}
                    <div class="text-xs text-base-content/50 leading-snug">{activity.intent}</div>
                  {/if}
                  {#if activity.skills?.length}
                    <div class="flex flex-wrap gap-1 mt-1">
                      {#each activity.skills as skill}
                        <span class="px-1.5 py-0.5 rounded text-xs bg-base-200 text-base-content/50 font-mono">{skill}</span>
                      {/each}
                    </div>
                  {/if}
                </div>
                {#if runActivity?.duration}
                  <span class="font-mono text-xs text-base-content/50 shrink-0">{runActivity.duration}</span>
                {/if}
              </div>
            {/each}
          </div>
        {/if}

        <!-- Connections (activity flow) -->
        {#if workflowConnections.length > 0}
          <div class="pt-2 border-t border-base-content/5">
            <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">Flow</div>
            <div class="flex flex-col gap-1">
              {#each workflowConnections as conn}
                <div class="flex items-center gap-1.5 text-xs text-base-content/70">
                  <span class="font-mono font-medium">{conn.from}</span>
                  <span class="text-base-content/30">&rarr;</span>
                  <span class="font-mono font-medium">{conn.to}</span>
                </div>
              {/each}
            </div>
          </div>
        {/if}

        <!-- Last Run -->
        {#if item.run}
          <div class="pt-2 border-t border-base-content/5">
            <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Last Run</div>
            <div class="flex items-center gap-2 mb-1">
              <span class="inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-xs font-medium {item.run.status === 'success' ? 'bg-success/10 text-success' : item.run.status === 'failed' ? 'bg-error/10 text-error' : 'bg-base-200 text-base-content/70'}">
                {statusIcon(item.run.status)} {item.run.status}
              </span>
              {#if item.run.actualDuration}
                <span class="text-xs text-base-content/50 font-mono">{item.run.actualDuration}</span>
              {/if}
              {#if item.run.totalTokensUsed}
                <span class="text-xs text-base-content/50 font-mono">· {item.run.totalTokensUsed.toLocaleString()} tok</span>
              {/if}
            </div>
            {#if item.run.startedAt}
              <div class="text-xs text-base-content/50 mb-2">{item.run.startedAt}</div>
            {/if}

            <!-- Run output preview -->
            {#if item.run.output}
              <div class="rounded-lg bg-base-200/50 border border-base-300 p-3 mb-2">
                <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">Output</div>
                <div class="text-xs text-base-content leading-relaxed whitespace-pre-wrap break-words max-h-40 overflow-y-auto">{item.run.output.length > 500 ? item.run.output.slice(0, 500) + '...' : item.run.output}</div>
              </div>
            {/if}

            <!-- Error details -->
            {#if item.run.status === 'failed' && item.run.error}
              <div class="rounded-lg bg-error/5 border border-error/20 p-3 mb-2">
                <div class="text-xs font-semibold uppercase tracking-wider text-error/70 mb-1">Error{item.run.errorActivity ? ` in ${item.run.errorActivity}` : ''}</div>
                <div class="text-xs text-error leading-relaxed whitespace-pre-wrap break-words">{item.run.error}</div>
              </div>
            {/if}
          </div>
        {:else if !workflowDef}
          <div class="pt-2 border-t border-base-content/5">
            <div class="text-xs text-base-content/50">No runs yet</div>
          </div>
        {/if}

        <!-- Recent Runs -->
        {#if recentRuns.length > 0}
          <div class="pt-2 border-t border-base-content/5">
            <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">History</div>
            {#each recentRuns as run}
              <div class="py-1.5 border-b border-base-content/5 last:border-b-0">
                <div class="flex items-center gap-2">
                  <span class="font-mono text-xs {statusColor(run.status)}">{statusIcon(run.status)}</span>
                  <span class="flex-1 text-xs text-base-content/70 truncate">{run.date}</span>
                  <span class="font-mono text-xs text-base-content/50">{run.duration}</span>
                </div>
                {#if run.output}
                  <div class="text-xs text-base-content/50 mt-0.5 ml-5 truncate">{run.output.slice(0, 80)}{run.output.length > 80 ? '...' : ''}</div>
                {/if}
                {#if run.status === 'failed' && run.error}
                  <div class="text-xs text-error/70 mt-0.5 ml-5 truncate">{run.error.slice(0, 80)}{run.error.length > 80 ? '...' : ''}</div>
                {/if}
              </div>
            {/each}
          </div>
        {/if}

      </div>
    {/if}
  </div>
{/if}
