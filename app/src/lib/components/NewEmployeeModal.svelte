<!--
  NewEmployeeModal — hire an additional employee. Same doctrine as the
  christening: hiring starts with a name. Unlike christening this is
  dismissible — the workforce already exists.

  An optional job description is worked out into what the employee will be
  able to do (the one needs step, `POST /agents/needs`), shown above Create
  in plain words. Each item is removable; Create grants what is left.

  Below the name, "Hire from another app" lists the apps on the owner's
  computers (GET /agents/linked), one row per app in one order: Claude Code,
  Codex, Gemini CLI, then Hermes, OpenClaw and any other. Nebo already knows
  them, so the list answers at once; what it finds later arrives as ONE
  `linked_apps_changed` event, which never moves a row: a row keeps its
  place and an app found since is added at the end.

  A coding app (Claude Code, Codex, ...) hires a new one every time: the
  owner names it (the app's name for the first, one he types after that,
  never a name already on the team; the ones he has are there to open
  instead), picks the computer when it is on more than one, then the
  permission mode it runs in there, the company's unless he picks another.
  Another app (Hermes, OpenClaw) is hired as its agent: at once when it runs
  one, from a list of them when it runs several, the ones on the team
  checked. The hired employee opens.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { Bot, Check, ChevronLeft, Terminal, X } from 'lucide-svelte';
  import { createAgent, getCompanyPermissions, listLinkedAgents, workOutAgentNeeds } from '$lib/api/nebo';
  import type { ListLinkedAgentsResponse } from '$lib/api/neboComponents';
  import { onWsEvent } from '$lib/websocket/subscribe';
  import {
    appLine,
    linkedHire,
    mergeLinkedApps,
    nameTaken,
    suggestedName,
    type LinkedApp,
    type LinkedChoice,
    type LinkedMode
  } from '$lib/utils/linkedHire';

  let { onclose, oncreated, onopen, names = [] }: {
    onclose: () => void;
    oncreated: (agentId: string, name: string, threadId: string | null) => void;
    // Opens an employee already on the team.
    onopen: (agentId: string) => void;
    // Every employee's name: a new one never takes one of them.
    names?: string[];
  } = $props();

  let name = $state('');
  let job = $state('');
  let busy = $state(false);
  let errorMsg = $state('');
  // "Hire from another app": one row per app, and whether the first listing
  // has answered.
  let apps = $state<LinkedApp[]>([]);
  let listed = $state(false);

  // The hire of one app, a step at a time: its name (a coding app), the
  // computer (a coding app on several), the agent (an app running several),
  // then what it may do (a coding app).
  let picked = $state<LinkedApp | null>(null);
  let step = $state<'name' | 'computer' | 'agent' | 'mode'>('name');
  let hireName = $state('');
  let choice = $state<LinkedChoice | null>(null);
  const taken = $derived(picked?.coding ? nameTaken(hireName, names) : null);

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

  // What Nebo knows of the owner's computers, at once; what it finds later
  // comes as an event. Neither moves a row that is shown.
  onWsEvent<ListLinkedAgentsResponse>('linked_apps_changed', (data) => {
    apps = mergeLinkedApps(apps, data.computers ?? []);
    listed = true;
  });

  onMount(() => {
    getCompanyPermissions()
      .then((page) => (companyMode = page.companyMode))
      .catch(() => {
        // The first choice reads "Same as company defaults" alone.
      });
    listLinkedAgents()
      .then((resp) => (apps = mergeLinkedApps(apps, resp.computers ?? [])))
      .catch(() => {
        // Nothing listed is the same as no apps: the section stays hidden.
      })
      .finally(() => (listed = true));
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

  // A row tapped: a coding app is named first; an app running one agent
  // hires it at once (or opens it, when it is on the team already); one
  // running several lists them.
  function pick(app: LinkedApp) {
    if (busy) return;
    errorMsg = '';
    if (app.coding) {
      picked = app;
      hireName = suggestedName(app);
      choice = app.choices.length === 1 ? app.choices[0] : null;
      step = 'name';
      return;
    }
    if (app.choices.length === 1) {
      pickAgent(app.choices[0]);
      return;
    }
    picked = app;
    step = 'agent';
  }

  function named() {
    if (!hireName.trim() || taken) return;
    step = choice ? 'mode' : 'computer';
  }

  function pickComputer(c: LinkedChoice) {
    choice = c;
    step = 'mode';
  }

  function pickAgent(c: LinkedChoice) {
    if (c.agent.hired) {
      if (c.agent.employeeId) onopen(c.agent.employeeId);
      return;
    }
    hireLinked(c, 'company');
  }

  function back() {
    picked = null;
    errorMsg = '';
  }

  async function hireLinked(c: LinkedChoice, mode: LinkedMode, as?: string) {
    if (busy) return;
    busy = true;
    errorMsg = '';
    try {
      const resp = await createAgent(linkedHire(c.agent, mode, as));
      oncreated(resp.agent.id, resp.agent.name, resp.threadId);
    } catch (e: unknown) {
      errorMsg =
        e instanceof Error ? e.message : $t('newEmployee.linkedFailed', { values: { bot: c.computer.name } });
      busy = false;
    }
  }

  function computerName(c: LinkedChoice): string {
    const where = c.computer.local ? $t('newEmployee.thisComputer') : c.computer.name;
    return c.computer.online ? where : `${where} · ${$t('newEmployee.offline')}`;
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
    {#if picked}
      <div class="w-full text-left">
        <button type="button" class="btn btn-ghost btn-xs rounded-field -ml-2" onclick={back} disabled={busy}>
          <ChevronLeft class="w-3.5 h-3.5" />
          {$t('common.back')}
        </button>
        {#if step === 'name'}
          <h1 class="text-base font-semibold mt-2">{$t('newEmployee.nameYour', { values: { app: picked.app } })}</h1>
          {#if picked.team.length > 0}
            <p class="text-xs text-base-content/60 mt-3">{$t('newEmployee.youAlreadyHave')}</p>
            <div class="mt-1 flex flex-wrap gap-1">
              {#each picked.team as member (member.employeeId)}
                <button type="button" class="btn btn-ghost btn-xs rounded-field border border-base-300" onclick={() => onopen(member.employeeId)}>
                  {member.name}
                </button>
              {/each}
            </div>
          {/if}
          <input
            type="text"
            class="input input-bordered w-full mt-3"
            placeholder={$t('newEmployee.placeholder')}
            aria-label={$t('newEmployee.nameYour', { values: { app: picked.app } })}
            maxlength="40"
            bind:value={hireName}
            onkeydown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault();
                named();
              }
            }}
            autofocus
          />
          {#if taken}
            <p class="text-xs text-error mt-1">{$t('newEmployee.nameTaken', { values: { name: taken } })}</p>
          {/if}
          <button type="button" class="btn btn-primary rounded-field w-full mt-4" disabled={!hireName.trim() || !!taken} onclick={named}>
            {$t('common.continue')}
          </button>
        {:else if step === 'computer'}
          <h1 class="text-base font-semibold mt-2">{$t('newEmployee.linkedWhere')}</h1>
          <ul class="mt-3 flex flex-col gap-1">
            {#each picked.choices as c (`${c.agent.botId}/${c.agent.id}`)}
              <li>
                <button type="button" class="btn btn-ghost btn-sm rounded-field w-full justify-start font-normal" onclick={() => pickComputer(c)}>
                  {computerName(c)}
                </button>
              </li>
            {/each}
          </ul>
        {:else if step === 'agent'}
          <h1 class="text-base font-semibold mt-2">{$t('newEmployee.linkedPickAgent', { values: { app: picked.app } })}</h1>
          <ul class="mt-3 flex flex-col gap-1">
            {#each picked.choices as c (`${c.agent.botId}/${c.agent.id}`)}
              <li>
                <button
                  type="button"
                  class="btn btn-ghost btn-sm rounded-field w-full h-auto min-h-0 py-2 justify-start gap-3 font-normal text-left"
                  disabled={busy}
                  onclick={() => pickAgent(c)}
                >
                  <span class="min-w-0 flex-1 flex flex-col items-start gap-0.5">
                    <span class="font-medium whitespace-normal break-words">{c.agent.name}</span>
                    <span class="text-xs text-base-content/60 truncate w-full">{c.agent.hired ? $t('newEmployee.onYourTeam') : c.agent.description}</span>
                  </span>
                  {#if c.agent.hired}
                    <Check class="w-4 h-4 shrink-0 text-success" aria-label={$t('newEmployee.onYourTeam')} />
                  {/if}
                </button>
              </li>
            {/each}
          </ul>
        {:else if choice}
          {@const hireChoice = choice}
          <h1 class="text-base font-semibold mt-2">{$t('newEmployee.linkedModeTitle')}</h1>
          <div class="mt-2 flex flex-col gap-1" role="radiogroup" aria-label={$t('newEmployee.linkedModeTitle')}>
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
          <button type="button" class="btn btn-primary rounded-field w-full mt-4" disabled={busy} onclick={() => hireLinked(hireChoice, linkedMode, hireName)}>
            {#if busy}
              <span class="loading loading-spinner loading-xs"></span>
            {:else}
              {$t('common.hire')}
            {/if}
          </button>
        {/if}
        {#if errorMsg}
          <p class="text-xs text-error mt-2">{errorMsg}</p>
        {/if}
      </div>
    {:else}
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

      {#if !listed || apps.length > 0}
        <div class="w-full mt-5 pt-4 border-t border-base-300 text-left">
          <h2 class="text-sm font-semibold">{$t('newEmployee.hireFromApps')}</h2>
          <p class="text-xs text-base-content/60 mt-1 leading-relaxed">{$t('newEmployee.linkedLede')}</p>
          {#if !listed}
            <p class="text-xs text-base-content/60 mt-3">{$t('newEmployee.linkedLooking')}</p>
          {:else}
            <ul class="mt-2 flex flex-col gap-1">
              {#each apps as app (app.runtime)}
                {@const line = appLine(app)}
                <li>
                  <button
                    type="button"
                    class="btn btn-ghost btn-sm rounded-field w-full h-auto min-h-0 py-2 justify-start gap-3 font-normal text-left"
                    disabled={busy}
                    onclick={() => pick(app)}
                  >
                    {#if app.coding}
                      <Terminal class="w-4 h-4 shrink-0 text-base-content/70" />
                    {:else}
                      <Bot class="w-4 h-4 shrink-0 text-base-content/70" />
                    {/if}
                    <span class="min-w-0 flex-1 flex flex-col items-start gap-0.5">
                      <span class="font-medium">{app.app}</span>
                      <span class="text-xs text-base-content/60">{$t(line.key, { values: line.values })}</span>
                    </span>
                    {#if line.key === 'newEmployee.onYourTeam'}
                      <Check class="w-4 h-4 shrink-0 text-success" />
                    {/if}
                  </button>
                </li>
              {/each}
            </ul>
          {/if}
        </div>
      {/if}

      <p class="text-xs text-base-content/50 mt-4">{$t('newEmployee.marketplaceHint')}</p>
    {/if}
  </div>
</div>
