<!--
  PermissionsSection — what an employee may do, in plain words (Settings →
  employee → Permissions). With no agentId it is the company defaults page,
  with the same controls (Settings → Permissions).

  Every line is a sentence the server renders; the page never shows a rule.
  Changes save as they are made: a mode, an item removed, a capability or a
  folder added, a money amount typed (debounced).
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import { t } from 'svelte-i18n';
  import Check from 'lucide-svelte/icons/check';
  import X from 'lucide-svelte/icons/x';
  import Lock from 'lucide-svelte/icons/lock';
  import FolderPlus from 'lucide-svelte/icons/folder-plus';
  import AlertTriangle from 'lucide-svelte/icons/alert-triangle';
  import * as api from '$lib/api/nebo';
  import type { Mode, PermissionItem, PermissionsPage, MoneyAmounts } from '$lib/api/nebo';
  import Spinner from '$lib/components/ui/Spinner.svelte';

  let { agentId, name = '' }: { agentId?: string; name?: string } = $props();

  let page = $state<PermissionsPage | null>(null);
  let loading = $state(true);
  let error = $state('');
  let saved = $state(false);
  let savedTimer: ReturnType<typeof setTimeout> | null = null;
  let confirmFullAccess = $state(false);
  let addChoice = $state('');

  /** Money amounts as typed, in dollars, keyed by item id. */
  let moneyInputs = $state<Record<string, { perAction: string; perDay: string; perDayCount: string }>>({});
  const moneyTimers: Record<string, ReturnType<typeof setTimeout>> = {};

  const modes: { id: Mode; label: string; desc: string }[] = [
    { id: 'automatic', label: 'permissions.modeAutomatic', desc: 'permissions.modeAutomaticDesc' },
    { id: 'ask', label: 'permissions.modeAsk', desc: 'permissions.modeAskDesc' },
    { id: 'plan', label: 'permissions.modePlan', desc: 'permissions.modePlanDesc' },
    { id: 'full_access', label: 'permissions.modeFullAccess', desc: 'permissions.modeFullAccessDesc' }
  ];

  function modeLabel(m: Mode): string {
    return $t(modes.find((x) => x.id === m)?.label ?? 'permissions.modeAutomatic');
  }

  function dollars(cents?: number): string {
    return cents === undefined || cents === null ? '' : (cents / 100).toFixed(cents % 100 === 0 ? 0 : 2);
  }

  function show(p: PermissionsPage) {
    page = p;
    const inputs: typeof moneyInputs = {};
    for (const item of p.money) {
      inputs[item.id] = {
        perAction: dollars(item.money?.perActionCents),
        perDay: dollars(item.money?.perDayCents),
        perDayCount: item.money?.perDayCount?.toString() ?? ''
      };
    }
    moneyInputs = inputs;
  }

  async function load(id: string | undefined) {
    loading = true;
    error = '';
    try {
      show(id ? await api.getAgentPermissions(id) : await api.getCompanyPermissions());
    } catch {
      error = $t('permissions.loadError');
    } finally {
      loading = false;
    }
  }

  // Depends on agentId only: the loader writes the state it would
  // otherwise read.
  $effect(() => {
    const id = agentId;
    untrack(() => void load(id));
  });

  function flashSaved() {
    saved = true;
    if (savedTimer) clearTimeout(savedTimer);
    savedTimer = setTimeout(() => (saved = false), 2000);
  }

  async function change(body: Record<string, unknown>) {
    error = '';
    try {
      show(agentId ? await api.updateAgentPermissions(agentId, body) : await api.updateCompanyPermissions(body));
      flashSaved();
    } catch (e) {
      error = e instanceof Error && e.message ? e.message : $t('permissions.saveError');
    }
  }

  function pickMode(id: Mode | 'company') {
    if (id === 'full_access' && page?.mode !== 'full_access') {
      confirmFullAccess = true;
      return;
    }
    void change({ mode: id });
  }

  function turnOnFullAccess() {
    confirmFullAccess = false;
    void change({ mode: 'full_access' });
  }

  async function removeItem(item: PermissionItem) {
    error = '';
    try {
      if (agentId) await api.removeAgentPermission(agentId, item.id);
      else await api.removeCompanyPermission(item.id);
      await load(agentId);
      flashSaved();
    } catch (e) {
      error = e instanceof Error && e.message ? e.message : $t('permissions.saveError');
    }
  }

  function addCapability() {
    const id = addChoice;
    addChoice = '';
    if (id) void change({ addCapability: id });
  }

  async function addFolder() {
    try {
      const picked = await api.pickFolder();
      if (picked?.path) await change({ addFolder: picked.path });
    } catch (e) {
      error = e instanceof Error && e.message ? e.message : $t('permissions.saveError');
    }
  }

  function cents(value: string): number | undefined {
    const n = Number.parseFloat(value);
    return Number.isFinite(n) && n >= 0 ? Math.round(n * 100) : undefined;
  }

  function editMoney(id: string) {
    if (moneyTimers[id]) clearTimeout(moneyTimers[id]);
    moneyTimers[id] = setTimeout(() => {
      const input = moneyInputs[id];
      if (!input) return;
      const count = Number.parseInt(input.perDayCount, 10);
      const amounts: MoneyAmounts = {
        perActionCents: cents(input.perAction),
        perDayCents: cents(input.perDay),
        perDayCount: Number.isFinite(count) && count >= 0 ? count : undefined
      };
      void change({ money: { id, ...amounts } });
    }, 700);
  }

  const isEmployee = $derived(!!agentId);
