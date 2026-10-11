<!--
  IntelligencePicker — what this conversation runs on, as two choices.

  Main view: ONE card of the five levels (one checkmark; a choice with no
  level reads as Medium), the footnote, then one "Intelligence pack ›" row
  that opens the pack list (one checkmark) with a link to build a pack.
  A raw model no pack names shows as one checked "Custom model" row in both
  cards. Presentational only: the caller turns a pick into a model string
  (`valueFor`) and writes it.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import Check from 'lucide-svelte/icons/check';
  import X from 'lucide-svelte/icons/x';
  import ChevronLeft from 'lucide-svelte/icons/chevron-left';
  import ChevronRight from 'lucide-svelte/icons/chevron-right';
  import Zap from 'lucide-svelte/icons/zap';
  import Feather from 'lucide-svelte/icons/feather';
  import Gauge from 'lucide-svelte/icons/gauge';
  import Brain from 'lucide-svelte/icons/brain';
  import Sparkles from 'lucide-svelte/icons/sparkles';
  import Settings2 from 'lucide-svelte/icons/settings-2';
  import { withBase } from '$lib/nav';
  import { EFFORTS, NEBO_AI_ID, isCustom, packName, shownLevel, type Choice, type Effort, type PackRow } from '$lib/models/speeds';

  let {
    packs,
    choice,
    view = $bindable('level'),
    onpick,
    onclose,
  }: {
    packs: PackRow[];
    choice: Choice;
    view?: 'level' | 'packs';
    onpick: (pack: string, level: Effort | null) => void;
    onclose: () => void;
  } = $props();

  const LEVEL_ICONS = { instant: Zap, low: Feather, medium: Gauge, high: Brain, max: Sparkles } as const;

  const custom = $derived(isCustom(choice));
  const checkedLevel = $derived(shownLevel(choice));
  const currentPack = $derived(isCustom(choice) ? null : choice.pack);
  const currentPackRow = $derived(packs.find((p) => p.id === currentPack));

  function pickLevel(level: Effort) {
    onpick(isCustom(choice) ? NEBO_AI_ID : choice.pack, level);
  }

  // A pack keeps the level in use; none stays none.
  function pickPack(id: string) {
    onpick(id, isCustom(choice) ? null : choice.level);
    view = 'level';
  }
</script>

<svelte:window onkeydown={(e) => { if (e.key === 'Escape') onclose(); }} />

<div class="intel-picker" role="dialog" aria-label={$t('modelPick.intelligence')}>
  {#if view === 'level'}
    <div class="intel-head">
      <button type="button" class="intel-round" aria-label={$t('common.close')} onclick={onclose}><X class="w-4 h-4" /></button>
      <h2 class="intel-title">{$t('modelPick.intelligence')}</h2>
      <span class="intel-round-spacer"></span>
    </div>

    <div class="intel-card">
      {#if custom}
        <div class="intel-row" data-testid="intel-custom">
          <Settings2 class="intel-icon" />
          <span class="intel-text">
            <span class="intel-name">{$t('modelPick.customModel')}</span>
            <span class="intel-desc">{$t('modelPick.customModelDesc')}</span>
          </span>
          <span class="intel-check" data-testid="intel-check"><Check class="w-4 h-4" /></span>
        </div>
      {/if}
      {#each EFFORTS as level (level)}
        {@const Icon = LEVEL_ICONS[level]}
        <button type="button" class="intel-row" data-level={level} aria-pressed={checkedLevel === level} onclick={() => pickLevel(level)}>
          <Icon class="intel-icon" />
          <span class="intel-text">
            <span class="intel-name">
              {$t(`settingsPacks.level.${level}`)}
              {#if level === 'medium'}<span class="badge badge-sm badge-primary badge-soft">{$t('modelPick.recommended')}</span>{/if}
              {#if level === 'max'}<span class="badge badge-sm badge-warning badge-soft">{$t('modelPick.mostCredits')}</span>{/if}
            </span>
            <span class="intel-desc">{$t(`modelPick.levelDesc.${level}`)}</span>
          </span>
          {#if checkedLevel === level}<span class="intel-check" data-testid="intel-check"><Check class="w-4 h-4" /></span>{/if}
        </button>
      {/each}
    </div>
    <p class="intel-foot">{$t('modelPick.levelHint')}</p>

    <button type="button" class="intel-card intel-drill" onclick={() => (view = 'packs')}>
      <span class="intel-name">{$t('modelPick.pack')}</span>
      <span class="intel-value">{currentPackRow ? packName(currentPackRow, $t) : $t('modelPick.custom')}</span>
      <ChevronRight class="w-4 h-4 text-base-content/50" />
    </button>
  {:else}
    <div class="intel-head">
      <button type="button" class="intel-round" aria-label={$t('common.back')} onclick={() => (view = 'level')}><ChevronLeft class="w-4 h-4" /></button>
      <h2 class="intel-title">{$t('modelPick.packsTitle')}</h2>
      <span class="intel-round-spacer"></span>
    </div>

    <div class="intel-card">
      {#if custom}
        <div class="intel-row" data-testid="intel-custom">
          <span class="intel-text">
            <span class="intel-name">{$t('modelPick.customModel')}</span>
            <span class="intel-desc">{$t('modelPick.customModelDesc')}</span>
          </span>
          <span class="intel-check" data-testid="intel-check"><Check class="w-4 h-4" /></span>
        </div>
      {/if}
      {#each packs as pack (pack.id)}
        <button type="button" class="intel-row" data-pack={pack.id} aria-pressed={currentPack === pack.id} onclick={() => pickPack(pack.id)}>
          <span class="intel-text">
            <span class="intel-name">{packName(pack, $t)}</span>
            <span class="intel-desc">{pack.builtIn ? $t('modelPick.packBuiltIn') : $t('modelPick.packOwn')}</span>
          </span>
          {#if currentPack === pack.id}<span class="intel-check" data-testid="intel-check"><Check class="w-4 h-4" /></span>{/if}
        </button>
      {/each}
    </div>
    <a class="intel-link" href={withBase('/settings/intelligence-packs')} onclick={onclose}>{$t('modelPick.createPack')}</a>
  {/if}
</div>
