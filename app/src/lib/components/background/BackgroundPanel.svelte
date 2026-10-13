<!--
  A list of background work: a header that folds it away, the first few
  rows with "Show more", and the work that finished lately in its own
  section that folds open. A chat's list (not collapsible) has a close
  button, and its running work is a section that folds too. Hidden when it
  has nothing to show.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import ChevronDown from 'lucide-svelte/icons/chevron-down';
  import X from 'lucide-svelte/icons/x';
  import type { BackgroundTask, FinishedTask } from '$lib/api/neboComponents';
  import { isRunning, rollUp, sortForPanel } from '$lib/stores/background';
  import BackgroundRow from './BackgroundRow.svelte';

  let {
    tasks,
    finished = [],
    now,
    title = '',
    showEmployee = false,
    colors = {},
    collapsible = true,
    ontranscript,
    onclose,
  }: {
    tasks: BackgroundTask[];
    finished?: FinishedTask[];
    now: number;
    /** The header; "Background tasks" when empty. */
    title?: string;
    showEmployee?: boolean;
    /** Employee colours, by id, for their avatars. */
    colors?: Record<string, string>;
    collapsible?: boolean;
    /** Open a row's transcript (the chat that holds its steps). */
    ontranscript?: (task: BackgroundTask) => void;
    /** Close the list (a chat's list). */
    onclose?: () => void;
  } = $props();

  /** Rows shown before "Show more". */
  const SHOWN = 4;

  let collapsed = $state(false);
  let showAll = $state(false);
  let showRunning = $state(true);
  let showFinished = $state(false);
  let announce = $state('');

  // Helpers a helper started follow their parent, a level in.
  const sorted = $derived(rollUp(sortForPanel(tasks)));
  const visible = $derived(showAll ? sorted : sorted.slice(0, SHOWN));
  const running = $derived(tasks.filter(isRunning).length);

  function stopped() {
    announce = '';
    queueMicrotask(() => (announce = $t('background.taskStopped')));
  }
</script>

{#if tasks.length > 0 || finished.length > 0}
  <section class="bg-panel">
    {#if collapsible}
      <button type="button" class="bg-panel-header" onclick={() => (collapsed = !collapsed)} aria-expanded={!collapsed}>
        <span class="bg-panel-title">{title || $t('background.title')}</span>
        {#if collapsed && running > 0}<span class="bg-panel-count">{$t('background.runningN', { values: { n: running } })}</span>{/if}
        <ChevronDown class="bg-panel-chevron {collapsed ? '' : 'rotate-180'}" aria-hidden="true" />
      </button>
    {:else}
      <div class="bg-panel-header">
        <span class="bg-panel-title">{title || $t('background.title')}</span>
        {#if onclose}
          <button type="button" class="bg-panel-close" aria-label={$t('common.close')} onclick={onclose}><X class="w-3.5 h-3.5" /></button>
        {/if}
      </div>
    {/if}
    {#if !collapsed}
      <div class="bg-panel-rows">
        {#if !collapsible && tasks.length > 0}
          <button type="button" class="bg-panel-section" onclick={() => (showRunning = !showRunning)} aria-expanded={showRunning}>
            {$t('background.section.running')}
            <ChevronDown class="bg-panel-chevron {showRunning ? 'rotate-180' : ''}" aria-hidden="true" />
          </button>
        {/if}
        {#if collapsible || showRunning}
          {#each visible as { task, depth } (task.id)}
            <BackgroundRow {task} {now} {depth} {showEmployee} color={colors[task.agentId] ?? null} onstopped={stopped} {ontranscript} />
          {/each}
          {#if sorted.length > SHOWN}
            <button type="button" class="bg-panel-more" onclick={() => (showAll = !showAll)}>
              {showAll ? $t('background.showLess') : $t('background.showMore')}
            </button>
          {/if}
        {/if}
        {#if finished.length > 0}
          <button type="button" class="bg-panel-section" onclick={() => (showFinished = !showFinished)} aria-expanded={showFinished}>
            {$t('background.finishedN', { values: { n: finished.length } })}
            <ChevronDown class="bg-panel-chevron {showFinished ? 'rotate-180' : ''}" aria-hidden="true" />
          </button>
          {#if showFinished}
            {#each finished as f (f.task.id + f.endedAt)}
              <BackgroundRow task={f.task} {now} {showEmployee} color={colors[f.task.agentId] ?? null} outcome={f.outcome} {ontranscript} />
            {/each}
          {/if}
        {/if}
      </div>
    {/if}
    <span class="sr-only" aria-live="polite">{announce}</span>
  </section>
{/if}