</script>

{#snippet itemRow(item: PermissionItem, locked = false)}
  <li class="flex items-start gap-3 py-2.5">
    {#if locked}<Lock class="w-3.5 h-3.5 mt-0.5 shrink-0 text-base-content/50" />{/if}
    <span class="flex-1 min-w-0 text-sm">
      {item.sentence}
      {#if item.fromCompany}
        <span class="badge badge-ghost badge-xs ml-1.5 align-middle">{$t('permissions.fromCompany')}</span>
      {/if}
    </span>
    {#if item.removable}
      <button
        type="button"
        class="btn btn-ghost btn-xs btn-square shrink-0"
        aria-label={$t('permissions.remove')}
        title={$t('permissions.remove')}
        onclick={() => removeItem(item)}
      >
        <X class="w-3.5 h-3.5" />
      </button>
    {/if}
  </li>
{/snippet}

{#snippet group(title: string, hint: string, items: PermissionItem[], empty: string, locked = false)}
  <section class="mb-7">
    <h3 class="text-sm font-semibold">{title}</h3>
    {#if hint}<p class="text-xs text-base-content/70 mt-0.5">{hint}</p>{/if}
    {#if items.length === 0}
      <p class="text-xs text-base-content/60 mt-2">{empty}</p>
    {:else}
      <ul class="divide-y divide-base-content/10 mt-1">
        {#each items as item (item.id)}
          {@render itemRow(item, locked)}
        {/each}
      </ul>
    {/if}
  </section>
{/snippet}

<div class="flex items-center justify-end h-5 mb-1">
  {#if saved}
    <span class="text-xs text-success flex items-center gap-1"><Check class="w-3 h-3" /> {$t('common.saved')}</span>
  {/if}
</div>

{#if error}
  <div class="alert alert-error mb-4 py-2 text-xs">
    <AlertTriangle class="w-4 h-4 shrink-0" />
    <span>{error}</span>
  </div>
{/if}

{#if loading && !page}
  <div class="py-6 flex justify-center"><Spinner /></div>
{:else if page}
  <!-- Mode -->
  <section class="mb-7">
    <h3 class="text-sm font-semibold mb-2">{$t('permissions.modeTitle')}</h3>
    <div class="flex flex-col gap-2" role="radiogroup" aria-label={$t('permissions.modeTitle')}>
      {#if isEmployee}
        <label class="flex items-start gap-3 rounded-lg border px-3 py-2.5 cursor-pointer {page.modeFromCompany ? 'border-primary bg-primary/5' : 'border-base-300 hover:bg-base-200/50'}">
          <input type="radio" class="radio radio-sm radio-primary mt-0.5" checked={page.modeFromCompany} onchange={() => pickMode('company')} />
          <span>
            <span class="block text-sm font-medium">{$t('permissions.modeCompany')}</span>
            <span class="block text-xs text-base-content/70 mt-0.5">{$t('permissions.modeCompanyDesc', { values: { mode: modeLabel(page.companyMode) } })}</span>
          </span>
        </label>
      {/if}
      {#each modes as m (m.id)}
        {@const selected = page.mode === m.id && !page.modeFromCompany}
        <label class="flex items-start gap-3 rounded-lg border px-3 py-2.5 cursor-pointer {selected ? 'border-primary bg-primary/5' : 'border-base-300 hover:bg-base-200/50'}">
          <input type="radio" class="radio radio-sm radio-primary mt-0.5" checked={selected} onchange={() => pickMode(m.id)} />
          <span>
            <span class="block text-sm font-medium">{$t(m.label)}</span>
            <span class="block text-xs text-base-content/70 mt-0.5">{$t(m.desc)}</span>
          </span>
        </label>
      {/each}
    </div>
  </section>

  <!-- What the job includes -->
  <section class="mb-7">
    <h3 class="text-sm font-semibold">{$t('permissions.jobTitle')}</h3>
    <p class="text-xs text-base-content/70 mt-0.5">{$t('permissions.jobHint')}</p>
    {#if page.job.length === 0}
      <p class="text-xs text-base-content/60 mt-2">{$t('permissions.jobEmpty')}</p>
    {:else}
      <ul class="divide-y divide-base-content/10 mt-1">
        {#each page.job as item (item.id)}
          {@render itemRow(item)}
        {/each}
      </ul>
    {/if}
    {#if page.canAdd.length > 0}
      <select class="select select-sm select-bordered mt-2 w-full max-w-sm" bind:value={addChoice} onchange={addCapability} aria-label={$t('permissions.addToJob')}>
        <option value="" disabled selected>{$t('permissions.addToJob')}</option>
        {#each page.canAdd as option (option.id)}
          <option value={option.id}>{option.sentence}</option>
        {/each}
      </select>
    {/if}
  </section>

  <!-- Money limits -->
  <section class="mb-7">
    <h3 class="text-sm font-semibold">{$t('permissions.moneyTitle')}</h3>
    <p class="text-xs text-base-content/70 mt-0.5">{$t('permissions.moneyHint')}</p>
    {#if page.money.length === 0}
      <p class="text-xs text-base-content/60 mt-2">{$t('permissions.moneyEmpty')}</p>
    {:else}
      <ul class="divide-y divide-base-content/10 mt-1">
        {#each page.money as item (item.id)}
          <li class="py-2.5">
            <div class="flex items-start gap-3">
              <span class="flex-1 min-w-0 text-sm">
                {item.sentence}
                {#if item.fromCompany}<span class="badge badge-ghost badge-xs ml-1.5 align-middle">{$t('permissions.fromCompany')}</span>{/if}
              </span>
              {#if item.removable}
                <button type="button" class="btn btn-ghost btn-xs btn-square shrink-0" aria-label={$t('permissions.remove')} title={$t('permissions.remove')} onclick={() => removeItem(item)}>
                  <X class="w-3.5 h-3.5" />
                </button>
              {/if}
            </div>
            {#if item.removable && moneyInputs[item.id]}
              <div class="flex flex-wrap gap-3 mt-2">
                <label class="flex flex-col gap-1 text-xs text-base-content/70">
                  {$t('permissions.perAction')}
                  <input type="number" min="0" step="0.01" inputmode="decimal" class="input input-sm input-bordered w-28" bind:value={moneyInputs[item.id].perAction} oninput={() => editMoney(item.id)} />
                </label>
                <label class="flex flex-col gap-1 text-xs text-base-content/70">
                  {$t('permissions.perDay')}
                  <input type="number" min="0" step="0.01" inputmode="decimal" class="input input-sm input-bordered w-28" bind:value={moneyInputs[item.id].perDay} oninput={() => editMoney(item.id)} />
                </label>
                <label class="flex flex-col gap-1 text-xs text-base-content/70">
                  {$t('permissions.perDayCount')}
                  <input type="number" min="0" step="1" inputmode="numeric" class="input input-sm input-bordered w-24" bind:value={moneyInputs[item.id].perDayCount} oninput={() => editMoney(item.id)} />
                </label>
              </div>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  <!-- Folders -->
  <section class="mb-7">
    <h3 class="text-sm font-semibold">{$t('permissions.foldersTitle')}</h3>
    <p class="text-xs text-base-content/70 mt-0.5">{$t('permissions.foldersHint')}</p>
    {#if page.folders.length === 0}
      <p class="text-xs text-base-content/60 mt-2">{$t('permissions.foldersEmpty')}</p>
    {:else}
      <ul class="divide-y divide-base-content/10 mt-1">
        {#each page.folders as item (item.id)}
          {@render itemRow(item)}
        {/each}
      </ul>
    {/if}
    <button type="button" class="btn btn-sm btn-ghost mt-2 gap-1.5" onclick={addFolder}>
      <FolderPlus class="w-4 h-4" /> {$t('permissions.addFolder')}
    </button>
  </section>

  {@render group($t('permissions.alwaysTitle'), $t('permissions.alwaysHint'), page.alwaysAllowed, $t('permissions.alwaysEmpty'))}
  {#if page.asksFirst.length > 0}
    {@render group($t('permissions.asksTitle'), $t('permissions.asksHint'), page.asksFirst, '')}
  {/if}
  {#if page.never.length > 0}
    {@render group($t('permissions.neverTitle'), $t('permissions.neverHint'), page.never, '')}
  {/if}
  {#if page.fixed.length > 0}
    {@render group($t('permissions.fixedTitle'), $t('permissions.fixedHint'), page.fixed, '', true)}
  {/if}

  <a class="link link-hover text-xs text-base-content/70" href={agentId ? `/activity?agent=${encodeURIComponent(agentId)}` : '/activity'}>
    {isEmployee ? $t('permissions.activityLinkEmployee', { values: { name } }) : $t('permissions.activityLink')}
  </a>
{/if}

{#if confirmFullAccess}
  <div class="modal modal-open" role="dialog" aria-modal="true" aria-labelledby="full-access-title">
    <div class="modal-box max-w-sm">
      <h3 id="full-access-title" class="text-base font-semibold">{$t('permissions.fullAccessConfirmTitle')}</h3>
      <p class="text-sm text-base-content/70 mt-2">{$t('permissions.fullAccessConfirmBody')}</p>
      <div class="modal-action">
        <button type="button" class="btn btn-sm btn-ghost" onclick={() => (confirmFullAccess = false)}>{$t('permissions.fullAccessKeep')}</button>
        <button type="button" class="btn btn-sm btn-warning" onclick={turnOnFullAccess}>{$t('permissions.fullAccessConfirm')}</button>
      </div>
    </div>
    <button type="button" class="modal-backdrop" aria-label={$t('permissions.fullAccessKeep')} onclick={() => (confirmFullAccess = false)}></button>
  </div>
{/if}
