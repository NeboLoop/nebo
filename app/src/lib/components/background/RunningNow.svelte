<!--
  The workspace's "Running now": every employee's background work, grouped
  into what works or waits now, the timers set to fire, and the watches that
  start work when something happens, with the stop that stops everything.
-->
<script lang="ts">
  import { onDestroy } from 'svelte';
  import { t } from 'svelte-i18n';
  import { backgroundTasks, backgroundFinished, isRunning, stopEverything } from '$lib/stores/background';
  import BackgroundPanel from './BackgroundPanel.svelte';

  let { colors = {} }: { colors?: Record<string, string> } = $props();

  let now = $state(Math.floor(Date.now() / 1000));
  const tick = setInterval(() => (now = Math.floor(Date.now() / 1000)), 1000);
  onDestroy(() => clearInterval(tick));

  let confirming = $state(false);
  let stopping = $state(false);
  let result = $state('');

  const working = $derived($backgroundTasks.filter(isRunning));
  const timers = $derived($backgroundTasks.filter((task) => !isRunning(task) && task.kind === 'loop'));
  const watches = $derived($backgroundTasks.filter((task) => !isRunning(task) && task.kind !== 'loop'));

  async function stopAll() {
    stopping = true;
    try {
      const n = await stopEverything();
      result = $t('background.stoppedN', { values: { n } });
    } catch (e) {
      result = $t('background.actionFailed', { values: { error: (e as Error)?.message ?? String(e) } });
    } finally {
      stopping = false;
      confirming = false;
    }
  }
</script>

<section class="min-w-0">
  <div class="flex flex-wrap items-baseline gap-3 mb-3">
    <h2 class="text-[15px] font-semibold">{$t('background.runningNow')}</h2>
    <span class="text-xs text-base-content/50">{$t('background.runningNowSub')}</span>
    {#if working.length > 0}
      <div class="ms-auto flex items-center gap-2">
        {#if confirming}
          <span class="text-xs text-base-content/70">{$t('background.stopEverythingConfirm')}</span>
          <button type="button" class="btn btn-error btn-xs rounded-full" disabled={stopping} onclick={stopAll}>
            {#if stopping}<span class="loading loading-spinner loading-xs"></span>{/if}
            {$t('background.stopEverything')}
          </button>
          <button type="button" class="btn btn-ghost btn-xs rounded-full" onclick={() => (confirming = false)}>{$t('common.cancel')}</button>
        {:else}
          <button type="button" class="btn btn-outline btn-error btn-xs rounded-full" onclick={() => { result = ''; confirming = true; }}>
            {$t('background.stopEverything')}
          </button>
        {/if}
      </div>
    {/if}
  </div>
  {#if result}<div class="text-xs text-base-content/60 mb-2" aria-live="polite">{result}</div>{/if}
  {#if $backgroundTasks.length === 0 && $backgroundFinished.length === 0}
    <div class="bg-panel bg-panel-empty">{$t('background.nothingRunning')}</div>
  {:else}
    <div class="grid gap-3 md:grid-cols-3 min-w-0">
      <BackgroundPanel tasks={working} finished={$backgroundFinished} {now} title={$t('background.group.running')} showEmployee {colors} />
      <BackgroundPanel tasks={timers} {now} title={$t('background.group.scheduled')} showEmployee {colors} />
      <BackgroundPanel tasks={watches} {now} title={$t('background.group.watching')} showEmployee {colors} />
    </div>
  {/if}
</section>
