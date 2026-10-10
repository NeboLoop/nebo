<!--
  Intelligence Packs — Developer mode (a feature under test, never shown to an
  owner without it). What runs on what: the bot's default, then each pack as
  one table — the Effort levels (Auto is Nebo AI's alone) and the Vision,
  Voice and Decisions capabilities, each naming the connection and model it
  runs on. The built-in Nebo AI pack runs on Janus and is read-only; the
  owner's packs take any model of any provider added on Providers. A pack is
  assigned to an employee or a chat through the model pickers.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import SettingsHeader from '$lib/components/settings/SettingsHeader.svelte';
  import { onMount } from 'svelte';
  import Plus from 'lucide-svelte/icons/plus';
  import Trash2 from 'lucide-svelte/icons/trash-2';
  import X from 'lucide-svelte/icons/x';
  import Spinner from '$lib/components/ui/Spinner.svelte';
  import Alert from '$lib/components/ui/Alert.svelte';
  import * as api from '$lib/api/nebo';
  import type { LevelEffort, Pack, PackChoice, PackLanes, PackLevels } from '$lib/api/neboComponents';
  import { EFFORTS, type Effort } from '$lib/models/speeds';
  import { choiceProviderLabel, isDefaultPack, levelRows, packUsesLocal, runsOn } from '$lib/models/connections';

  // PLACEHOLDER — the Janus routing fee is not decided yet. Replace with the
  // real fee (from Janus or pricing) before Route through Janus ships.
  const ROUTING_FEE_PLACEHOLDER = '[FEE]';

  const LANES = ['heartbeat', 'scheduled', 'communication', 'helpers', 'build', 'workflow'] as const;
  type Lane = (typeof LANES)[number];
  const LANE_KEYS: Record<Lane, string> = {
    heartbeat: 'settingsPacks.laneHeartbeat',
    scheduled: 'settingsPacks.laneScheduled',
    communication: 'settingsPacks.laneCommunication',
    helpers: 'settingsPacks.laneHelpers',
    build: 'settingsPacks.laneBuild',
    workflow: 'settingsPacks.laneWorkflow',
  };
  const PROVIDER_EFFORTS = ['low', 'medium', 'high'] as const;

  let loading = $state(true);
  let error = $state('');
  let packs = $state<Pack[]>([]);
  let defaultValue = $state('');
  let savingDefault = $state(false);
  let chatChoices = $state<PackChoice[]>([]);
  let decisionChoices = $state<PackChoice[]>([]);
  const allChoices = $derived([...chatChoices, ...decisionChoices]);

  // The editor: null when closed; id '' for a new pack.
  let editing = $state<{
    id: string; name: string; levels: PackLevels; fallback: boolean;
    routeThroughJanus: boolean; lanes: PackLanes; levelEffort: LevelEffort;
  } | null>(null);
  let saving = $state(false);
  let editError = $state('');

  // Models that see images, plus the pack's current Vision model so a saved choice always shows.
  const visionChoices = $derived(
    chatChoices.filter((c) => c.capabilities?.includes('vision') || c.value === editing?.levels.vision),
  );

  function levelLabel(level: Effort | 'auto' | 'every'): string {
    return $t(`settingsPacks.level.${level}`);
  }

  function choiceLabel(c: PackChoice): string {
    return `${choiceProviderLabel(c)} · ${c.modelId}`;
  }

  async function load() {
    loading = true;
    error = '';
    try {
      const [packsRes, choicesRes] = await Promise.all([api.listPacks(), api.listPackChoices()]);
      packs = packsRes.packs ?? [];
      defaultValue = packsRes.default ?? '';
      chatChoices = choicesRes.chat ?? [];
      decisionChoices = choicesRes.decision ?? [];
    } catch (err: any) {
      error = err?.message || $t('settingsPacks.loadFailed');
    } finally {
      loading = false;
    }
  }

  /** The bot-default options: each pack (Auto or its default level), then each of its levels. */
  const defaultOptions = $derived(
    packs.flatMap((p) => [
      { value: `pack/${p.id}`, label: p.levels.auto ? `${p.name} · ${levelLabel('auto')}` : p.name },
      ...EFFORTS.map((e) => ({ value: `pack/${p.id}/${e}`, label: `${p.name} · ${levelLabel(e)}` })),
    ]),
  );
  /** A default that is a model rather than a pack still shows as itself. */
  const defaultIsOther = $derived(!!defaultValue && !defaultOptions.some((o) => o.value === defaultValue));

  async function setDefault(value: string) {
    const previous = defaultValue;
    defaultValue = value;
    savingDefault = true;
    try {
      const res = await api.setDefaultPack({ value });
      defaultValue = res.default ?? value;
    } catch (err: any) {
      defaultValue = previous;
      error = err?.message || $t('settingsPacks.defaultSaveFailed');
    } finally {
      savingDefault = false;
    }
  }

  function openEditor(pack?: Pack) {
    editError = '';
    editing = pack
      ? {
          id: pack.id, name: pack.name, levels: { ...pack.levels }, fallback: pack.fallback,
          routeThroughJanus: !!pack.routeThroughJanus, lanes: { ...(pack.lanes ?? {}) }, levelEffort: { ...(pack.levelEffort ?? {}) },
        }
      : { id: '', name: '', levels: {}, fallback: true, routeThroughJanus: false, lanes: {}, levelEffort: {} };
  }

  /** Drop empty values: absent means "nearest level" / "chosen by the work" / "provider default". */
  function compact<T extends object>(obj: T): T {
    const out: Record<string, string> = {};
    for (const [k, v] of Object.entries(obj)) {
      const s = typeof v === 'string' ? v.trim() : '';
      if (s) out[k] = s;
    }
    return out as T;
  }

  const editorUsesLocal = $derived(
    !!editing && Object.values(editing.levels).some((v) => !!v && !!allChoices.find((c) => c.value === v)?.local),
  );

  async function save() {
    if (!editing) return;
    if (!editing.name.trim()) { editError = $t('settingsPacks.nameRequired'); return; }
    saving = true;
    editError = '';
    const body = {
      name: editing.name.trim(),
      levels: compact(editing.levels),
      fallback: editing.fallback,
      routeThroughJanus: editing.routeThroughJanus,
      lanes: compact(editing.lanes),
      levelEffort: compact(editing.levelEffort),
    };
    try {
      if (editing.id) await api.updatePack(editing.id, body);
      else await api.createPack(body);
      editing = null;
      await load();
    } catch (err: any) {
      editError = err?.message || $t('settingsPacks.saveFailed');
    } finally {
      saving = false;
    }
  }

  async function remove(pack: Pack) {
    if (!confirm($t('settingsPacks.removeConfirm', { values: { name: pack.name } }))) return;
    try {
      await api.deletePack(pack.id);
      await load();
    } catch (err: any) {
      error = err?.message || $t('settingsPacks.removeFailed');
    }
  }

  onMount(load);
</script>

{#snippet runsOnCell(value: string | undefined, empty: string, effort?: string)}
  {@const on = runsOn(value, allChoices)}
  {#if on}
    <span class="flex flex-col items-end text-end min-w-0">
      <span class="text-sm">{on.label}{#if effort}{' · '}{$t('settingsPacks.withEffort', { values: { effort: $t(`settingsPacks.level.${effort}`) } })}{/if}</span>
      <span class="text-xs text-base-content/50 font-mono truncate max-w-full">{on.modelId}</span>
    </span>
  {:else}
    <span class="text-xs text-base-content/50 text-end">{empty}</span>
  {/if}
{/snippet}

<SettingsHeader
  title={$t('settingsPacks.title')}
  description={$t('settingsPacks.description')}
/>

{#if loading}
  <div class="flex items-center justify-center gap-3 py-16">
    <Spinner size={20} />
    <span class="text-xs text-base-content/50">{$t('settingsPacks.loading')}</span>
  </div>
{:else}
  <div class="flex flex-col gap-6">
    {#if error}
      <Alert type="error">{error}</Alert>
    {/if}

    <!-- Bot default -->
    <section>
      <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">{$t('settingsPacks.botDefault')}</div>
      <div class="rounded-lg border border-base-content/5 bg-base-100 p-4 flex flex-col gap-2">
        <label class="text-xs font-medium text-base-content/70 block" for="bot-default">{$t('settingsPacks.botDefaultLabel')}</label>
        <div class="flex items-center gap-3">
          <select
            id="bot-default"
            class="select select-bordered select-sm w-full max-w-sm"
            value={defaultValue}
            disabled={savingDefault}
            onchange={(e) => setDefault((e.currentTarget as HTMLSelectElement).value)}
          >
            {#if defaultIsOther}
              {@const on = runsOn(defaultValue, allChoices)}
              <option value={defaultValue}>{on ? `${on.label} · ${on.modelId}` : defaultValue}</option>
            {/if}
            {#each defaultOptions as opt (opt.value)}
              <option value={opt.value}>{opt.label}</option>
            {/each}
          </select>
          {#if savingDefault}<Spinner size={14} />{/if}
        </div>
        <span class="text-xs text-base-content/50">{$t('settingsPacks.botDefaultHint')}</span>
      </div>
    </section>

    <!-- Packs -->
    <section>
      <div class="flex items-center justify-between mb-2">
        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50">{$t('settingsPacks.packs')}</div>
        <button
          type="button"
          class="flex items-center gap-1 text-xs text-base-content/50 hover:text-primary transition-colors cursor-pointer"
          onclick={() => openEditor()}
        >
          <Plus class="w-3.5 h-3.5" /> {$t('settingsPacks.newPack')}
        </button>
      </div>

      <div class="flex flex-col gap-3">
        {#each packs as pack (pack.id)}
          <div class="rounded-lg border border-base-content/5 bg-base-100 p-4">
            <div class="flex items-center justify-between gap-3 mb-3">
              <div class="flex items-center gap-2 flex-wrap min-w-0">
                <span class="text-sm font-medium">{pack.name}</span>
                {#if pack.builtIn}
                  <span class="badge badge-ghost badge-sm">{$t('settingsPacks.builtIn')}</span>
                {/if}
                {#if isDefaultPack(defaultValue, pack.id)}
                  <span class="badge badge-primary badge-sm">{$t('settingsPacks.badgeDefault')}</span>
                {/if}
                {#if pack.routeThroughJanus}
                  <span class="badge badge-info badge-outline badge-sm">{$t('settingsPacks.badgeJanus')}</span>
                {/if}
                {#if !pack.builtIn && !pack.fallback}
                  <span class="badge badge-warning badge-outline badge-sm">{$t('settingsPacks.neverNeboAi')}</span>
                {/if}
              </div>
              {#if pack.builtIn}
                <span class="text-xs text-base-content/40 shrink-0">{$t('settingsPacks.readOnly')}</span>
              {:else}
                <div class="flex items-center gap-3 shrink-0">
                  <button type="button" class="text-xs text-base-content/50 hover:text-primary transition-colors cursor-pointer" onclick={() => openEditor(pack)} aria-label={$t('settingsPacks.editNamed', { values: { name: pack.name } })}>
                    {$t('common.edit')}
                  </button>
                  <button type="button" class="text-base-content/30 hover:text-error transition-colors cursor-pointer" onclick={() => remove(pack)} aria-label={$t('settingsPacks.removeNamed', { values: { name: pack.name } })}>
                    <Trash2 class="w-3.5 h-3.5" />
                  </button>
                </div>
              {/if}
            </div>

            <div class="flex flex-col gap-1">
              <div class="flex items-center justify-between px-3 text-xs font-medium text-base-content/50">
                <span>{$t('settingsPacks.colLevel')}</span>
                <span>{$t('settingsPacks.colRunsOn')}</span>
              </div>
              {#each levelRows(pack) as row (row.level)}
                <div class="flex items-center justify-between gap-3 py-1.5 px-3 rounded-md bg-base-200/50">
                  <span class="text-sm shrink-0">{levelLabel(row.level)}</span>
                  {#if row.level === 'auto'}
                    <span class="text-xs text-base-content/50 text-end">{$t('settingsPacks.autoRunsOn')}</span>
                  {:else}
                    {@render runsOnCell(row.value, $t('settingsPacks.emptyLevel'), row.level === 'every' ? undefined : pack.levelEffort?.[row.level])}
                  {/if}
                </div>
              {/each}
              <div class="px-3 pt-2 text-xs font-medium text-base-content/50">{$t('settingsPacks.capabilities')}</div>
              <div class="flex items-center justify-between gap-3 py-1.5 px-3 rounded-md bg-base-200/50">
                <span class="text-sm shrink-0">{$t('settingsPacks.vision')}</span>
                {@render runsOnCell(pack.levels.vision, $t('settingsPacks.visionDefault'))}
              </div>
              <div class="flex items-center justify-between gap-3 py-1.5 px-3 rounded-md bg-base-200/50">
                <span class="text-sm shrink-0">{$t('settingsPacks.voice')}</span>
                {@render runsOnCell(pack.levels.voice, pack.fallback || pack.builtIn ? $t('settingsPacks.voiceDefault') : $t('settingsPacks.voiceUnavailable'))}
              </div>
              <div class="flex items-center justify-between gap-3 py-1.5 px-3 rounded-md bg-base-200/50">
                <span class="text-sm shrink-0">{$t('settingsPacks.decisions')}</span>
                {@render runsOnCell(pack.levels.decisions, pack.fallback || pack.builtIn ? $t('settingsPacks.decisionsDefault') : $t('settingsPacks.decisionsNone'))}
              </div>
            </div>
            {#if !pack.builtIn && packUsesLocal(pack, allChoices)}
              <p class="text-xs text-base-content/50 mt-2">{$t('settingsPacks.localNoJanus')}</p>
            {/if}
          </div>
        {/each}
      </div>
    </section>
  </div>
{/if}

{#if editing}
  <div class="fixed inset-0 z-50 flex items-center justify-center p-4">
    <button type="button" class="absolute inset-0 bg-base-content/40 cursor-default" onclick={() => (editing = null)} aria-label={$t('common.close')}></button>
    <div class="relative bg-base-100 rounded-xl border border-base-300 shadow-lg w-full max-w-xl max-h-full flex flex-col" role="dialog" aria-modal="true" aria-labelledby="pack-editor-title">
      <div class="flex items-center justify-between px-5 py-4 border-b border-base-content/10 shrink-0">
        <h3 id="pack-editor-title" class="text-base font-semibold">{editing.id ? $t('settingsPacks.editPack') : $t('settingsPacks.newPack')}</h3>
        <button type="button" onclick={() => (editing = null)} class="text-base-content/50 hover:text-base-content transition-colors cursor-pointer" aria-label={$t('common.close')}>
          <X class="w-4 h-4" />
        </button>
      </div>
      <div class="px-5 py-5 flex flex-col gap-6 overflow-y-auto">
        <div>
          <label class="text-xs font-medium text-base-content/70 mb-1 block" for="pack-name">{$t('settingsPacks.name')}</label>
          <input id="pack-name" type="text" bind:value={editing.name} placeholder={$t('settingsPacks.namePlaceholder')} class="input input-bordered input-sm w-full" />
        </div>

        <!-- Effort levels -->
        <section class="flex flex-col gap-2">
          <div>
            <h4 class="text-sm font-semibold">{$t('settingsPacks.effortLevels')}</h4>
            <p class="text-xs text-base-content/50">{$t('settingsPacks.effortLevelsHint')}</p>
          </div>
          <div class="grid grid-cols-[5rem_1fr_7rem] gap-2 items-center">
            <span class="text-xs font-medium text-base-content/50">{$t('settingsPacks.colLevel')}</span>
            <span class="text-xs font-medium text-base-content/50">{$t('settingsPacks.colModel')}</span>
            <span class="text-xs font-medium text-base-content/50">{$t('settingsPacks.colProviderEffort')}</span>
            {#each EFFORTS as level}
              <label class="text-sm" for="lv-{level}">{levelLabel(level)}</label>
              <select id="lv-{level}" bind:value={editing.levels[level]} class="select select-bordered select-sm w-full min-w-0">
                <option value={undefined}>{$t('settingsPacks.emptyLevel')}</option>
                {#each chatChoices as c (c.value)}
                  <option value={c.value}>{choiceLabel(c)}</option>
                {/each}
              </select>
              <select
                bind:value={editing.levelEffort[level]}
                class="select select-bordered select-sm w-full"
                aria-label={$t('settingsPacks.providerEffortNamed', { values: { level: levelLabel(level) } })}
              >
                <option value={undefined}>{$t('settingsPacks.effortDefault')}</option>
                {#each PROVIDER_EFFORTS as e}
                  <option value={e}>{levelLabel(e)}</option>
                {/each}
              </select>
            {/each}
          </div>
        </section>

        <!-- Capabilities -->
        <section class="flex flex-col gap-2">
          <div>
            <h4 class="text-sm font-semibold">{$t('settingsPacks.capabilities')}</h4>
            <p class="text-xs text-base-content/50">{$t('settingsPacks.capabilitiesHint')}</p>
          </div>
          <div class="grid grid-cols-[5rem_1fr] gap-2 items-center">
            <label class="text-sm" for="cap-vision">{$t('settingsPacks.vision')}</label>
            <select id="cap-vision" bind:value={editing.levels.vision} class="select select-bordered select-sm w-full min-w-0">
              <option value={undefined}>{$t('settingsPacks.visionDefault')}</option>
              {#each visionChoices as c (c.value)}
                <option value={c.value}>{choiceLabel(c)}</option>
              {/each}
            </select>
            <label class="text-sm" for="cap-voice">{$t('settingsPacks.voice')}</label>
            <select id="cap-voice" bind:value={editing.levels.voice} class="select select-bordered select-sm w-full min-w-0">
              <option value={undefined}>{$t('settingsPacks.voiceDefault')}</option>
              {#each chatChoices as c (c.value)}
                <option value={c.value}>{choiceLabel(c)}</option>
              {/each}
            </select>
            <label class="text-sm" for="cap-decisions">{$t('settingsPacks.decisions')}</label>
            <select id="cap-decisions" bind:value={editing.levels.decisions} class="select select-bordered select-sm w-full min-w-0">
              <option value={undefined}>{$t('settingsPacks.decisionsDefaultOption')}</option>
              {#each decisionChoices as c (c.value)}
                <option value={c.value}>{choiceLabel(c)}</option>
              {/each}
            </select>
          </div>
        </section>

        <!-- Lanes -->
        <section class="flex flex-col gap-2">
          <div>
            <h4 class="text-sm font-semibold">{$t('settingsPacks.lanes')}</h4>
            <p class="text-xs text-base-content/50">{$t('settingsPacks.lanesHint')}</p>
          </div>
          <div class="grid grid-cols-[7rem_1fr] gap-2 items-center">
            {#each LANES as lane}
              <label class="text-sm" for="ln-{lane}">{$t(LANE_KEYS[lane])}</label>
              <select id="ln-{lane}" bind:value={editing.lanes[lane]} class="select select-bordered select-sm w-full">
                <option value={undefined}>{$t('settingsPacks.laneByWork')}</option>
                {#each EFFORTS as e}
                  <option value={e}>{levelLabel(e)}</option>
                {/each}
              </select>
            {/each}
          </div>
        </section>

        <!-- Routing and fallback -->
        <section class="flex flex-col gap-4">
          <label class="flex items-start justify-between gap-4 cursor-pointer">
            <span class="flex flex-col gap-1">
              <span class="text-sm font-medium">{$t('settingsPacks.routeJanus')}</span>
              <span class="text-xs text-base-content/50">{$t('settingsPacks.routeJanusHint', { values: { fee: ROUTING_FEE_PLACEHOLDER } })}</span>
              <span class="text-xs {editorUsesLocal ? 'text-warning' : 'text-base-content/40'}">{$t('settingsPacks.routeJanusLocal')}</span>
            </span>
            <input type="checkbox" class="toggle toggle-sm toggle-primary shrink-0 mt-0.5" bind:checked={editing.routeThroughJanus} />
          </label>
          <label class="flex items-start justify-between gap-4 cursor-pointer">
            <span class="flex flex-col gap-1">
              <span class="text-sm font-medium">{$t('settingsPacks.fallback')}</span>
              <span class="text-xs text-base-content/50">{$t('settingsPacks.fallbackHint')}</span>
            </span>
            <input type="checkbox" class="toggle toggle-sm toggle-primary shrink-0 mt-0.5" bind:checked={editing.fallback} />
          </label>
        </section>

        {#if editError}
          <Alert type="error">{editError}</Alert>
        {/if}
      </div>
      <div class="flex items-center justify-end gap-2 px-5 py-4 border-t border-base-content/10 shrink-0">
        <button type="button" class="btn btn-ghost btn-sm" onclick={() => (editing = null)}>{$t('common.cancel')}</button>
        <button type="button" class="btn btn-primary btn-sm" onclick={save} disabled={saving}>
          {#if saving}<Spinner size={14} /> {$t('common.saving')}{:else}{$t('common.save')}{/if}
        </button>
      </div>
    </div>
  </div>
{/if}
