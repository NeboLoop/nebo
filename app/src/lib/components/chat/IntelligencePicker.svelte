<!--
  IntelligencePicker — what this conversation runs on, as two choices.

  Main view: ONE card of Auto and the five levels (one checkmark; a choice
  with no level is Auto), the footnote, then one "Intelligence pack ›" row
  that opens the pack list (one checkmark) with a link to build a pack.
  A raw model no pack names shows as one checked "Custom model" row in both
  cards. Choosing a level or a pack closes the picker once its checkmark
  shows; the row already in use just closes. The caller turns a pick into a model string
  (`valueFor`) and writes it.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import Check from 'lucide-svelte/icons/check';
  import X from 'lucide-svelte/icons/x';
  import ChevronLeft from 'lucide-svelte/icons/chevron-left';
  import ChevronRight from 'lucide-svelte/icons/chevron-right';
  import Zap from 'lucide-svelte/icons/zap';
  import Lightbulb from 'lucide-svelte/icons/lightbulb';
  import Scale from 'lucide-svelte/icons/scale';
  import Brain from 'lucide-svelte/icons/brain';
  import Rocket from 'lucide-svelte/icons/rocket';
  import Network from 'lucide-svelte/icons/network';
  import KeyRound from 'lucide-svelte/icons/key-round';
  import WandSparkles from 'lucide-svelte/icons/wand-sparkles';
  import Settings2 from 'lucide-svelte/icons/settings-2';
  import { withBase } from '$lib/nav';
  import type { PackChoice } from '$lib/api/neboComponents';
  import { LEVELS, NEBO_AI_ID, isCustom, levelOf, levelSubtitle, packName, packSubtitle, selectThenClose, shownLevel, type Choice, type Effort, type Level, type PackRow } from '$lib/models/speeds';

  let {
    packs,
    choices = [],
    choice,
    view = $bindable('level'),
    onpick,
    onclose,
  }: {
    packs: PackRow[];
    /** The models packs name: an owner's pack shows what each level runs on. */
    choices?: PackChoice[];
    choice: Choice;
    view?: 'level' | 'packs';
    onpick: (pack: string, level: Effort | null) => void;
    onclose: () => void;
  } = $props();

  const LEVEL_ICONS = { auto: WandSparkles, instant: Zap, low: Lightbulb, medium: Scale, high: Brain, max: Rocket } as const;

  const custom = $derived(isCustom(choice));
  const checkedLevel = $derived(shownLevel(choice));
  const currentPack = $derived(isCustom(choice) ? null : choice.pack);
  const currentPackRow = $derived(packs.find((p) => p.id === currentPack));

  // Choosing closes the picker: the checkmark moves, then it goes.
  function pickLevel(level: Level) {
    selectThenClose(choice, isCustom(choice) ? NEBO_AI_ID : choice.pack, levelOf(level), onpick, onclose);
  }

  // A pack keeps the level in use; Auto stays Auto.
  function pickPack(id: string) {
    selectThenClose(choice, id, isCustom(choice) ? null : choice.level, onpick, onclose);
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
      {#each LEVELS as level (level)}
        {@const Icon = LEVEL_ICONS[level]}
        {@const sub = levelSubtitle(currentPackRow, level, choices, $t)}
        <button type="button" class="intel-row" data-level={level} aria-pressed={checkedLevel === level} onclick={() => pickLevel(level)}>
          <Icon class="intel-icon" />
          <span class="intel-text">
            <span class="intel-name">
              {$t(`settingsPacks.level.${level}`)}
              {#if level === 'auto'}<span class="badge badge-sm badge-primary badge-soft">{$t('modelPick.recommended')}</span>{/if}
              {#if level === 'max'}<span class="badge badge-sm badge-warning badge-soft">{$t('modelPick.mostBalance')}</span>{/if}
            </span>
            <span class="intel-desc" class:intel-desc-muted={sub.muted}>{sub.text}</span>
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
      <h2 class="intel-title">{$t('modelPick.pack')}</h2>
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
      {#each packs as pack (pack.id)}
        <button type="button" class="intel-row" data-pack={pack.id} aria-pressed={currentPack === pack.id} onclick={() => pickPack(pack.id)}>
          {#if pack.builtIn}<Network class="intel-icon" />{:else}<KeyRound class="intel-icon" />{/if}
          <span class="intel-text">
            <span class="intel-name">{packName(pack, $t)}</span>
            <span class="intel-desc">{packSubtitle(pack, choices, $t)}</span>
          </span>
          {#if currentPack === pack.id}<span class="intel-check" data-testid="intel-check"><Check class="w-4 h-4" /></span>{/if}
        </button>
      {/each}
    </div>
    <a class="intel-link" href={withBase('/settings/intelligence-packs')} onclick={onclose}>{$t('modelPick.createPack')}</a>
  {/if}
</div>
