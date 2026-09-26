<!--
  NewEmployeeModal — hire an additional employee. Same doctrine as the
  christening: hiring starts with a name. Unlike christening this is
  dismissible — the workforce already exists.

  An optional job description is worked out into what the employee will be
  able to do (the one needs step, `POST /agents/needs`), shown above Create
  in plain words. Each item is removable; Create grants what is left.

  Below the name, "Hire from another app" lists every OpenClaw, Hermes or
  ACP agent (Claude Code, Codex, Gemini CLI, OpenCode) install of the
  owner's joined through Nebo Link, each with the agents it
  offers: picking one makes it an employee here, with its own name and brain.
  A coding agent (Claude Code, Codex, ...) is hired with the permission mode
  the owner picks here, which it runs in on its computer; Settings changes
  it later.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { X } from 'lucide-svelte';
  import { createAgent, listLinkedAgents, workOutAgentNeeds } from '$lib/api/nebo';
  import type { LinkedBotEntry } from '$lib/api/neboComponents';

  let { onclose, oncreated }: {
    onclose: () => void;
    oncreated: (agentId: string, name: string, threadId: string | null) => void;
  } = $props();

  let name = $state('');
  let job = $state('');
  let busy = $state(false);
  let errorMsg = $state('');
  let linkedBots = $state<LinkedBotEntry[]>([]);

  // The runtimes that are coding agents: they run their own tools on their
  // computer, under the permission mode the employee is hired with.
  const CODING = new Set(['claude-code', 'codex', 'gemini', 'opencode', 'acp']);
  type LinkedMode = 'automatic' | 'ask' | 'plan' | 'full_access';
  const linkedModes: { id: LinkedMode; label: string; desc: string }[] = [
    { id: 'automatic', label: 'permissions.modeAutomatic', desc: 'newEmployee.linkedModeAutomaticDesc' },
    { id: 'ask', label: 'permissions.modeAsk', desc: 'newEmployee.linkedModeAskDesc' },
    { id: 'plan', label: 'permissions.modePlan', desc: 'newEmployee.linkedModePlanDesc' },
    { id: 'full_access', label: 'permissions.modeFullAccess', desc: 'newEmployee.linkedModeFullAccessDesc' }
  ];
  let linkedMode = $state<LinkedMode>('automatic');
  const anyCoding = $derived(linkedBots.some((b) => CODING.has(b.runtime)));

  // The drafted job: its plain-words items and the draft Create grants.
  let draftId = $state<string | null>(null);
  let items = $state<string[]>([]);
  let removed = $state<string[]>([]);
  let working = $state(false);
  let asked = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;

  const valid = $derived(name.trim().length > 0 && name.trim().length <= 40);
  const kept = $derived(items.filter((i) => !removed.includes(i)));

  async function workOut() {
    const description = job.trim();
    const mine = ++asked;
    if (!description) {
      draftId = null;
      items = [];
      removed = [];
      working = false;
      return;
    }
    working = true;
    try {
      const res = await workOutAgentNeeds({ name: name.trim(), description });
      if (mine !== asked) return;
      draftId = res.draftId;
      items = res.items;
      removed = removed.filter((r) => res.items.includes(r));
    } catch {
      if (mine === asked) {
        draftId = null;
        items = [];
      }
    }
    if (mine === asked) working = false;
  }

  // Worked out once the owner pauses, not on every keystroke.
  function jobChanged() {
    clearTimeout(timer);
    timer = setTimeout(workOut, 700);
  }

  function remove(item: string) {
    removed = [...removed, item];
  }

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
    if (!valid || busy || working) return;
    busy = true;
    errorMsg = '';
    try {
      const resp = await createAgent({
        blank: true,
        name: name.trim(),
        ...(draftId ? { draftId, removed } : {}),
      });
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
      const resp = await createAgent({
        linked: { botId: bot.id, agentId, ...(CODING.has(bot.runtime) ? { permissionMode: linkedMode } : {}) }
      });
      oncreated(resp.agent.id, resp.agent.name, resp.threadId);
    } catch (e: unknown) {
      errorMsg =
        e instanceof Error ? e.message : $t('newEmployee.linkedFailed', { values: { bot: bot.name } });
      busy = false;
    }
  }

  // The app a linked bot runs, as the owner knows it.
  const APP_NAMES: Record<string, string> = {
    openclaw: 'OpenClaw',
    hermes: 'Hermes',
    'claude-code': 'Claude Code',
    codex: 'Codex',
    gemini: 'Gemini CLI',
    opencode: 'OpenCode',
    acp: 'ACP agent'
  };
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
      <textarea
        class="textarea textarea-bordered w-full text-sm"
        rows="2"
        placeholder={$t('newEmployee.jobPlaceholder')}
        aria-label={$t('newEmployee.jobLabel')}
        bind:value={job}
        oninput={jobChanged}
      ></textarea>
      {#if working}
        <p class="consent-chip-hint">{$t('newEmployee.working')}</p>
      {:else if kept.length}
        <div class="w-full flex flex-col gap-1.5 text-left">
          <p class="consent-line">{$t('newEmployee.willDo', { values: { name: name.trim() || $t('newEmployee.thisEmployee') } })}</p>
          <div class="consent-items">
            {#each kept as item (item)}
              <span class="consent-item">
                {item}
                <button type="button" class="consent-item-remove" aria-label={$t('newEmployee.removeItem', { values: { item } })} onclick={() => remove(item)}>
                  <X class="w-3 h-3" />
                </button>
              </span>
            {/each}
          </div>
        </div>
      {/if}
      {#if errorMsg}
        <p class="text-xs text-error">{errorMsg}</p>
      {/if}
    </div>

    <div class="w-full mt-4 flex gap-2">
      <button type="button" class="btn btn-ghost rounded-field flex-1" onclick={onclose} disabled={busy}>
        {$t('common.cancel')}
      </button>
      <button type="button" class="btn btn-primary rounded-field flex-1" disabled={!valid || busy || working} onclick={create}>
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
        {#if anyCoding}
          <fieldset class="mt-3">
            <legend class="text-xs font-medium text-base-content/70">{$t('newEmployee.linkedModeTitle')}</legend>
            <div class="mt-1 flex flex-col gap-1" role="radiogroup" aria-label={$t('newEmployee.linkedModeTitle')}>
              {#each linkedModes as m (m.id)}
                <label class="flex items-start gap-2 rounded-field px-2 py-1.5 cursor-pointer hover:bg-base-200/50">
                  <input type="radio" class="radio radio-xs radio-primary mt-0.5" name="linked-mode" value={m.id} bind:group={linkedMode} disabled={busy} />
                  <span class="min-w-0">
                    <span class="block text-sm">{$t(m.label)}</span>
                    <span class="block text-xs text-base-content/60">{$t(m.desc)}</span>
                  </span>
                </label>
              {/each}
            </div>
          </fieldset>
        {/if}
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
