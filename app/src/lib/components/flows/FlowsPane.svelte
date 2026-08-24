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

  // Recurring work lives in TWO stores: workflow bindings, and the event
  // tool's cron jobs ("set up a recurring weather check" in chat lands
  // there). A Flows list that only shows one of them is lying — the owner
  // must see every recurring thing this employee does, in one place.
  // Named agents' crons get migrated into workflows at startup; what remains
  // here are the primary employee's own reminders, matched by agentId.
  import * as api from '$lib/api/nebo';
  import type { CronJob } from '$lib/api/neboComponents';
  import { describeSchedule, parseSimple, buildSimple, type SimpleSchedule } from '$lib/utils/schedule';
  import ConfirmModal from '$lib/components/settings/ConfirmModal.svelte';
  import ShelfModal from '$lib/components/ui/ShelfModal.svelte';

  let reminders = $state<CronJob[]>([]);
  let reminderBusy = $state<string | null>(null);
  let deleteReminder = $state<CronJob | null>(null);

  async function loadReminders() {
    const id = ctx.agentId;
    if (!id) return;
    try {
      const r = await api.listTasks(200, 0);
      reminders = (r.tasks ?? []).filter(
        (t) => (t.agentId ?? '') === id || (id === 'assistant' && !t.agentId)
      );
    } catch { reminders = []; }
  }
  $effect(() => { void ctx.agentId; loadReminders(); });

  async function toggleReminder(t: CronJob) {
    if (reminderBusy) return;
    reminderBusy = t.name;
    try { await api.toggleTask(t.name, { enabled: t.enabled === false }); await loadReminders(); }
    catch { /* row keeps prior state */ }
    reminderBusy = null;
  }

  async function runReminder(t: CronJob) {
    if (reminderBusy) return;
    reminderBusy = t.name;
    try { await api.runTask(t.name); } catch { /* surfaced by run history */ }
    reminderBusy = null;
  }

  // Clicking a reminder card opens THE editor — a narrow modal, the same
  // "over" tier every other editor uses (cards stay read-only summaries; one
  // editing surface per object). Schedules the simple shapes can't hold stay
  // read-only — never rewrite a schedule into something it never said. Draft
  // state is flat (freq + n + time) and only becomes a SimpleSchedule at save.
  let editorReminder = $state<CronJob | null>(null);
  let draftText = $state('');
  let scheduleEditable = $state(false);
  let draftFreq = $state('daily');
  let draftN = $state(4);
  let draftHour = $state(9); // 0-23
  let draftMinute = $state(0);

  const DOW_LABELS = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];

  function startEditSchedule(t: CronJob) {
    const p = parseSimple(t.schedule);
    if (!p) return;
    if (p.kind === 'hours' || p.kind === 'minutes') {
      draftFreq = p.kind;
      draftN = p.n;
      draftHour = 9; draftMinute = 0;
    } else if (p.kind === 'weekly') {
      draftFreq = `dow${p.dow}`;
      draftHour = p.hour; draftMinute = p.minute;
      draftN = 4;
    } else {
      draftFreq = p.kind;
      draftHour = p.hour; draftMinute = p.minute;
      draftN = 4;
    }
  }

  function draftToSimple(): SimpleSchedule {
    if (draftFreq === 'hours') return { kind: 'hours', n: draftN };
    if (draftFreq === 'minutes') return { kind: 'minutes', n: draftN };
    if (draftFreq.startsWith('dow')) return { kind: 'weekly', dow: +draftFreq.slice(3), hour: draftHour, minute: draftMinute };
    return { kind: draftFreq as 'daily' | 'weekdays' | 'weekends', hour: draftHour, minute: draftMinute };
  }

  function openEditor(r: CronJob) {
    editorReminder = r;
    draftText = r.instructions || r.message || r.command || '';
    scheduleEditable = !!parseSimple(r.schedule);
    if (scheduleEditable) startEditSchedule(r);
  }

  // ONE save: text back into the field it came from, plus the schedule when
  // the simple editor could hold it.
  async function saveReminder() {
    const t = editorReminder;
    if (!t || reminderBusy) return;
    reminderBusy = t.name;
    const body: Record<string, string> = {};
    const field = t.instructions ? 'instructions' : t.message ? 'message' : t.command ? 'command' : 'instructions';
    if (draftText.trim() && draftText !== (t.instructions || t.message || t.command || '')) {
      body[field] = draftText;
    }
    if (scheduleEditable) body.schedule = buildSimple(draftToSimple());
    try {
      if (Object.keys(body).length) await api.updateTask(t.name, body);
      await loadReminders();
      editorReminder = null;
    } catch { /* keep the editor open so nothing is silently lost */ }
    reminderBusy = null;
  }

  async function confirmDeleteReminder() {
    const t = deleteReminder;
    if (!t) return;
    reminderBusy = t.name;
    try { await api.deleteTask(t.name); await loadReminders(); }
    catch { /* keep the row */ }
    reminderBusy = null;
    deleteReminder = null;
  }
  const entries = $derived(ctx.workflowEntries);
  const stats = $derived(ctx.workflowStats);

  // Verbatim from the settings section this replaced — a move should not
  // quietly change what the rows say.
  function triggerSummary(wf: WorkflowConfig): string {
    if (wf.trigger?.type === 'schedule') {
      const raw = wf.schedule || wf.trigger.cron || '';
      return raw ? describeSchedule(raw).text : 'Scheduled';
    }
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
      <!-- The numbers are doors: each tile opens the run list aimed at what
           it counts. -->
      <div class="grid grid-cols-2 gap-2 mb-1 shrink-0">
        <button
          type="button"
          class="rounded-lg border border-base-300 bg-base-100 p-2 text-center cursor-pointer hover:bg-base-200/50 transition-colors"
          onclick={() => ctx.openRuns()}
        >
          <div class="text-base font-semibold">{stats.totalRuns}</div>
          <div class="text-xs text-base-content/50">{$t('agentActivity.totalRuns')}</div>
        </button>
        <button
          type="button"
          class="rounded-lg border border-base-300 bg-base-100 p-2 text-center cursor-pointer hover:bg-base-200/50 transition-colors"
          onclick={() => ctx.openRuns(stats.failed > 0 ? 'failed' : undefined)}
        >
          <div class="text-base font-semibold {stats.failed > 0 ? 'text-error' : 'text-success'}">
            {stats.failed > 0 ? stats.failed : stats.completed}
          </div>
          <div class="text-xs text-base-content/50">
            {stats.failed > 0 ? $t('common.failed') : $t('common.completed')}
          </div>
        </button>
      </div>
    {/if}

    {#if entries.length === 0}
      <p class="text-center py-8 text-sm text-base-content/50">{$t('agentSettings.noWorkflows')}</p>
    {:else}
      {#each entries as [name, wf] (name)}
        {@const purchased = wf.source === 'marketplace'}
        <div class="rounded-lg border border-base-300 bg-base-100 overflow-hidden shrink-0">
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

    {#if reminders.length > 0}
      <div class="mt-2">
        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">{$t('flows.reminders')}</div>
        <div class="flex flex-col gap-2">
          {#each reminders as r (r.id)}
            {@const sched = describeSchedule(r.schedule)}
            <div class="rounded-lg border border-base-300 bg-base-100 p-3 flex items-start gap-2.5 shrink-0">
              <div class="w-[22px] h-[22px] rounded flex items-center justify-center text-sm shrink-0 mt-0.5 {r.enabled !== false ? 'bg-primary/10 text-primary' : 'bg-base-200 text-base-content/40'}">&#8986;</div>
              <div class="flex-1 min-w-0">
                <!-- The card is a summary; editing lives in ONE place — the modal. -->
                <button
                  type="button"
                  class="w-full text-left bg-transparent border-none p-0 cursor-pointer"
                  onclick={() => openEditor(r)}
                >
                  <div class="flex items-center gap-1.5 flex-wrap">
                    <span class="text-sm font-medium">{r.name}</span>
                    {#if r.enabled === false}
                      <span class="py-0 px-1.5 rounded bg-base-200 text-xs text-base-content/50">{$t('common.paused')}</span>
                    {/if}
                  </div>
                  {#if r.instructions || r.message || r.command}
                    <div class="text-xs text-base-content/70 mt-0.5 line-clamp-2">{r.instructions || r.message || r.command}</div>
                  {/if}
                  <div class="flex items-center gap-1.5 mt-1.5 flex-wrap">
                    <span class="text-xs text-base-content/50 {sched.isCron ? 'font-mono' : ''}" title={parseSimple(r.schedule) ? undefined : 'This schedule is more specific than the simple editor can hold'}>{sched.text}</span>
                    {#if r.lastRun}
                      <span class="text-xs text-base-content/30">&middot;</span>
                      <span class="text-xs text-base-content/50 font-mono">{r.lastRun}</span>
                    {/if}
                    {#if r.lastError}
                      <span class="text-xs text-error font-mono truncate">{r.lastError}</span>
                    {/if}
                  </div>
                </button>
                <div class="flex items-center gap-2 mt-2">
                  <button class="btn btn-ghost btn-xs" disabled={reminderBusy === r.name} onclick={() => runReminder(r)}>{$t('flows.runNow')}</button>
                  <button class="btn btn-ghost btn-xs text-error ml-auto" disabled={reminderBusy === r.name} onclick={() => (deleteReminder = r)}>{$t('common.delete')}</button>
                </div>
              </div>
              <input
                type="checkbox"
                class="toggle toggle-sm toggle-primary shrink-0 mt-1"
                checked={r.enabled !== false}
                disabled={reminderBusy === r.name}
                role="switch"
                aria-checked={r.enabled !== false}
                onchange={() => toggleReminder(r)}
              />
            </div>
          {/each}
        </div>
      </div>
    {/if}

    <button
      class="mt-1 w-full py-2.5 rounded-lg border border-dashed border-base-300 text-sm text-primary font-medium cursor-pointer bg-transparent hover:bg-base-200 transition-colors"
      onclick={() => onask(`Set up a new flow for me: `)}
    >{$t('flows.askSetup', { values: { name: ctx.agent?.name ?? $t('chat.yourEmployee') } })}</button>
  </div>
</div>

<!-- THE reminder editor — the same "over" tier as every other editor. -->
<ShelfModal
  narrow
  open={editorReminder !== null}
  title={editorReminder?.name ?? ''}
  onclose={() => (editorReminder = null)}
>
  {#if editorReminder}
    <div class="flex-1 min-w-0 flex flex-col gap-3 p-4 overflow-y-auto">
      <div>
        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">{$t('flows.editorInstructions')}</div>
        <textarea
          class="w-full py-2 px-2.5 rounded-md border border-base-300 text-sm max-md:text-base bg-base-100 outline-none resize-y leading-relaxed min-h-32"
          bind:value={draftText}
        ></textarea>
      </div>
      <div>
        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">{$t('flows.editorSchedule')}</div>
        {#if scheduleEditable}
          <div class="flex items-center gap-1.5 flex-wrap">
            <select class="select select-sm bg-base-100 border-base-300" bind:value={draftFreq}>
              <option value="daily">Every day</option>
              <option value="weekdays">Weekdays</option>
              <option value="weekends">Weekends</option>
              {#each DOW_LABELS as d, i (i)}
                <option value={`dow${i}`}>{d}s</option>
              {/each}
              <option value="hours">Every N hours</option>
              <option value="minutes">Every N minutes</option>
            </select>
            {#if draftFreq === 'hours' || draftFreq === 'minutes'}
              <input type="number" min="1" max={draftFreq === 'hours' ? 23 : 59} class="input input-sm w-16 bg-base-100 border-base-300" bind:value={draftN} />
            {:else}
              <select class="select select-sm bg-base-100 border-base-300"
                value={String(draftHour % 12 === 0 ? 12 : draftHour % 12)}
                onchange={(e) => { const h12 = +e.currentTarget.value; draftHour = (h12 % 12) + (draftHour >= 12 ? 12 : 0); }}>
                {#each Array.from({ length: 12 }, (_, i) => i + 1) as h (h)}<option value={String(h)}>{h}</option>{/each}
              </select>
              <select class="select select-sm bg-base-100 border-base-300" bind:value={draftMinute}>
                {#each [0, 15, 30, 45] as mnt (mnt)}<option value={mnt}>:{String(mnt).padStart(2, '0')}</option>{/each}
              </select>
              <select class="select select-sm bg-base-100 border-base-300"
                value={draftHour >= 12 ? 'PM' : 'AM'}
                onchange={(e) => { draftHour = (draftHour % 12) + (e.currentTarget.value === 'PM' ? 12 : 0); }}>
                <option>AM</option><option>PM</option>
              </select>
            {/if}
          </div>
        {:else}
          <!-- More specific than the simple shapes can hold — display, never rewrite. -->
          <div class="text-sm text-base-content/70 font-mono">{describeSchedule(editorReminder.schedule).text}</div>
        {/if}
      </div>
      <div class="flex items-center gap-2 mt-1">
        <button class="btn btn-primary btn-sm" disabled={reminderBusy === editorReminder.name} onclick={saveReminder}>{$t('common.save')}</button>
        <button class="btn btn-ghost btn-sm" onclick={() => (editorReminder = null)}>{$t('common.cancel')}</button>
      </div>
    </div>
  {/if}
</ShelfModal>

{#if deleteReminder}
  <ConfirmModal
    title={$t('flows.deleteReminderTitle', { values: { name: deleteReminder.name } })}
    message={$t('flows.deleteReminderBody')}
    confirmLabel={$t('common.delete')}
    busy={reminderBusy === deleteReminder.name}
    onConfirm={confirmDeleteReminder}
    onCancel={() => (deleteReminder = null)}
  />
{/if}
