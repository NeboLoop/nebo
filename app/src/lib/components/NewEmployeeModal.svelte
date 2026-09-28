<!--
  NewEmployeeModal — hire an additional employee. Same doctrine as the
  christening: hiring starts with a name. Unlike christening this is
  dismissible — the workforce already exists.

  An optional job description is worked out into what the employee will be
  able to do (the one needs step, `POST /agents/needs`), shown above Create
  in plain words. Each item is removable; Create grants what is left.

  Below the name, "Hire from another app" lists the owner's computers
  (GET /agents/linked), this one first, each with what is installed on it,
  one entry per app, named for what it is: Claude Code, Codex, Gemini CLI,
  OpenClaw, Hermes. Picking one makes it an employee here, started as
  itself on its own computer. The list is looked for again while it is
  open, so an app installed meanwhile shows up.
  A coding agent (Claude Code, Codex, ...) is hired with the permission mode
  the owner picks here, which it runs in on its computer: the company's
  unless he picks another. Settings changes it later.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { X } from 'lucide-svelte';
  import { createAgent, getCompanyPermissions, listLinkedAgents, workOutAgentNeeds } from '$lib/api/nebo';
  import type { LinkedAgentEntry, LinkedComputerEntry } from '$lib/api/neboComponents';
  import { RELIST_MS, anyCoding as codingOnList, linkedHire, type LinkedMode } from '$lib/utils/linkedHire';

  let { onclose, oncreated }: {
    onclose: () => void;
    oncreated: (agentId: string, name: string, threadId: string | null) => void;
  } = $props();

  let name = $state('');
  let job = $state('');
  let busy = $state(false);
  let errorMsg = $state('');
  let computers = $state<LinkedComputerEntry[]>([]);

  const linkedModes: { id: LinkedMode; label: string; desc: string }[] = [
    { id: 'company', label: 'permissions.modeCompany', desc: 'permissions.modeCompanyDesc' },
    { id: 'automatic', label: 'permissions.modeAutomatic', desc: 'newEmployee.linkedModeAutomaticDesc' },
    { id: 'ask', label: 'permissions.modeAsk', desc: 'newEmployee.linkedModeAskDesc' },
    { id: 'plan', label: 'permissions.modePlan', desc: 'newEmployee.linkedModePlanDesc' },
    { id: 'full_access', label: 'permissions.modeFullAccess', desc: 'newEmployee.linkedModeFullAccessDesc' }
  ];
  let linkedMode = $state<LinkedMode>('company');
  // The company's mode, named on the first choice.
  let companyMode = $state('');
  const companyModeLabel = $derived(linkedModes.find((m) => m.id !== 'company' && m.id === companyMode)?.label);
  const anyCoding = $derived(codingOnList(computers));

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

  // What is installed on each computer, looked for again while the list is
  // open. A listing that fails keeps what is shown.
  async function relist() {
    try {
      const resp = await listLinkedAgents();
      computers = resp.computers ?? [];
    } catch {
      // Nothing listed yet is the same as no computers: the section stays
      // hidden.
    }
  }

  onMount(() => {
    getCompanyPermissions()
      .then((page) => (companyMode = page.companyMode))
      .catch(() => {
        // The first choice reads "Same as company defaults" alone.
      });
    relist();
    const every = setInterval(relist, RELIST_MS);
    return () => clearInterval(every);
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

  async function hireLinked(computer: LinkedComputerEntry, agent: LinkedAgentEntry) {
    if (busy || agent.hired) return;
    busy = true;
    errorMsg = '';
    try {
      const resp = await createAgent({ linked: linkedHire(agent, linkedMode) });
      oncreated(resp.agent.id, resp.agent.name, resp.threadId);
    } catch (e: unknown) {
      errorMsg =
        e instanceof Error ? e.message : $t('newEmployee.linkedFailed', { values: { bot: computer.name } });
      busy = false;
    }
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

    {#if computers.length > 0}
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
                    {#if m.id !== 'company'}
                      <span class="block text-xs text-base-content/60">{$t(m.desc)}</span>
                    {:else if companyModeLabel}
                      <span class="block text-xs text-base-content/60">{$t(m.desc, { values: { mode: $t(companyModeLabel) } })}</span>
                    {/if}
                  </span>
                </label>
              {/each}
            </div>
          </fieldset>
        {/if}
        {#each computers as computer (computer.id)}
          {@const name = computer.local ? $t('newEmployee.thisComputer') : computer.name}
          <div class="mt-3">
            <h3 class="text-xs font-medium text-base-content/70">{name}{computer.online ? '' : ` · ${$t('newEmployee.offline')}`}</h3>
            {#if computer.agents.length === 0}
              {#if computer.online}
                <p class="text-xs text-base-content/60 mt-1">{$t('newEmployee.linkedEmpty', { values: { computer: name } })}</p>
              {/if}
            {:else}
              <ul class="mt-1 flex flex-col gap-1">
                {#each computer.agents as agent (`${agent.botId}/${agent.id}`)}
                  <li>
                    <button
                      type="button"
                      class="btn btn-ghost btn-sm rounded-field w-full h-auto min-h-0 py-2 flex-col items-start gap-0.5 font-normal text-left"
                      disabled={busy || agent.hired}
                      onclick={() => hireLinked(computer, agent)}
                    >
                      <span class="font-medium whitespace-normal break-words">{agent.name}</span>
                      <span class="text-xs text-base-content/60 truncate w-full">{agent.hired ? $t('newEmployee.onYourTeam') : agent.description}</span>
                    </button>
                  </li>
                {/each}
              </ul>
            {/if}
          </div>
        {/each}
      </div>
    {/if}

    <p class="text-xs text-base-content/50 mt-4">{$t('newEmployee.marketplaceHint')}</p>
  </div>
</div>
