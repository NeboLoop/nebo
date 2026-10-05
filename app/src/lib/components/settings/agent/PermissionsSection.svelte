<!--
  PermissionsSection — what an employee may do, in plain words (Settings →
  employee → Permissions). With no agentId it is the company defaults page,
  with the same controls (Settings → Permissions).

  Every choice is the same three-way switch — Allow, Ask, Off — on each
  capability, each connected service's default and its actions or tools,
  and each specific setting. A switch with no setting of the page's own
  shows its default's or the company's value, dimmed. The server builds the
  page (sentences, groups, current and inherited values); the page never
  shows a rule. Changes save as they are made: a mode, a switch, a folder
  added or removed, a money amount typed (debounced).
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import { t } from 'svelte-i18n';
  import Check from 'lucide-svelte/icons/check';
  import X from 'lucide-svelte/icons/x';
  import Lock from 'lucide-svelte/icons/lock';
  import Hand from 'lucide-svelte/icons/hand';
  import Ban from 'lucide-svelte/icons/ban';
  import FolderPlus from 'lucide-svelte/icons/folder-plus';
  import AlertTriangle from 'lucide-svelte/icons/alert-triangle';
  import * as api from '$lib/api/nebo';
  import type { PermissionItem, PermissionSwitch, PermissionsPage, MoneyAmounts } from '$lib/api/nebo';
  import Spinner from '$lib/components/ui/Spinner.svelte';
  import { switchStates, switchChange, inheritedNote, type SwitchChange, type SwitchValue } from '$lib/utils/permissionSwitch';

  let { agentId, name = '' }: { agentId?: string; name?: string } = $props();

  /** The server's mode ids. */
  type Mode = 'automatic' | 'ask' | 'plan' | 'full_access';

  let page = $state<PermissionsPage | null>(null);
  let loading = $state(true);
  let error = $state('');
  let saved = $state(false);
  let savedTimer: ReturnType<typeof setTimeout> | null = null;
  let confirmFullAccess = $state(false);

  /** Money amounts as typed, in dollars, keyed by item id. */
  let moneyInputs = $state<Record<string, { perAction: string; perDay: string; perDayCount: string }>>({});
  const moneyTimers: Record<string, ReturnType<typeof setTimeout>> = {};

  const modes: { id: Mode; label: string; desc: string }[] = [
    { id: 'automatic', label: 'permissions.modeAutomatic', desc: 'permissions.modeAutomaticDesc' },
    { id: 'ask', label: 'permissions.modeAsk', desc: 'permissions.modeAskDesc' },
    { id: 'plan', label: 'permissions.modePlan', desc: 'permissions.modePlanDesc' },
    { id: 'full_access', label: 'permissions.modeFullAccess', desc: 'permissions.modeFullAccessDesc' }
  ];

  /** Each switch state's icon and lit colour. */
  const stateLook: Record<SwitchValue, { icon: typeof Check; lit: string }> = {
    allow: { icon: Check, lit: 'btn-active text-success' },
    ask: { icon: Hand, lit: 'btn-active text-warning' },
    deny: { icon: Ban, lit: 'btn-active text-error' }
  };

  function modeLabel(m: string): string {
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

  function setSwitch(sw: PermissionSwitch, value: SwitchChange | null) {
    if (value) void change({ set: { id: sw.id, value } });
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

{#snippet toggle(sw: PermissionSwitch)}
  <div class="join shrink-0 {sw.inherited ? 'opacity-70' : ''}" role="group" aria-label={sw.sentence}>
    {#each switchStates as s (s.value)}
      {@const look = stateLook[s.value]}
      <button
        type="button"
        class="btn btn-xs join-item {sw.value === s.value ? look.lit : 'btn-ghost text-base-content/50'}"
        title={$t(s.long)}
        aria-label={$t(s.long)}
        aria-pressed={sw.value === s.value}
        disabled={sw.locked}
        onclick={() => setSwitch(sw, switchChange(sw, s.value))}
      >
        <look.icon class="w-3 h-3" />
        <span class="hidden sm:inline">{$t(s.label)}</span>
      </button>
    {/each}
  </div>
{/snippet}

{#snippet switchRow(sw: PermissionSwitch, title = '')}
  {@const note = inheritedNote(sw)}
  <li class="flex items-center gap-3 py-2 flex-wrap">
    <span class="flex-1 min-w-0 text-sm">
      {#if sw.locked}<Lock class="w-3.5 h-3.5 inline me-1 align-[-2px] text-base-content/50" />{/if}
      {title || sw.sentence}
      {#if note}<span class="text-xs text-base-content/50 ms-1.5">{$t(note)}</span>{/if}
    </span>
    {@render toggle(sw)}
  </li>
{/snippet}

{#snippet itemRow(item: PermissionItem, locked = false)}
  <li class="flex items-start gap-3 py-2.5">
    {#if locked}<Lock class="w-3.5 h-3.5 mt-0.5 shrink-0 text-base-content/50" />{/if}
    <span class="flex-1 min-w-0 text-sm">
      {item.sentence}
      {#if item.fromCompany}
        <span class="badge badge-ghost badge-xs ms-1.5 align-middle">{$t('permissions.fromCompany')}</span>
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

  <!-- What employees can do -->
  <section class="mb-7">
    <h3 class="text-sm font-semibold">{isEmployee ? $t('permissions.canDoTitleEmployee', { values: { name } }) : $t('permissions.canDoTitle')}</h3>
    <p class="text-xs text-base-content/70 mt-0.5">{$t('permissions.canDoHint')}</p>
    <ul class="divide-y divide-base-content/10 mt-1">
      {#each page.capabilities as sw (sw.id)}
        {@render switchRow(sw)}
      {/each}
    </ul>
  </section>

  <!-- Connected services: plugins and MCP servers -->
  {#if page.groups.length > 0}
    <section class="mb-7">
      <h3 class="text-sm font-semibold">{$t('permissions.servicesTitle')}</h3>
      <p class="text-xs text-base-content/70 mt-0.5">{$t('permissions.servicesHint')}</p>
      <div class="flex flex-col gap-3 mt-2">
        {#each page.groups as g (g.id)}
          <div class="rounded-lg border border-base-300">
            <div class="px-3.5 pt-2.5">
              <div class="text-sm font-medium">{g.title}</div>
              {#if g.subtitle}<div class="text-xs text-base-content/70">{g.subtitle}</div>{/if}
            </div>
            <ul class="divide-y divide-base-content/10 px-3.5">
              {@render switchRow(g.default, $t('permissions.groupDefault'))}
              {#each g.rows as sw (sw.id)}
                {@render switchRow(sw)}
              {/each}
            </ul>
          </div>
        {/each}
      </div>
    </section>
  {/if}

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
                {#if item.fromCompany}<span class="badge badge-ghost badge-xs ms-1.5 align-middle">{$t('permissions.fromCompany')}</span>{/if}
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

  <!-- Specific actions -->
  {#if page.specific.length > 0}
    <section class="mb-7">
      <h3 class="text-sm font-semibold">{$t('permissions.specificTitle')}</h3>
      <p class="text-xs text-base-content/70 mt-0.5">{$t('permissions.specificHint')}</p>
      <ul class="divide-y divide-base-content/10 mt-1">
        {#each page.specific as sw (sw.id)}
          {@render switchRow(sw)}
        {/each}
      </ul>
    </section>
  {/if}

  <!-- Safety rules -->
  {#if page.alwaysAsks.length > 0}
    <section class="mb-7">
      <h3 class="text-sm font-semibold">{$t('permissions.alwaysAsksTitle')}</h3>
      <p class="text-xs text-base-content/70 mt-0.5">{$t('permissions.alwaysAsksHint')}</p>
      <ul class="divide-y divide-base-content/10 mt-1">
        {#each page.alwaysAsks as item (item.id)}
          {@render itemRow(item, true)}
        {/each}
      </ul>
    </section>
  {/if}

  {#if page.fixed.length > 0}
    <section class="mb-7">
      <h3 class="text-sm font-semibold">{$t('permissions.fixedTitle')}</h3>
      <p class="text-xs text-base-content/70 mt-0.5">{$t('permissions.fixedHint')}</p>
      <ul class="divide-y divide-base-content/10 mt-1">
        {#each page.fixed as item (item.id)}
          {@render itemRow(item, true)}
        {/each}
      </ul>
    </section>
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
