<script lang="ts">
  import { AGENT_COLORS } from '$lib/tokens.js';
  import { AGENTS } from '$lib/data.js';
  import { triggerGlyph, fmtTime } from '$lib/utils.js';
  import { getRecentRuns, addUserItem, getScheduleAgents, snapTo15, userScheduleItems, updateUserItem } from '$lib/stores/schedule.js';
  import type { CalendarItem } from '$lib/stores/schedule.js';

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

  // Workflow definition — loaded from API via schedule store, no longer from mock data
  interface WorkflowDef {
    activities: { id: string; intent: string }[];
  }
  const workflowDef = $derived.by((): WorkflowDef | null => {
    return null; // TODO: load from listAgentWorkflows() API when item selected
  });
  const workflowActivities = $derived(workflowDef?.activities ?? []);

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
  let formDurMinutes = $state(30);
  let formDays = $state<number[]>([1, 2, 3, 4, 5]);
  let formTriggerType = $state<'schedule' | 'heartbeat'>('schedule');
  let formInterval = $state('30m');

  const dayLabels = ['', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
  const durationPresets = [15, 30, 45, 60, 90, 120];
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
      formDurMinutes = 30;
      formTriggerType = 'schedule';
      formInterval = '30m';
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

  function handleSave() {
    if (!formAgent || !formLabel.trim() || formDays.length === 0) return;
    const fractionalHour = formHour + formMinute / 60;
    addUserItem({
      agent: formAgent,
      agentFull: '',
      label: formLabel.trim(),
      days: formDays,
      hour: fractionalHour,
      dur: formDurMinutes / 60,
      triggerType: formTriggerType,
      recurrence: recurrenceText(),
    });
    onclose?.();
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
        dur: formDurMinutes / 60,
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
  <div class="w-72 border-l border-base-content/10 bg-base-100 p-5 flex flex-col gap-4 shrink-0 overflow-y-auto">
    <div class="flex items-center justify-between">
      <div class="text-sm font-semibold">New Scheduled Event</div>
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

    <!-- Label -->
    <div class="flex flex-col gap-1">
      <span class="text-xs font-semibold uppercase tracking-wider text-base-content/50">Label</span>
      <input type="text" class="input input-bordered input-sm w-full" placeholder="e.g. Morning scan" bind:value={formLabel} />
    </div>

    <!-- Trigger Type -->
    <div class="flex flex-col gap-1">
      <span class="text-xs font-semibold uppercase tracking-wider text-base-content/50">Trigger</span>
      <div class="inline-flex bg-base-200/80 rounded-lg p-0.5 border border-base-content/5">
        <button
          class="px-3 py-1 rounded-md text-xs cursor-pointer transition-all {formTriggerType === 'schedule' ? 'bg-base-100 font-medium text-base-content shadow-sm' : 'text-base-content/70'}"
          onclick={() => formTriggerType = 'schedule'}
        >Schedule</button>
        <button
          class="px-3 py-1 rounded-md text-xs cursor-pointer transition-all {formTriggerType === 'heartbeat' ? 'bg-base-100 font-medium text-base-content shadow-sm' : 'text-base-content/70'}"
          onclick={() => formTriggerType = 'heartbeat'}
        >Interval</button>
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
        <span class="text-xs font-semibold uppercase tracking-wider text-base-content/50">Time</span>
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

    <!-- Duration -->
    <div class="flex flex-col gap-1">
      <span class="text-xs font-semibold uppercase tracking-wider text-base-content/50">Duration</span>
      <div class="flex flex-wrap gap-1">
        {#each durationPresets as d}
          <button
            class="px-2 py-0.5 rounded-md text-xs cursor-pointer border transition-colors {formDurMinutes === d ? 'bg-primary text-primary-content border-primary' : 'bg-base-200 border-base-300 hover:bg-base-300'}"
            onclick={() => formDurMinutes = d}
          >{d < 60 ? `${d}m` : `${d / 60}h`}</button>
        {/each}
      </div>
    </div>

    <!-- Recurrence -->
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

    <!-- Save -->
    <button
      class="btn btn-primary btn-sm w-full mt-1"
      disabled={!formAgent || !formLabel.trim() || formDays.length === 0}
      onclick={handleSave}
    >Create Event</button>
  </div>

{:else if showDetail && item}
  <!-- ─── Detail View ─────────────────────────────────────────── -->
  <div class="w-80 border-l border-base-content/10 bg-base-100 p-5 flex flex-col gap-3 shrink-0 overflow-y-auto">
    <!-- Header -->
    <div class="flex items-start gap-3">
      <span class="font-mono text-lg shrink-0 {c?.textClass}">
        {triggerGlyph(item.kind)}
      </span>
      <div class="flex-1 min-w-0">
        <div class="text-sm font-semibold truncate">{item.label}</div>
        <span
          class="inline-flex items-center gap-1.5 mt-1 px-2 py-0.5 rounded-full text-xs font-medium {c?.fillClass} {c?.textClass}"
        >
          <span class="w-1.5 h-1.5 rounded-full {c?.dotClass}"></span>
          {agent?.name ?? item.agent}
        </span>
      </div>
      <button class="w-6 h-6 rounded grid place-items-center hover:text-base-content hover:bg-base-200 cursor-pointer transition-colors text-lg leading-none" onclick={onclose}>×</button>
    </div>

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

        <!-- Duration -->
        <div class="flex flex-col gap-1">
          <span class="text-xs font-semibold uppercase tracking-wider text-base-content/50">Duration</span>
          <div class="flex flex-wrap gap-1">
            {#each durationPresets as d}
              <button
                class="px-2 py-0.5 rounded-md text-xs cursor-pointer border transition-colors {editDurMinutes === d ? 'bg-primary text-primary-content border-primary' : 'bg-base-200 border-base-300 hover:bg-base-300'}"
                onclick={() => editDurMinutes = d}
              >{d < 60 ? `${d}m` : `${d / 60}h`}</button>
            {/each}
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
        <!-- When -->
        <div>
          <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-0.5">When</div>
          <div class="flex items-center justify-between">
            <div>
              <div class="text-base-content">{fmtTime(item.hour)} – {fmtTime(item.end)}</div>
              {#if item.recurrence}
                <div class="text-xs text-base-content/70 mt-0.5">{item.recurrence}</div>
              {/if}
            </div>
            {#if isEditable}
              <button
                class="text-xs text-primary hover:underline cursor-pointer"
                onclick={startEditing}
              >Edit</button>
            {/if}
          </div>
        </div>

        <!-- Last Run -->
        {#if item.run}
          <div>
            <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-0.5">Last Run</div>
            <div class="flex items-center gap-2">
              <span class="font-mono text-xs {statusColor(item.run.status)}">{statusIcon(item.run.status)}</span>
              <span class="text-base-content font-mono text-xs">{item.run.actualDuration}</span>
              {#if item.run.tokens}
                <span class="text-xs text-base-content/50">· {item.run.tokens.input.toLocaleString()} in / {item.run.tokens.output.toLocaleString()} out</span>
              {/if}
            </div>
            {#if item.run.startedAt}
              <div class="text-xs text-base-content/50 mt-0.5">{item.run.startedAt}</div>
            {/if}
          </div>
        {/if}

        <!-- Trigger -->
        <div>
          <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-0.5">Trigger</div>
          <div class="text-base-content text-xs">
            {item.triggerType === 'heartbeat' ? 'Heartbeat · interval' : item.kind === 'sched' ? 'Scheduled · recurring' : item.kind === 'event' ? 'Event · webhook' : 'You · manual'}
          </div>
        </div>

        <!-- Open in Canvas -->
        {#if item.agentFull && item.workflowId && onopencanvas}
          <button
            class="btn btn-sm btn-ghost gap-1.5 w-full justify-start text-primary"
            onclick={() => onopencanvas(item.agentFull)}
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="7" height="7" rx="1"/><rect x="14" y="3" width="7" height="7" rx="1"/><rect x="8" y="14" width="7" height="7" rx="1"/><line x1="6.5" y1="10" x2="11.5" y2="14"/><line x1="17.5" y1="10" x2="11.5" y2="14"/></svg>
            <span class="text-xs">Open in Canvas</span>
          </button>
        {/if}

        <!-- Workflow Activities -->
        {#if workflowActivities.length > 0}
          <div class="pt-2 border-t border-base-content/5">
            <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">Workflow</div>
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
                  <div class="text-xs text-base-content/50 leading-snug">{activity.intent}</div>
                </div>
                {#if runActivity?.duration}
                  <span class="font-mono text-xs text-base-content/50 shrink-0">{runActivity.duration}</span>
                {/if}
              </div>
            {/each}
          </div>
        {/if}

        <!-- Recent Runs -->
        {#if recentRuns.length > 0}
          <div class="pt-2 border-t border-base-content/5">
            <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">Recent Runs</div>
            {#each recentRuns as run}
              <div class="flex items-center gap-2 py-1 border-b border-base-content/5 last:border-b-0">
                <span class="font-mono text-xs {statusColor(run.status)}">{statusIcon(run.status)}</span>
                <span class="flex-1 text-xs text-base-content/70 truncate">{run.date}</span>
                <span class="font-mono text-xs text-base-content/50">{run.duration}</span>
              </div>
            {/each}
          </div>
        {/if}
      </div>
    {/if}
  </div>
{/if}
