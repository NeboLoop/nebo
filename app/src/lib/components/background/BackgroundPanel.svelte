<!--
  A list of background work: a header that folds it away, the first few
  rows with "Show more", and the work that finished lately behind one row.
  Hidden when it has nothing to show.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import ChevronDown from 'lucide-svelte/icons/chevron-down';
  import type { BackgroundTask, FinishedTask } from '$lib/api/neboComponents';
  import { isRunning, sortForPanel } from '$lib/stores/background';
  import BackgroundRow from './BackgroundRow.svelte';

  let {
    tasks,
    finished = [],
    now,
    title = '',
    showEmployee = false,
    colors = {},
    collapsible = true,
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
  } = $props();

  /** Rows shown before "Show more". */
  const SHOWN = 4;

  let collapsed = $state(false);
  let showAll = $state(false);
  let showFinished = $state(false);
  let announce = $state('');

  const sorted = $derived(sortForPanel(tasks));
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
      </div>
    {/if}
    {#if !collapsed}
      <div class="bg-panel-rows">
        {#each visible as task (task.id)}
          <BackgroundRow {task} {now} {showEmployee} color={colors[task.agentId] ?? null} onstopped={stopped} />
        {/each}
        {#if sorted.length > SHOWN}
          <button type="button" class="bg-panel-more" onclick={() => (showAll = !showAll)}>
            {showAll ? $t('background.showLess') : $t('background.showMore')}
          </button>
        {/if}
        {#if finished.length > 0}
          <button type="button" class="bg-panel-more" onclick={() => (showFinished = !showFinished)} aria-expanded={showFinished}>
            {$t('background.finishedN', { values: { n: finished.length } })}
          </button>
          {#if showFinished}
            {#each finished as f (f.task.id + f.endedAt)}
              <div class="bg-row bg-row-finished">
                <div class="bg-row-line">
                  <span class="bg-row-main">
                    <span class="bg-row-title">
                      {#if showEmployee}<span class="bg-row-employee">{`${f.task.employee} · `}</span>{/if}{f.task.title || $t('background.heartbeat')}
                    </span>
                    <span class="bg-row-meta {f.outcome === 'failed' ? 'text-error' : ''}">{$t(`background.outcome.${f.outcome}`)}</span>
                  </span>
                </div>
              </div>
            {/each}
          {/if}
        {/if}
      </div>
    {/if}
    <span class="sr-only" aria-live="polite">{announce}</span>
  </section>
{/if}
