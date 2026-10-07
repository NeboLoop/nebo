<!--
  Intelligence packs — Developer mode (a feature under test, never shown to an
  owner without it). A pack maps each Effort level (Instant…Max) and the
  Vision / Voice capabilities to a model; the built-in Nebo AI pack runs on
  Janus and is read-only. Assign a pack as the bot's default on Routing
  (General), or to an employee or a chat through the model pickers.
  Developer surfaces are English.
-->
<script lang="ts">
  import SettingsHeader from '$lib/components/settings/SettingsHeader.svelte';
  import { onMount } from 'svelte';
  import Plus from 'lucide-svelte/icons/plus';
  import Pencil from 'lucide-svelte/icons/pencil';
  import Trash2 from 'lucide-svelte/icons/trash-2';
  import X from 'lucide-svelte/icons/x';
  import Spinner from '$lib/components/ui/Spinner.svelte';
  import Alert from '$lib/components/ui/Alert.svelte';
  import * as api from '$lib/api/nebo';
  import type { Pack, PackLevels } from '$lib/api/neboComponents';
  import { EFFORTS, EFFORT_LABELS } from '$lib/models/speeds';

  type Slot = (typeof EFFORTS)[number] | 'vision' | 'voice';
  const SLOTS: { key: Slot; label: string }[] = [
    ...EFFORTS.map((e) => ({ key: e as Slot, label: EFFORT_LABELS[e] })),
    { key: 'vision', label: 'Vision' },
    { key: 'voice', label: 'Voice' },
  ];

  let loading = $state(true);
  let error = $state('');
  let packs = $state<Pack[]>([]);
  let modelGroups = $state<{ provider: string; models: string[] }[]>([]);

  // The editor: null when closed; id '' for a new pack.
  let editing = $state<{ id: string; name: string; levels: PackLevels; fallback: boolean } | null>(null);
  let saving = $state(false);
  let editError = $state('');

  async function load() {
    loading = true;
    error = '';
    try {
      const [packsRes, modelsRes] = await Promise.all([api.listPacks(), api.listModels()]);
      packs = packsRes.packs ?? [];
      const models = (modelsRes.models ?? {}) as Record<string, { id: string; isActive: boolean }[]>;
      modelGroups = Object.entries(models)
        .map(([provider, list]) => ({ provider, models: list.filter((m) => m.isActive).map((m) => `${provider}/${m.id}`) }))
        .filter((g) => g.models.length > 0)
        .sort((a, b) => (a.provider === 'janus' ? -1 : b.provider === 'janus' ? 1 : a.provider.localeCompare(b.provider)));
    } catch (err: any) {
      error = err?.message || 'Could not load the packs.';
    } finally {
      loading = false;
    }
  }

  function openEditor(pack?: Pack) {
    editError = '';
    editing = pack
      ? { id: pack.id, name: pack.name, levels: { ...pack.levels }, fallback: pack.fallback }
      : { id: '', name: '', levels: {}, fallback: true };
  }

  async function save() {
    if (!editing) return;
    saving = true;
    editError = '';
    const levels: PackLevels = {};
    for (const { key } of SLOTS) {
      const v = (editing.levels[key] ?? '').trim();
      if (v) levels[key] = v;
    }
    const body = { name: editing.name, levels, fallback: editing.fallback };
    try {
      if (editing.id) await api.updatePack(editing.id, body);
      else await api.createPack(body);
      editing = null;
      await load();
    } catch (err: any) {
      editError = err?.message || 'Could not save the pack.';
    } finally {
      saving = false;
    }
  }

  async function remove(pack: Pack) {
    if (!confirm(`Remove ${pack.name}? An employee still on it runs on Nebo AI, and is told so.`)) return;
    try {
      await api.deletePack(pack.id);
      await load();
    } catch (err: any) {
      error = err?.message || 'Could not remove the pack.';
    }
  }

  onMount(load);
</script>

<SettingsHeader
  title="Intelligence packs"
  description="Bring your own AI: map each Effort level to a model. A developer feature under test."
/>

