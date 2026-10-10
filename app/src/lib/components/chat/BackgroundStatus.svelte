<!--
  The one line above the composer: shown while the turn works or any of the
  employee's background work runs, after the turn ends too. A mark, the
  turn's time, the employee's running tasks (a button that opens them), and
  what the turn is doing. When the last task ends it says how many finished,
  briefly, then goes.
-->
<script lang="ts">
  import { onDestroy } from 'svelte';
  import { t } from 'svelte-i18n';
  import { getWebSocketClient, type ConnectionStatus } from '$lib/websocket/client';
  import { backgroundTasks, backgroundFinished, belongsTo, inStrip, clock } from '$lib/stores/background';
  import BackgroundPanel from '$lib/components/background/BackgroundPanel.svelte';
  import { widthFade } from '$lib/components/background/widthFade';

  let {
    agentId,
    isLoading = false,
    activityStatus = '',
    turnStartedAt = 0,
  }: {
    agentId: string;
    isLoading?: boolean;
    activityStatus?: string;
    /** When the running turn started (ms); 0 while none runs. */
    turnStartedAt?: number;
  } = $props();

  /** The turn's time shows once it has run this long. */
  const ELAPSED_AFTER_S = 2;
  /** How long "N tasks finished" stays. */
  const FINISHED_FLASH_MS = 1500;

  let nowMs = $state(Date.now());
  const tick = setInterval(() => (nowMs = Date.now()), 1000);
  let status = $state<ConnectionStatus>(getWebSocketClient().getStatus());
  const offStatus = getWebSocketClient().onStatus((s) => (status = s));
  onDestroy(() => {
    clearInterval(tick);
    offStatus();
    if (flashTimer) clearTimeout(flashTimer);
  });

  let open = $state(false);
  let flash = $state(0);
  let flashTimer: ReturnType<typeof setTimeout> | null = null;
  let lastCount = 0;

  const mine = $derived($backgroundTasks.filter((task) => belongsTo(task, agentId) && inStrip(task)));
  const finished = $derived($backgroundFinished.filter((f) => belongsTo(f.task, agentId) && inStrip(f.task)));
  const count = $derived(mine.length);

  $effect(() => {
    const n = count;
    if (n === 0 && lastCount > 0) {
      flash = lastCount;
      open = false;
      if (flashTimer) clearTimeout(flashTimer);
      flashTimer = setTimeout(() => (flash = 0), FINISHED_FLASH_MS);
    } else if (n > 0) {
      flash = 0;
    }
    lastCount = n;
  });

  const elapsed = $derived(isLoading && turnStartedAt > 0 ? Math.floor((nowMs - turnStartedAt) / 1000) : 0);
  const thinking = $derived(isLoading && !activityStatus && status === 'connected');
  const phrase = $derived.by(() => {
    if (!isLoading) return '';
    if (status !== 'connected') return $t('background.status.connecting');
    if (activityStatus) return activityStatus;
    if (elapsed >= 30) return $t('background.status.thinkingMore');
    if (elapsed >= 15) return $t('background.status.stillThinking');
    return $t('background.status.thinking');
  });
  const shown = $derived(isLoading || count > 0 || flash > 0);

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape') open = false;
  }
</script>

<svelte:window onkeydown={onKey} />

{#if shown}
  <div class="bg-status">
    {#if open && count > 0}
      <button type="button" class="bg-status-backdrop" aria-label={$t('common.close')} onclick={() => (open = false)}></button>
      <div class="bg-status-popover">
        <BackgroundPanel tasks={mine} {finished} now={Math.floor(nowMs / 1000)} collapsible={false} />
      </div>
    {/if}
    <div class="bg-status-row">
      {#if isLoading || count > 0}
        <span class="bg-status-piece" transition:widthFade><span class="loading loading-spinner loading-xs text-primary"></span></span>
      {/if}
      {#if elapsed >= ELAPSED_AFTER_S}
        <span class="bg-status-piece" transition:widthFade>{clock(elapsed)}</span>
      {/if}
      {#if count > 0}
        <span class="bg-status-piece" transition:widthFade>
          <button type="button" class="bg-status-tasks" aria-pressed={open} onclick={() => (open = !open)}>
            {$t('background.runningTasks', { values: { n: count } })}
          </button>
        </span>
      {:else if flash > 0}
        <span class="bg-status-piece" transition:widthFade>{$t('background.tasksFinished', { values: { n: flash } })}</span>
      {/if}
      {#if phrase}
        <span class="bg-status-piece bg-status-phrase {thinking ? 'bg-status-thinking' : ''}" transition:widthFade>{phrase}</span>
      {/if}
    </div>
  </div>
{/if}
