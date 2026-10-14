<!--
  The three intro screens before the guided first-run chat: meet your
  assistant, it works while you sleep, hand off to a team. Full screen over
  the shell while the server's step is `intro`. Get started and Skip both
  leave through `startFirstRun` (the server moves on and the assistant starts
  talking in its chat); `onstarted` opens that thread.

  Drawn in code: the characters are AgentAvatar's, the scenes are CSS
  (app.css, `.intro-*`). All three screens are in the page at once and
  cross-fade; a screen's animations start when it becomes the active one,
  and `prefers-reduced-motion` stops them.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import { onMount, tick } from 'svelte';
  import AgentAvatar from '$lib/components/AgentAvatar.svelte';
  import { ASSISTANT_SHAPE } from '$lib/characters';
  import { startFirstRun } from '$lib/stores/onboarding';
  import Moon from 'lucide-svelte/icons/moon';
  import Check from 'lucide-svelte/icons/check';

  let {
    assistantName,
    onstarted
  }: {
    /** The assistant's name on the team screen. */
    assistantName: string;
    /** Called once the intro is over, with the assistant's thread (or null). */
    onstarted: (threadId: string | null) => void;
  } = $props();

  const LAST = 2;
  let screen = $state(0);
  let starting = $state(false);
  const buttons: HTMLButtonElement[] = [];

  onMount(() => buttons[0]?.focus());

  async function next() {
    screen += 1;
    await tick();
    buttons[screen]?.focus();
  }

  async function begin() {
    if (starting) return;
    starting = true;
    onstarted(await startFirstRun());
  }

  const team = [
    { key: 'storeManager', color: 'violet', shape: 'ridge' },
    { key: 'content', color: 'peach', shape: 'volcano' },
    { key: 'invoices', color: 'green', shape: 'mesa' },
    { key: 'standup', color: 'amber', shape: 'butte' }
  ] as const;
  const tasks = ['taskReorder', 'taskVideo', 'taskInvoices'] as const;
  const screens = [
    { title: 'firstRun.meetTitle', body: 'firstRun.meetBody' },
    { title: 'firstRun.awayTitle', body: 'firstRun.awayBody' },
    { title: 'firstRun.teamTitle', body: 'firstRun.teamBody' }
  ] as const;
</script>

<div class="intro-screens" role="dialog" aria-modal="true" aria-labelledby="intro-title-{screen}">
  <button type="button" class="intro-skip" onclick={begin} disabled={starting}>{$t('common.skip')}</button>
  <div class="intro-deck">
    {#each screens as s, i (i)}
      <section class="intro-screen" class:is-active={screen === i} inert={screen !== i}>
        <div class="intro-art" aria-hidden="true">
          {#if i === 0}
            <div class="intro-hero">
              <AgentAvatar name={assistantName} shape={ASSISTANT_SHAPE} size="xl" class="intro-hero-character" />
            </div>
          {:else if i === 1}
            <div class="intro-clock"><Moon class="w-3.5 h-3.5" />{$t('firstRun.awayClock')}</div>
            <div class="intro-window">
              <div class="intro-window-bar"><i></i><i></i><i></i></div>
              {#each [0, 1, 2, 3, 4] as n (n)}<div class="intro-window-line"></div>{/each}
              <div class="intro-worker">
                <AgentAvatar name={assistantName} shape={ASSISTANT_SHAPE} size="xl" class="intro-worker-character" />
              </div>
            </div>
          {:else}
            <div class="intro-team">
              <div class="intro-member">
                <AgentAvatar name={assistantName} shape={ASSISTANT_SHAPE} size="xl" class="intro-member-lead" />
                <span>{$t('firstRun.assistantRole', { values: { name: assistantName } })}</span>
              </div>
              {#each team as m (m.key)}
                <div class="intro-member">
                  <AgentAvatar name={$t(`firstRun.${m.key}`)} color={m.color} shape={m.shape} size="xl" class="intro-member-character" />
                  <span>{$t(`firstRun.${m.key}`)}</span>
                </div>
              {/each}
              <div class="intro-tasks">
                {#each tasks as task (task)}
                  <div class="intro-task"><Check class="intro-task-check" />{$t(`firstRun.${task}`)}</div>
                {/each}
              </div>
            </div>
          {/if}
        </div>
        <h2 id="intro-title-{i}" class="intro-title">{$t(s.title)}</h2>
        <p class="intro-body">{$t(s.body)}</p>
        <div class="intro-dots" aria-hidden="true">
          {#each screens as _, k (k)}<i class:is-on={k === i}></i>{/each}
        </div>
        {#if i < LAST}
          <button type="button" class="intro-next" bind:this={buttons[i]} onclick={next}>{$t('common.next')}</button>
        {:else}
          <button type="button" class="intro-next" bind:this={buttons[i]} onclick={begin} disabled={starting}>{$t('firstRun.getStarted')}</button>
        {/if}
      </section>
    {/each}
  </div>
</div>
