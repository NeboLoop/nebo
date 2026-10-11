<!--
  ModelControls — what one employee's new chats run on
  (Settings → employee → General, below the spending limit).

  The same two choices as the composer's Intelligence popover: the level
  (Instant … Max; none reads as Medium) and the intelligence pack those levels
  run on. Picking writes the entity's modelPreference through the ONE
  entity-config pathway; a pick equal to the bot's default clears it. Saves on
  tap — no Save button. A raw model no pack names shows as one checked
  "Custom model" row.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import Check from 'lucide-svelte/icons/check';
  import Settings2 from 'lucide-svelte/icons/settings-2';
  import Zap from 'lucide-svelte/icons/zap';
  import Feather from 'lucide-svelte/icons/feather';
  import Gauge from 'lucide-svelte/icons/gauge';
  import Brain from 'lucide-svelte/icons/brain';
  import Sparkles from 'lucide-svelte/icons/sparkles';
  import * as api from '$lib/api/nebo';
  import {
    EFFORTS,
    NEBO_AI_ID,
    isCustom,
    loadIntelligence,
    packName,
    resolve,
    shownLevel,
    valueFor,
    type Effort,
    type Intelligence,
  } from '$lib/models/speeds';

  let { agentId }: { agentId: string } = $props();

  const LEVEL_ICONS = { instant: Zap, low: Feather, medium: Gauge, high: Brain, max: Sparkles } as const;

  let loading = $state(true);
  let saving = $state(false);
  let saved = $state(false);
  let intel = $state<Intelligence | null>(null);
  let selected = $state('');

  const choice = $derived(resolve(selected || intel?.botDefault || '', intel?.packs ?? []));
  const checkedLevel = $derived(shownLevel(choice));
  const currentPack = $derived(isCustom(choice) ? null : choice.pack);

  async function load() {
    loading = true;
    try {
      const [cfgRes, packs] = await Promise.all([
        api.getEntityConfig('agent', agentId) as Promise<{ config?: { modelPreference?: string | null } }>,
        loadIntelligence(),
      ]);
      selected = cfgRes.config?.modelPreference ?? '';
      intel = packs;
    } catch {
      intel = null;
    } finally {
      loading = false;
    }
  }

  async function pick(pack: string, level: Effort | null) {
    if (!intel) return;
    const value = valueFor(pack, level, intel.botDefault, intel.packs);
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

{#snippet customRow()}
  <div class="intel-row" data-testid="intel-custom">
    <Settings2 class="intel-icon" />
    <span class="intel-text">
      <span class="intel-name">{$t('modelPick.customModel')}</span>
      <span class="intel-desc">{$t('modelPick.customModelDesc')}</span>
    </span>
    <span class="intel-check" data-testid="intel-check"><Check class="w-4 h-4" /></span>
  </div>
{/snippet}

{#if intel}
  <div class="intel-settings">
    <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 flex items-center gap-2">
      {$t('modelPick.intelligence')}
      {#if saving}
        <span class="loading loading-spinner loading-xs"></span>
      {:else if saved}
        <span class="text-xs font-normal normal-case tracking-normal text-success">{$t('common.saved')}</span>
      {/if}
    </div>
    <div class="intel-card" role="radiogroup" aria-label={$t('modelPick.intelligence')}>
      {#if isCustom(choice)}{@render customRow()}{/if}
      {#each EFFORTS as level (level)}
        {@const Icon = LEVEL_ICONS[level]}
        <button
          type="button"
          class="intel-row"
          role="radio"
          aria-checked={checkedLevel === level}
          disabled={loading || saving}
          onclick={() => pick(isCustom(choice) ? NEBO_AI_ID : choice.pack, level)}
        >
          <Icon class="intel-icon" />
          <span class="intel-text">
            <span class="intel-name">
              {$t(`settingsPacks.level.${level}`)}
              {#if level === 'medium'}<span class="badge badge-sm badge-primary badge-soft">{$t('modelPick.recommended')}</span>{/if}
              {#if level === 'max'}<span class="badge badge-sm badge-warning badge-soft">{$t('modelPick.mostBalance')}</span>{/if}
            </span>
            <span class="intel-desc">{$t(`modelPick.levelDesc.${level}`)}</span>
          </span>
          {#if checkedLevel === level}<span class="intel-check" data-testid="intel-check"><Check class="w-4 h-4" /></span>{/if}
        </button>
      {/each}
    </div>
    <p class="intel-foot">{$t('modelPick.levelHint')}</p>

    <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mt-2">{$t('modelPick.pack')}</div>
    <div class="intel-card" role="radiogroup" aria-label={$t('modelPick.pack')}>
      {#if isCustom(choice)}{@render customRow()}{/if}
      {#each intel.packs as pack (pack.id)}
        <button
          type="button"
          class="intel-row"
          role="radio"
          aria-checked={currentPack === pack.id}
          disabled={loading || saving}
          onclick={() => pick(pack.id, isCustom(choice) ? null : choice.level)}
        >
          <span class="intel-text">
            <span class="intel-name">{packName(pack, $t)}</span>
            <span class="intel-desc">{pack.builtIn ? $t('modelPick.packBuiltIn') : $t('modelPick.packOwn')}</span>
          </span>
          {#if currentPack === pack.id}<span class="intel-check" data-testid="intel-check"><Check class="w-4 h-4" /></span>{/if}
        </button>
      {/each}
    </div>
    <p class="intel-foot">{$t('modelPick.hint')}</p>
  </div>
{/if}
