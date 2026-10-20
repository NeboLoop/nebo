<!--
  ModelInfo — everything Nebo knows about one model, opened from a model row
  (Intelligence Packs, a connection's models, Browse models): what it is and
  who makes it, what it is good for, the level it fits, its abilities,
  context and output, speed, and what it costs — Nebo AI's retail prices per
  1M tokens with the request fee, or on the owner's own key the request fee
  alone (his provider bills the model).
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import X from 'lucide-svelte/icons/x';
  import type { ModelCard } from '$lib/api/neboComponents';
  import { bandLabel, cardBands, cardSpeed, cardTier, shortTokens, usd } from '$lib/models/modelCard';

  let { card, modelId, provider, onclose }: {
    card: ModelCard | undefined;
    modelId: string;
    provider: string;
    onclose: () => void;
  } = $props();

  const name = $derived(card?.displayName || modelId);
  const abilities = $derived(
    card
      ? (['vision', 'tools', 'reasoning', 'audio'] as const).filter((a) => card.abilities?.[a])
      : [],
  );
  const pricing = $derived(card?.pricing);
  const bands = $derived(cardBands(card));
  const speed = $derived(cardSpeed(card));

  // Focus moves into the card when it opens and back to what opened it when
  // it closes.
  let closeButton = $state<HTMLButtonElement | undefined>();
  $effect(() => {
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    closeButton?.focus();
    return () => opener?.focus();
  });
</script>

<svelte:window onkeydown={(e) => { if (e.key === 'Escape') onclose(); }} />

<div class="model-info-backdrop">
  <button type="button" class="model-info-scrim" onclick={onclose} aria-label={$t('common.close')}></button>
  <div class="model-info" role="dialog" aria-modal="true" aria-labelledby="model-info-title">
    <div class="model-info-head">
      <div class="min-w-0">
        <h3 id="model-info-title" class="model-info-name">{name}</h3>
        <p class="model-info-sub">{[provider, card?.maker, card?.family].filter(Boolean).join(' · ')}</p>
      </div>
      <button type="button" class="model-info-close" bind:this={closeButton} onclick={onclose} aria-label={$t('common.close')}><X class="w-4 h-4" /></button>
    </div>

    {#if card?.description}
      <p class="model-info-desc">{card.description}</p>
    {/if}

    <dl class="model-info-facts">
      {#if cardTier(card, $t)}
        <dt>{$t('modelCard.level')}</dt><dd>{cardTier(card, $t)}</dd>
      {/if}
      {#if card}
        <dt>{$t('modelCard.reasons')}</dt><dd>{card.reasons ? $t('modelCard.yes') : $t('modelCard.no')}</dd>
      {/if}
      <dt>{$t('modelCard.abilities')}</dt>
      <dd>{abilities.length ? abilities.map((a) => $t(`modelCard.ability.${a}`)).join(', ') : $t('modelCard.unknown')}</dd>
      {#if card?.contextWindow}
        <dt>{$t('modelCard.context')}</dt><dd>{$t('modelCard.tokens', { values: { count: shortTokens(card.contextWindow) } })}</dd>
      {/if}
      {#if card?.maxOutput}
        <dt>{$t('modelCard.maxOutput')}</dt><dd>{$t('modelCard.tokens', { values: { count: shortTokens(card.maxOutput) } })}</dd>
      {/if}
      {#if speed}
        <dt>{$t('modelCard.speed')}</dt><dd>{$t(`modelCard.speedValue.${speed}`)}</dd>
      {/if}
      <dt class="font-mono">{$t('modelCard.modelId')}</dt><dd class="font-mono">{modelId}</dd>
    </dl>

    <div class="model-info-price">
      <div class="model-info-price-title" title={pricing?.version ? $t('modelCard.priceVersion', { values: { version: pricing.version } }) : undefined}>{$t('modelCard.pricing')}</div>
      {#if !pricing}
        <p class="model-info-note">{$t('modelCard.noPrice')}</p>
      {:else if pricing.providerBills}
        <p class="model-info-note">
          {pricing.requestFee ? $t('modelCard.ownKeyPrice', { values: { fee: usd(pricing.requestFee) } }) : $t('modelCard.providerBills')}
        </p>
      {:else}
        {#each bands as band, i}
          {#if bandLabel(bands, i, $t)}<div class="model-info-band">{bandLabel(bands, i, $t)}</div>{/if}
          <dl class="model-info-facts">
            {#if band.inputPer1m}<dt>{$t('modelCard.input')}</dt><dd>{$t('modelCard.perMillion', { values: { price: usd(band.inputPer1m) } })}</dd>{/if}
            {#if band.outputPer1m}<dt>{$t('modelCard.output')}</dt><dd>{$t('modelCard.perMillion', { values: { price: usd(band.outputPer1m) } })}</dd>{/if}
            {#if band.cacheReadPer1m}<dt>{$t('modelCard.cacheRead')}</dt><dd>{$t('modelCard.perMillion', { values: { price: usd(band.cacheReadPer1m) } })}</dd>{/if}
            {#if band.cacheWritePer1m}<dt>{$t('modelCard.cacheWrite')}</dt><dd>{$t('modelCard.perMillion', { values: { price: usd(band.cacheWritePer1m) } })}</dd>{/if}
          </dl>
        {/each}
        {#if pricing.requestFee}
          <dl class="model-info-facts">
            <dt>{$t('modelCard.requestFee')}</dt><dd>{$t('modelCard.perRequest', { values: { price: usd(pricing.requestFee) } })}</dd>
          </dl>
        {/if}
      {/if}
    </div>
  </div>
</div>
