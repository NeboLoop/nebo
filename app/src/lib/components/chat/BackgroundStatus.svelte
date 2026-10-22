<!--
  The last line of the conversation, after the newest message (it scrolls
  with the messages): shown while the turn works or any of the
  work this conversation launched runs, after the turn ends too. A mark, the
  turn's time, the conversation's running tasks (a button that opens them), its
  code work being checked (each a button that opens its sheet), and what the
  turn is doing. When the last task ends it says how many finished,
  briefly, then goes. A task's "View transcript" opens the chat that holds
  its steps (BackgroundTranscript).
-->
<script lang="ts">
  import { onDestroy } from 'svelte';
  import { t } from 'svelte-i18n';
  import { backgroundTasks, backgroundFinished, isRunning, launchedIn, clock, waitsOnOwner } from '$lib/stores/background';
  import BackgroundPanel from '$lib/components/background/BackgroundPanel.svelte';
  import BackgroundTranscript from '$lib/components/background/BackgroundTranscript.svelte';
  import type { BackgroundTask } from '$lib/api/neboComponents';
  import { widthFade } from '$lib/components/background/widthFade';
  import { slide } from 'svelte/transition';
  import type { VerificationView } from '$lib/api/neboComponents';
  import type { VerificationAnswer } from '$lib/stores/verifications';
  import VerifiedBadge from '$lib/components/verify/VerifiedBadge.svelte';

  let {
    sessionKey,
    isLoading = false,
    activityStatus = '',
    turnStartedAt = 0,
    checking = [],
    onanswer,
  }: {
    /** The conversation's session: its list is what its turns launched. */
    sessionKey: string;
    isLoading?: boolean;
    activityStatus?: string;
    /** When the running turn started (ms); 0 while none runs. */
    turnStartedAt?: number;
    /** The conversation's code work still being checked or waiting on the
     *  owner: one piece each, "Checking · round 2 ›". */
    checking?: VerificationView[];
    /** Answers a question code work waits on, from its sheet. */
    onanswer?: VerificationAnswer;
  } = $props();

  /** The turn's time shows once it has run this long. */
  const ELAPSED_AFTER_S = 2;
  /** How long "N tasks finished" stays. */
  const FINISHED_FLASH_MS = 1500;

  let nowMs = $state(Date.now());
  const tick = setInterval(() => (nowMs = Date.now()), 1000);
  onDestroy(() => {
    clearInterval(tick);
    if (flashTimer) clearTimeout(flashTimer);
  });

  let open = $state(false);
  let popover = $state<HTMLDivElement | null>(null);
  // The list opens under its line, at the end of the conversation: brought
  // into view it lands above the composer (the scroller's scroll padding).
  $effect(() => {
    popover?.scrollIntoView({ block: 'nearest' });
  });
  let transcript = $state<BackgroundTask | null>(null);
  let flash = $state(0);
  let flashTimer: ReturnType<typeof setTimeout> | null = null;
  let lastCount = 0;

  // What this conversation's turns launched, and what that launched in turn.
  const launched = $derived(launchedIn([...$backgroundTasks, ...$backgroundFinished.map((f) => f.task)], sessionKey));
  const mine = $derived($backgroundTasks.filter((task) => isRunning(task) && launched.has(task.id)));
  const finished = $derived($backgroundFinished.filter((f) => launched.has(f.task.id)));
  const count = $derived(mine.length);
  const onYou = $derived(mine.filter(waitsOnOwner).length);

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
  // A dropped socket changes nothing here: a short drop is redialed silently,
  // and one that lasts is reported once, app-wide ($lib/websocket/connectionNotice).
  const thinking = $derived(isLoading && !activityStatus);
  const phrase = $derived.by(() => {
    if (!isLoading) return '';
    if (activityStatus) return activityStatus;
    if (elapsed >= 30) return $t('background.status.thinkingMore');
    if (elapsed >= 15) return $t('background.status.stillThinking');
    return $t('background.status.thinking');
  });
  const shown = $derived(isLoading || count > 0 || flash > 0 || checking.length > 0);

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape' && !transcript) open = false;
  }

  function showTranscript(task: BackgroundTask) {
    open = false;
    transcript = task;
  }
</script>

<svelte:window onkeydown={onKey} />

{#if shown}
  <!-- Comes and goes smoothly: at the end of the conversation, the list
       anchored to the bottom moves with it, never in a jump. -->
  <div class="bg-status" transition:slide={{ duration: 200 }}>
    <div class="bg-status-row">
      {#if isLoading || count > 0 || checking.length > 0}
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
        {#if onYou > 0}
          <span class="bg-status-piece bg-status-on-you" transition:widthFade>{$t('background.waitingOnYouN', { values: { n: onYou } })}</span>
        {/if}
      {:else if flash > 0}
        <span class="bg-status-piece" transition:widthFade>{$t('background.tasksFinished', { values: { n: flash } })}</span>
      {/if}
      {#each checking as v (v.id)}
        <span class="bg-status-piece" transition:widthFade><VerifiedBadge verification={v} statusLine {onanswer} /></span>
      {/each}
      {#if phrase}
        <span class="bg-status-piece bg-status-phrase {thinking ? 'bg-status-thinking' : ''}" transition:widthFade>{phrase}</span>
      {/if}
    </div>
    <!-- Opens in the conversation, under the line: never clipped by a short
         chat's edge, and scrolled into view above the composer. -->
    {#if open && count > 0}
      <button type="button" class="bg-status-backdrop" aria-label={$t('common.close')} onclick={() => (open = false)}></button>
      <div class="bg-status-popover" bind:this={popover}>
        <BackgroundPanel
          tasks={mine}
          {finished}
          now={Math.floor(nowMs / 1000)}
          collapsible={false}
          ontranscript={showTranscript}
          onclose={() => (open = false)}
        />
      </div>
    {/if}
  </div>
{/if}

{#if transcript}
  {#key transcript.id}
    <BackgroundTranscript task={transcript} onclose={() => (transcript = null)} />
  {/key}
{/if}
