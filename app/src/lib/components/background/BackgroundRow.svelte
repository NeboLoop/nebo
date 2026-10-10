<!--
  One piece of background work: what it is, how long it has run (or when it
  fires next), and the owner's actions on it. A click opens the end of its
  output and what started it.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import Clock from 'lucide-svelte/icons/clock';
  import Terminal from 'lucide-svelte/icons/terminal';
  import Bot from 'lucide-svelte/icons/bot';
  import Square from 'lucide-svelte/icons/square';
  import type { BackgroundAction, BackgroundTask } from '$lib/api/neboComponents';
  import { actOn, outputOf, shortClock, isRunning } from '$lib/stores/background';
  import AgentAvatar from '$lib/components/AgentAvatar.svelte';

  let {
    task,
    now,
    showEmployee = false,
    color = null,
    onstopped,
  }: {
    task: BackgroundTask;
    /** Now, in unix seconds, ticking. */
    now: number;
    /** Name the employee it belongs to (a list across employees). */
    showEmployee?: boolean;
    color?: string | null;
    /** It was stopped by the owner from this row. */
    onstopped?: () => void;
  } = $props();

  let open = $state(false);
  let busy = $state<BackgroundAction | null>(null);
  let stopping = $state(false);
  let error = $state('');
  let output = $state<{ output: string; truncated: boolean } | null>(null);
  let outputFailed = $state(false);

  const title = $derived(task.title || (task.source === 'heartbeat' ? $t('background.heartbeat') : task.detail));
  const stopAction = $derived(task.actions.find((a) => a === 'stop' || a === 'cancel'));
  const otherActions = $derived(task.actions.filter((a) => a !== stopAction));

  const meta = $derived.by(() => {
    if (stopping) return $t('background.stopping');
    if (task.status === 'waiting') return $t(`background.wait.${task.wait ?? 'approval'}`);
    if (task.status === 'degraded') return $t('background.degraded');
    if (isRunning(task)) return task.startedAt ? shortClock(now - task.startedAt) : '';
    if (task.nextRunAt) return $t('background.next', { values: { when: shortClock(task.nextRunAt - now) } });
    if (task.status === 'watching') return $t(`background.trigger.${task.trigger ?? 'event'}`);
    return '';
  });

  async function act(action: BackgroundAction) {
    if (busy) return;
    busy = action;
    error = '';
    try {
      await actOn(task.id, action);
      if (action === 'stop' || action === 'cancel') {
        stopping = true;
        onstopped?.();
      }
    } catch (e) {
      error = $t('background.actionFailed', { values: { error: (e as Error)?.message ?? String(e) } });
    } finally {
      busy = null;
    }
  }

  async function toggle() {
    open = !open;
    if (!open || output) return;
    try {
      output = await outputOf(task.id);
    } catch {
      outputFailed = true;
    }
  }
</script>

<div class="bg-row {open ? 'bg-row-open' : ''}">
  <div class="bg-row-line">
    <button type="button" class="bg-row-main" onclick={toggle} aria-expanded={open}>
      <span class="bg-row-icon" aria-hidden="true">
        {#if task.kind === 'loop'}<Clock class="w-3.5 h-3.5" />{:else if task.kind === 'shell'}<Terminal class="w-3.5 h-3.5" />{:else if showEmployee}<AgentAvatar name={task.employee} {color} size="xs" />{:else}<Bot class="w-3.5 h-3.5" />{/if}
      </span>
      <span class="bg-row-title">
        {#if showEmployee}<span class="bg-row-employee">{`${task.employee} · `}</span>{/if}{title}
      </span>
      <span class="bg-row-meta">{meta}</span>
    </button>
    {#if stopAction && !stopping}
      <button
        type="button"
        class="bg-row-stop"
        onclick={() => act(stopAction)}
        disabled={busy !== null}
        aria-label={$t(`background.action.${stopAction}`)}
        title={$t(`background.action.${stopAction}`)}
      >
        {#if busy === stopAction}<span class="loading loading-spinner loading-xs"></span>{:else}<Square class="w-3 h-3" />{/if}
      </button>
    {/if}
  </div>
  {#if open}
    <div class="bg-row-detail">
      {#if task.detail && task.detail !== title}<div class="bg-row-detail-line">{task.detail}</div>{/if}
      <div class="bg-row-detail-line bg-row-muted">
        {$t(`background.createdBy.${task.createdBy}`)}{#if task.turnsUsed != null && task.turnsCap != null} · {$t('background.turns', { values: { used: task.turnsUsed, cap: task.turnsCap } })}{/if}
      </div>
      {#if outputFailed}
        <div class="bg-output bg-row-muted">{$t('background.outputUnavailable')}</div>
      {:else if output}
        {#if output.truncated}<div class="bg-row-detail-line bg-row-muted">{$t('background.outputTruncated')}</div>{/if}
        <pre class="bg-output">{output.output || $t('background.noOutput')}</pre>
      {:else}
        <span class="loading loading-dots loading-xs"></span>
      {/if}
      {#if otherActions.length > 0}
        <div class="bg-row-actions">
          {#each otherActions as action (action)}
            <button type="button" class="btn btn-xs {action === 'approve' ? 'btn-primary' : 'btn-ghost'}" disabled={busy !== null} onclick={() => act(action)}>
              {#if busy === action}<span class="loading loading-spinner loading-xs"></span>{/if}
              {$t(`background.action.${action}`)}
            </button>
          {/each}
        </div>
      {/if}
      {#if error}<div class="bg-row-detail-line text-error">{error}</div>{/if}
    </div>
  {/if}
</div>
