<!--
  ModelControls — which Nebo 1 speed one employee works at
  (Settings → employee → General, below the spending limit).

  The list is the sellable list Janus publishes (GET /v1/models, synced into
  the catalog): Default, then the named speeds with the one-line job each is
  for. Picking one writes the entity's modelPreference through the ONE
  entity-config pathway; Default clears it so each task picks for itself.
  Saves on tap — no Save button.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import * as api from '$lib/api/nebo';
  import { loadModelOptions, type ModelOption } from '$lib/models/speeds';

  let { agentId }: { agentId: string } = $props();

  let loading = $state(true);
  let saving = $state(false);
  let saved = $state(false);
  let options = $state<ModelOption[]>([]);
  let selected = $state('');

  async function load() {
    loading = true;
    try {
      const [opts, cfgRes] = await Promise.all([
        loadModelOptions(),
        api.getEntityConfig('agent', agentId) as Promise<{ config?: { modelPreference?: string | null } }>,
      ]);
      options = opts;
      selected = cfgRes.config?.modelPreference ?? '';
    } catch {
      options = [];
    } finally {
      loading = false;
    }
  }

  async function pick(value: string) {
    if (value === selected) return;
    const previous = selected;
    selected = value;
    saving = true;
    saved = false;
    try {
      await api.updateEntityConfig('agent', agentId, { modelPreference: value });
      saved = true;
      setTimeout(() => (saved = false), 1500);
    } catch {
      selected = previous;
    } finally {
      saving = false;
    }
  }

  onMount(load);
</script>

{#if options.length > 0}
  <div class="max-w-2xl">
    <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5 flex items-center gap-2">
      {$t('modelPick.title')}
      {#if saving}
        <span class="loading loading-spinner loading-xs"></span>
      {:else if saved}
        <span class="text-xs font-normal normal-case tracking-normal text-success">{$t('common.saved')}</span>
      {/if}
    </div>
    <div class="rounded-lg border border-base-300 divide-y divide-base-300">
      {#each options as opt (opt.value)}
        <label class="flex items-center gap-3 px-3.5 py-2.5 cursor-pointer hover:bg-base-200/50">
          <input
            type="radio"
            name="model-pick-{agentId}"
            class="radio radio-sm"
            value={opt.value}
            checked={selected === opt.value}
            disabled={loading || saving}
            onchange={() => pick(opt.value)}
          />
          <span class="min-w-0">
            <span class="block text-sm">{opt.label}</span>
            {#if opt.description}
              <span class="block text-xs text-base-content/60">{opt.description}</span>
            {/if}
          </span>
        </label>
      {/each}
    </div>
    <p class="text-xs text-base-content/60 mt-1.5">{$t('modelPick.hint')}</p>
  </div>
{/if}
