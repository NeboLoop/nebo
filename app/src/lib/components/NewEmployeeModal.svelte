<!--
  NewEmployeeModal — hire an additional employee. Same doctrine as the
  christening: hiring starts with a name. Unlike christening this is
  dismissible — the workforce already exists.

  Below the name, "Hire from another app" lists every OpenClaw or Hermes
  install of the owner's joined through Nebo Link, each with the agents it
  offers: picking one makes it an employee here, with its own name and brain.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { createAgent, listLinkedAgents } from '$lib/api/nebo';
  import type { LinkedBotEntry } from '$lib/api/neboComponents';

  let { onclose, oncreated }: {
    onclose: () => void;
    oncreated: (agentId: string, name: string, threadId: string | null) => void;
  } = $props();

  let name = $state('');
  let busy = $state(false);
  let errorMsg = $state('');
  let linkedBots = $state<LinkedBotEntry[]>([]);

  const valid = $derived(name.trim().length > 0 && name.trim().length <= 40);

  onMount(async () => {
    try {
      const resp = await listLinkedAgents();
      linkedBots = resp.bots ?? [];
    } catch {
      // Nothing to hire from is the same as no linked bots: the section
      // stays hidden.
      linkedBots = [];
    }
  });

  async function create() {
    if (!valid || busy) return;
    busy = true;
    errorMsg = '';
    try {
      const resp = await createAgent({ blank: true, name: name.trim() });
      oncreated(resp.agent.id, resp.agent.name, resp.threadId);
    } catch (e: unknown) {
      errorMsg = e instanceof Error ? e.message : $t('newEmployee.failed');
      busy = false;
    }
  }

  async function hireLinked(bot: LinkedBotEntry, agentId: string) {
    if (busy) return;
    busy = true;
    errorMsg = '';
    try {
      const resp = await createAgent({ linked: { botId: bot.id, agentId } });
      oncreated(resp.agent.id, resp.agent.name, resp.threadId);
    } catch (e: unknown) {
      errorMsg =
        e instanceof Error ? e.message : $t('newEmployee.linkedFailed', { values: { bot: bot.name } });
      busy = false;
    }
  }

  // The app a linked bot runs, as the owner knows it.
  const APP_NAMES: Record<string, string> = { openclaw: 'OpenClaw', hermes: 'Hermes' };
  function appName(runtime: string): string {
    return APP_NAMES[runtime] ?? runtime.charAt(0).toUpperCase() + runtime.slice(1);
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      create();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      onclose();
    }
  }
</script>

<div class="fixed inset-0 z-[80] flex items-center justify-center p-4" role="dialog" aria-modal="true">
  <div class="absolute inset-0 bg-black/50 backdrop-blur-sm" role="presentation" onclick={() => !busy && onclose()}></div>
  <div class="relative w-full max-w-sm rounded-2xl bg-base-100 border border-base-300 shadow-2xl p-6 flex flex-col items-center text-center">
    <h1 class="text-base font-semibold">{$t('newEmployee.title')}</h1>
    <p class="text-sm text-base-content/70 mt-2 leading-relaxed">{$t('newEmployee.lede')}</p>

    <div class="w-full mt-5 flex flex-col gap-2">
      <input
        type="text"
        class="input input-bordered w-full text-center text-lg font-medium"
        placeholder={$t('newEmployee.placeholder')}
        maxlength="40"
        bind:value={name}
        {onkeydown}
        autofocus
      />
      {#if errorMsg}
        <p class="text-xs text-error">{errorMsg}</p>
      {/if}
    </div>

    <div class="w-full mt-4 flex gap-2">
      <button type="button" class="btn btn-ghost rounded-field flex-1" onclick={onclose} disabled={busy}>
        {$t('common.cancel')}
      </button>
      <button type="button" class="btn btn-primary rounded-field flex-1" disabled={!valid || busy} onclick={create}>
        {#if busy}
          <span class="loading loading-spinner loading-xs"></span>
        {:else}
          {$t('newEmployee.create')}
        {/if}
      </button>
    </div>

    {#if linkedBots.length > 0}
      <div class="w-full mt-5 pt-4 border-t border-base-300 text-left">
        <h2 class="text-sm font-semibold">{$t('newEmployee.hireFromApps')}</h2>
        <p class="text-xs text-base-content/60 mt-1 leading-relaxed">{$t('newEmployee.linkedLede')}</p>
        {#each linkedBots as bot (bot.id)}
          <div class="mt-3">
            <h3 class="text-xs font-medium text-base-content/70">{bot.name} · {appName(bot.runtime)}{bot.online ? '' : ` · ${$t('newEmployee.offline')}`}</h3>
            <ul class="mt-1 flex flex-col gap-1">
              {#each bot.agents as agent (agent.id)}
                <li>
                  <button
                    type="button"
                    class="btn btn-ghost btn-sm rounded-field w-full h-auto min-h-0 py-2 flex-col items-start gap-0.5 font-normal text-left"
                    disabled={busy}
                    onclick={() => hireLinked(bot, agent.id)}
                  >
                    <span class="font-medium whitespace-normal break-words">{agent.name}</span>
                    {#if agent.description}
                      <span class="text-xs text-base-content/60 truncate w-full">{agent.description}</span>
                    {/if}
                  </button>
                </li>
              {/each}
            </ul>
          </div>
        {/each}
      </div>
    {/if}

    <p class="text-xs text-base-content/50 mt-4">{$t('newEmployee.marketplaceHint')}</p>
  </div>
</div>
