<script lang="ts">
  import { t } from 'svelte-i18n';
  import { AGENTS } from '$lib/data.js';
  import { AGENT_COLORS } from '$lib/tokens.js';
  import { addUserItem, getScheduleAgents, snapTo15, userScheduleItems } from '$lib/stores/schedule.js';

  let { open = $bindable(false), hour = 9, date = new Date() } = $props();

  const schedAgents = $derived(getScheduleAgents($userScheduleItems));

  let selectedAgent = $state('');
  let label = $state('');
  let selectedHour = $state(9);
  let selectedMinute = $state(0);
  let durMinutes = $state(30);
  let days = $state<number[]>([1, 2, 3, 4, 5]);

  // Reset form when modal opens
  $effect(() => {
    if (open) {
      const snapped = snapTo15(hour);
      selectedHour = Math.floor(snapped);
      selectedMinute = Math.round((snapped - Math.floor(snapped)) * 60);
      selectedAgent = schedAgents[0] || '';
      label = '';
      durMinutes = 30;
      const wd = date.getDay() === 0 ? 7 : date.getDay();
      days = [wd];
    }
  });

  const hours = Array.from({ length: 24 }, (_, i) => i);
  const minutes = [0, 15, 30, 45];
  const durationPresets = [15, 30, 45, 60, 90, 120];
  const dayLabels = ['', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
  const dayKeys = ['', 'weekdays.mon', 'weekdays.tue', 'weekdays.wed', 'weekdays.thu', 'weekdays.fri', 'weekdays.sat', 'weekdays.sun'];

  function toggleDay(d: number) {
    if (days.includes(d)) {
      days = days.filter(x => x !== d);
    } else {
      days = [...days, d].sort();
    }
  }

  function setPreset(preset: string) {
    if (preset === 'daily') days = [1, 2, 3, 4, 5, 6, 7];
    else if (preset === 'weekdays') days = [1, 2, 3, 4, 5];
  }

  function fmtHourLabel(h: number): string {
    if (h === 0) return '12 AM';
    if (h === 12) return '12 PM';
    return h < 12 ? `${h} AM` : `${h - 12} PM`;
  }

  function handleSave() {
    if (!selectedAgent || !label.trim() || days.length === 0) return;
    const fractionalHour = selectedHour + selectedMinute / 60;
    addUserItem({
      agent: selectedAgent,
      agentFull: '',
      label: label.trim(),
      days,
      hour: fractionalHour,
      dur: durMinutes / 60,
      triggerType: 'schedule',
      recurrence: days.length === 7 ? 'daily' : days.length === 5 && days.every((d, i) => d === i + 1) ? 'weekdays' : days.map(d => dayLabels[d]).join(', '),
    });
    open = false;
  }
</script>

{#if open}
  <div class="fixed inset-0 z-50 flex items-center justify-center">
    <div class="absolute inset-0 bg-base-content/30" onclick={() => open = false} role="presentation"></div>
    <div class="relative bg-base-100 rounded-xl border border-base-300 shadow-lg w-[400px] max-h-[90vh] overflow-y-auto p-5 flex flex-col gap-4">
      <div class="flex items-center justify-between">
        <h3 class="text-base font-semibold">{$t('scheduleEvent.title')}</h3>
        <button class="w-7 h-7 rounded-md flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none text-lg" onclick={() => open = false}>×</button>
      </div>

      <!-- Agent -->
      <div class="flex flex-col gap-1">
        <label class="text-xs font-semibold uppercase tracking-wider text-base-content/50" for="sched-agent">{$t('common.agent')}</label>
        <select id="sched-agent" class="select select-bordered select-sm w-full" bind:value={selectedAgent}>
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
        <label class="text-xs font-semibold uppercase tracking-wider text-base-content/50" for="sched-label">{$t('scheduleEvent.label')}</label>
        <input id="sched-label" type="text" class="input input-bordered input-sm w-full" placeholder={$t('scheduleEvent.labelPlaceholder')} bind:value={label} />
      </div>

      <!-- Time -->
      <div class="flex flex-col gap-1">
        <span class="text-xs font-semibold uppercase tracking-wider text-base-content/50">{$t('scheduleEvent.time')}</span>
        <div class="flex items-center gap-2">
          <select class="select select-bordered select-sm flex-1" bind:value={selectedHour}>
            {#each hours as h}
              <option value={h}>{fmtHourLabel(h)}</option>
            {/each}
          </select>
          <span class="text-base-content/50">:</span>
          <select class="select select-bordered select-sm w-20" bind:value={selectedMinute}>
            {#each minutes as m}
              <option value={m}>{String(m).padStart(2, '0')}</option>
            {/each}
          </select>
        </div>
      </div>

      <!-- Duration -->
      <div class="flex flex-col gap-1">
        <span class="text-xs font-semibold uppercase tracking-wider text-base-content/50">{$t('scheduleEvent.duration')}</span>
        <div class="flex flex-wrap gap-1.5">
          {#each durationPresets as d}
            <button
              class="px-2.5 py-1 rounded-md text-sm cursor-pointer border transition-colors {durMinutes === d ? 'bg-primary text-primary-content border-primary' : 'bg-base-200 border-base-300 hover:bg-base-300'}"
              onclick={() => durMinutes = d}
            >{d < 60 ? `${d}m` : `${d / 60}h`}</button>
          {/each}
        </div>
      </div>

      <!-- Recurrence -->
      <div class="flex flex-col gap-1.5">
        <span class="text-xs font-semibold uppercase tracking-wider text-base-content/50">{$t('scheduleEvent.repeatOn')}</span>
        <div class="flex gap-1.5">
          <button class="px-2 py-1 rounded-md text-xs cursor-pointer border transition-colors {days.length === 7 ? 'bg-primary text-primary-content border-primary' : 'bg-base-200 border-base-300 hover:bg-base-300'}" onclick={() => setPreset('daily')}>{$t('scheduleEvent.daily')}</button>
          <button class="px-2 py-1 rounded-md text-xs cursor-pointer border transition-colors {days.length === 5 && days.every((d, i) => d === i + 1) ? 'bg-primary text-primary-content border-primary' : 'bg-base-200 border-base-300 hover:bg-base-300'}" onclick={() => setPreset('weekdays')}>{$t('scheduleEvent.weekdays')}</button>
        </div>
        <div class="flex gap-1">
          {#each [1, 2, 3, 4, 5, 6, 7] as d}
            <button
              class="w-9 h-8 rounded-md text-xs font-medium cursor-pointer border transition-colors {days.includes(d) ? 'bg-primary text-primary-content border-primary' : 'bg-base-200 border-base-300 hover:bg-base-300'}"
              onclick={() => toggleDay(d)}
            >{$t(dayKeys[d])}</button>
          {/each}
        </div>
      </div>

      <!-- Actions -->
      <div class="flex justify-end gap-2 pt-2 border-t border-base-content/10">
        <button class="btn btn-ghost btn-sm" onclick={() => open = false}>{$t('common.cancel')}</button>
        <button
          class="btn btn-primary btn-sm"
          disabled={!selectedAgent || !label.trim() || days.length === 0}
          onclick={handleSave}
        >{$t('settingsAdvisors.create')}</button>
      </div>
    </div>
  </div>
{/if}