{#if loading}
  <div class="flex items-center justify-center gap-3 py-16">
    <Spinner size={20} />
    <span class="text-xs text-base-content/50">Loading packs…</span>
  </div>
{:else}
  <div class="flex flex-col gap-6">
    {#if error}
      <Alert type="error">{error}</Alert>
    {/if}

    <section>
      <div class="flex items-center justify-between mb-2">
        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50">Packs</div>
        <button
          type="button"
          class="flex items-center gap-1 text-xs text-base-content/50 hover:text-primary transition-colors cursor-pointer"
          onclick={() => openEditor()}
        >
          <Plus class="w-3.5 h-3.5" /> New pack
        </button>
      </div>

      <div class="flex flex-col gap-2">
        {#each packs as pack (pack.id)}
          <div class="rounded-lg border border-base-content/5 bg-base-100 p-4">
            <div class="flex items-center justify-between mb-3">
              <div class="flex items-center gap-2">
                <span class="text-sm font-medium">{pack.name}</span>
                {#if pack.builtIn}
                  <span class="text-xs text-base-content/50">Built in · Auto picks the level</span>
                {:else if !pack.fallback}
                  <span class="text-xs text-base-content/50">Never uses Nebo AI</span>
                {/if}
              </div>
              {#if !pack.builtIn}
                <div class="flex items-center gap-3">
                  <button type="button" class="text-base-content/50 hover:text-primary transition-colors cursor-pointer" onclick={() => openEditor(pack)} aria-label="Edit {pack.name}">
                    <Pencil class="w-3.5 h-3.5" />
                  </button>
                  <button type="button" class="text-base-content/30 hover:text-error transition-colors cursor-pointer" onclick={() => remove(pack)} aria-label="Remove {pack.name}">
                    <Trash2 class="w-3.5 h-3.5" />
                  </button>
                </div>
              {/if}
            </div>
            <div class="flex flex-col gap-1.5">
              {#each SLOTS as slot}
                <div class="flex items-center justify-between py-1.5 px-3 rounded-md bg-base-200/50">
                  <span class="text-sm">{slot.label}</span>
                  <span class="text-xs text-base-content/50 font-mono">{pack.levels[slot.key] ?? '—'}</span>
                </div>
              {/each}
            </div>
          </div>
        {/each}
      </div>
    </section>
  </div>
{/if}

{#if editing}
  <div class="fixed inset-0 z-50 flex items-center justify-center">
    <button type="button" class="absolute inset-0 bg-base-content/40 cursor-default" onclick={() => (editing = null)} aria-label="Close"></button>
    <div class="relative bg-base-100 rounded-xl border border-base-300 shadow-lg w-full max-w-lg" role="dialog" aria-modal="true">
      <div class="flex items-center justify-between px-5 py-4 border-b border-base-content/10">
        <h3 class="text-base font-semibold">{editing.id ? 'Edit pack' : 'New pack'}</h3>
        <button type="button" onclick={() => (editing = null)} class="text-base-content/50 hover:text-base-content transition-colors cursor-pointer" aria-label="Close">
          <X class="w-4 h-4" />
        </button>
      </div>
      <div class="px-5 py-5 flex flex-col gap-4">
        <div>
          <label class="text-xs font-medium text-base-content/70 mb-1 block" for="pack-name">Name</label>
          <input id="pack-name" type="text" bind:value={editing.name} placeholder="My Claude" class="input input-bordered input-sm w-full" />
        </div>
        {#each SLOTS as slot}
          <div>
            <label class="text-xs font-medium text-base-content/70 mb-1 block" for="pack-{slot.key}">{slot.label}</label>
            <select id="pack-{slot.key}" bind:value={editing.levels[slot.key]} class="select select-bordered select-sm w-full">
              <option value={undefined}>— {slot.key === 'vision' || slot.key === 'voice' ? 'Nebo AI' : 'the nearest level that is set'}</option>
              {#each modelGroups as group}
                <optgroup label={group.provider}>
                  {#each group.models as model}
                    <option value={model}>{model}</option>
                  {/each}
                </optgroup>
              {/each}
            </select>
          </div>
        {/each}
        <label class="flex items-center gap-2 text-sm cursor-pointer">
          <input type="checkbox" class="toggle toggle-sm toggle-primary" bind:checked={editing.fallback} />
          If this pack's AI isn't connected, use Nebo AI instead (and say so)
        </label>
        {#if editError}
          <Alert type="error">{editError}</Alert>
        {/if}
      </div>
      <div class="flex items-center justify-end gap-2 px-5 py-4 border-t border-base-content/10">
        <button type="button" class="btn btn-ghost btn-sm" onclick={() => (editing = null)}>Cancel</button>
        <button type="button" class="btn btn-primary btn-sm" onclick={save} disabled={saving}>
          {#if saving}<Spinner size={14} /> Saving…{:else}Save{/if}
        </button>
      </div>
    </div>
  </div>
{/if}
