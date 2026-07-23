<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { t } from 'svelte-i18n';
  import { onWsEvent } from '$lib/websocket/subscribe';
  import { goto } from '$lib/nav';
  import Check from 'lucide-svelte/icons/check';
  import ArrowRight from 'lucide-svelte/icons/arrow-right';
  import ArrowLeft from 'lucide-svelte/icons/arrow-left';
  import Zap from 'lucide-svelte/icons/zap';
  import * as api from '$lib/api/nebo';
  import type {
    AccountStatusResponse,
    BillingPriceInfo,
    NeboAIBillingSubscriptionResponse,
    NeboAIBillingCheckoutResponse
  } from '$lib/api/neboComponents';
  import Spinner from '$lib/components/ui/Spinner.svelte';

  let isLoading = $state(true);
  let status = $state<AccountStatusResponse | null>(null);
  let allPrices = $state<BillingPriceInfo[]>([]);
  let subscription = $state<NeboAIBillingSubscriptionResponse | null>(null);
  let billingInterval = $state<'month' | 'year'>('month');
  let boostSelections = $state<Record<string, boolean>>({});

  let step = $state<'plans' | 'checkout'>('plans');
  let checkoutLoading = $state(false);
  let checkoutError = $state('');

  let selectedPrice = $state<BillingPriceInfo | null>(null);
  let selectedBoost = $state<BillingPriceInfo | null>(null);

  let embeddedCheckout = $state<any>(null);

  const currentPlan = $derived((subscription?.plan || status?.plan || 'free').toLowerCase());

  const visiblePrices = $derived(
    allPrices
      .filter((p) => (p.category ?? '') === 'personal' && p.interval === billingInterval)
      .sort((a, b) => (a.displayOrder ?? 0) - (b.displayOrder ?? 0))
  );
  const boostPrices = $derived(allPrices.filter((p) => (p.category ?? '') === 'boost'));
  const popularIndex = $derived(Math.floor(visiblePrices.length / 2));

  function getBoostPrice(id: string | undefined): BillingPriceInfo | undefined {
    if (!id) return undefined;
    return boostPrices.find((p) => p.id === id);
  }

  onMount(() => {
    (async () => {
      try {
        status = (await api.neboAIAccountStatus()) as AccountStatusResponse;
        if (status?.connected) {
          const [pricesResp, subResp] = await Promise.allSettled([
            api.neboAIBillingPrices(),
            api.neboAIBillingSubscription()
          ]);
          if (pricesResp.status === 'fulfilled') allPrices = pricesResp.value?.prices || [];
          if (subResp.status === 'fulfilled') subscription = subResp.value;
        }
      } catch { status = null; }
      finally { isLoading = false; }
    })();

  });

  onWsEvent<{ plan?: string }>('plan_changed', (d) => {
    if (d?.plan && status) status = { ...status, plan: d.plan };
  });

  onDestroy(() => {
    if (embeddedCheckout) {
      embeddedCheckout.destroy();
      embeddedCheckout = null;
    }
  });

  function fmt(cents: number, currency?: string): string {
    return new Intl.NumberFormat('en-US', { style: 'currency', currency: currency || 'usd', minimumFractionDigits: 0 }).format(cents / 100);
  }

  // With Double Up attached, the plan's "Nx the usage of Lite" claim doubles
  // (Plus 5x → 10x, Max 10x → 20x) to reflect the doubled usage envelope.
  function displayFeatures(features: string[], doubled: boolean): string[] {
    if (!doubled) return features;
    return features.map((f) => {
      const m = f.match(/^(\d+)x\b/);
      return m ? f.replace(/^\d+x/, `${parseInt(m[1], 10) * 2}x`) : f;
    });
  }

  async function selectPlan(price: BillingPriceInfo) {
    selectedPrice = price;
    selectedBoost = boostSelections[price.id ?? ''] ? getBoostPrice(price.boostPriceId) || null : null;
    step = 'checkout';
    checkoutLoading = true;
    checkoutError = '';

    try {
      if (!(window as any).Stripe) {
        await new Promise<void>((resolve, reject) => {
          const s = document.createElement('script');
          s.src = 'https://js.stripe.com/v3/';
          s.onload = () => resolve();
          s.onerror = () => reject(new Error($t('pricing.stripeLoadFailed')));
          document.head.appendChild(s);
        });
      }

      const priceIds = [price.stripePriceId ?? ''];
      if (selectedBoost) priceIds.push(selectedBoost.stripePriceId ?? '');

      const data: NeboAIBillingCheckoutResponse = await api.neboAIBillingCheckout({ priceIds, uiMode: 'embedded' });

      if (!data.clientSecret) {
        throw new Error('Missing clientSecret from checkout response');
      }

      const stripe = (window as any).Stripe(data.publishableKey);

      embeddedCheckout = await stripe.initEmbeddedCheckout({
        clientSecret: data.clientSecret,
        onComplete: () => {
          if (embeddedCheckout) {
            embeddedCheckout.destroy();
            embeddedCheckout = null;
          }
          goto('/');
        }
      });

      await new Promise(r => setTimeout(r, 50));
      const container = document.getElementById('stripe-checkout');
      if (container) {
        embeddedCheckout.mount(container);
      }
    } catch (e: any) {
      checkoutError = e?.message || $t('pricing.somethingWentWrong');
    } finally {
      checkoutLoading = false;
    }
  }

  function goBack() {
    if (embeddedCheckout) {
      embeddedCheckout.destroy();
      embeddedCheckout = null;
    }
    step = 'plans';
    selectedPrice = null;
    selectedBoost = null;
    checkoutError = '';
  }

  const includedFeatures = ['pricing.featureLocal', 'pricing.featureDataLocal', 'pricing.featureMarketplace', 'pricing.featureDesktop', 'pricing.featureMcp', 'pricing.featureMemory'];
</script>

<svelte:head><title>{$t('pricing.pageTitle')}</title></svelte:head>

{#if isLoading}
  <div class="flex items-center justify-center gap-3 py-24">
    <Spinner size={20} />
    <span class="text-sm text-base-content/70">{$t('pricing.loadingPlans')}</span>
  </div>
{:else if !status?.connected}
  <div class="text-center py-24">
    <h1 class="text-2xl font-bold text-base-content mb-2">{$t('pricing.connectNeboAI')}</h1>
    <p class="text-xs text-base-content/70 mb-6">{$t('pricing.connectDescription')}</p>
    <a href="/settings/account" class="inline-flex h-9 px-4 items-center rounded-xl bg-primary text-primary-content text-sm font-bold hover:brightness-110 transition-all">{$t('settingsUsage.goToAccount')}</a>
  </div>

{:else if step === 'plans'}
  <div class="space-y-8">
    <div class="text-center">
      <h1 class="text-3xl font-bold tracking-tight text-base-content">{$t('pricing.heading')}</h1>
      <p class="text-xs text-base-content/50 max-w-md mx-auto mt-2">{$t('pricing.subheading')}</p>
    </div>

    {#if visiblePrices.length > 0}
      <div class="grid sm:grid-cols-3 gap-5">
        {#each visiblePrices as price, i (price.id)}
          {@const boost = getBoostPrice(price.boostPriceId)}
          {@const boostChecked = boostSelections[price.id ?? ''] || false}
          {@const isPopular = i === popularIndex}
          {@const isCurrent = price.nickname === currentPlan}

          <div class="relative rounded-2xl border p-6 flex flex-col transition-all {isPopular ? 'bg-primary/5 border-primary/30 ring-1 ring-primary/20 scale-[1.02]' : 'bg-base-200/50 border-base-content/10 hover:border-base-content/20'}">
            {#if isPopular}
              <div class="absolute -top-3 left-1/2 -translate-x-1/2">
                <span class="px-3 py-1 rounded-full bg-primary text-primary-content text-xs font-bold shadow-sm">{$t('pricing.mostPopular')}</span>
              </div>
            {/if}
            {#if isCurrent}
              <div class="absolute -top-3 right-4">
                <span class="px-3 py-1 rounded-full bg-base-content/10 text-base-content/60 text-xs font-bold">{$t('pricing.current')}</span>
              </div>
            {/if}

            <h3 class="text-xl font-bold text-base-content {isPopular ? 'mt-1' : ''}">{price.displayName || price.nickname}</h3>
            {#if price.description}
              <p class="text-xs text-base-content/50 mt-1">{price.description}</p>
            {/if}

            <div class="mt-5 mb-5">
              {#if price.interval === 'year'}
                <span class="text-4xl font-bold text-base-content tracking-tight">{fmt(Math.round((price.amountCents ?? 0) / 12), price.currency)}</span>
                <span class="text-sm text-base-content/40 ml-1">{$t('pricing.perMonth')}</span>
                <p class="text-xs text-base-content/40 mt-1">{$t('pricing.billedAnnually', { values: { amount: fmt(price.amountCents ?? 0, price.currency) } })}</p>
              {:else}
                <span class="text-4xl font-bold text-base-content tracking-tight">{fmt(price.amountCents ?? 0, price.currency)}</span>
                <span class="text-sm text-base-content/40 ml-1">{$t('pricing.perMonth')}</span>
              {/if}
            </div>

            {#if price.features && price.features.length > 0}
              <ul class="space-y-2.5 mb-5 flex-1">
                {#each displayFeatures(price.features, boostChecked) as feature}
                  <li class="flex items-start gap-2 text-sm text-base-content/70">
                    <Check class="w-4 h-4 shrink-0 mt-0.5 {isPopular ? 'text-primary' : 'text-base-content/30'}" />
                    {feature}
                  </li>
                {/each}
              </ul>
            {:else}<div class="flex-1"></div>{/if}

            {#if boost}
              <label class="flex items-start gap-2.5 mb-5 p-3 rounded-xl border cursor-pointer select-none group transition-all {boostChecked ? 'bg-accent/10 border-accent/30' : 'bg-base-content/3 border-transparent hover:border-base-content/10'}">
                <input type="checkbox" class="checkbox checkbox-sm checkbox-warning mt-0.5" checked={boostChecked} onchange={() => (boostSelections[price.id ?? ''] = !boostChecked)} />
                <div class="flex-1">
                  <div class="flex items-center gap-1.5">
                    <Zap class="w-3.5 h-3.5 text-accent" />
                    <span class="text-xs font-bold text-base-content uppercase tracking-wide">{$t('pricing.doubleUp')}</span>
                  </div>
                  <p class="text-xs text-base-content/50 mt-1">{$t('pricing.doubleUpDesc')}</p>
                  <p class="text-xs font-bold text-accent mt-1">
                    {$t('pricing.boostPerMonth', { values: { amount: fmt(boost.amountCents ?? 0, boost.currency) } })}
                  </p>
                </div>
              </label>
            {/if}

            {#if isCurrent}
              <button disabled class="w-full h-11 rounded-xl text-sm font-bold bg-base-content/10 text-base-content/40 cursor-not-allowed">{$t('pricing.currentPlan')}</button>
            {:else}
              <button
                onclick={() => selectPlan(price)}
                class="w-full h-11 flex items-center justify-center gap-2 rounded-xl text-sm font-bold transition-all mt-auto cursor-pointer border-none {isPopular ? 'bg-primary text-primary-content hover:brightness-110 shadow-md shadow-primary/20' : 'bg-primary text-primary-content hover:brightness-110'}"
              >
                {$t('pricing.getStarted')} <ArrowRight class="w-4 h-4" />
              </button>
            {/if}
          </div>
        {/each}
      </div>
    {/if}

    <section class="pt-4 pb-6">
      <div class="rounded-2xl bg-base-200/30 border border-base-content/5 p-6">
        <h2 class="text-xs font-bold text-base-content/30 uppercase tracking-widest mb-4">{$t('pricing.everyPlanIncludes')}</h2>
        <div class="grid grid-cols-2 sm:grid-cols-3 gap-3">
          {#each includedFeatures as feature}
            <div class="flex items-center gap-2 text-xs text-base-content/50">
              <Check class="w-4 h-4 text-primary/60 shrink-0" />
              {$t(feature)}
            </div>
          {/each}
        </div>
      </div>
    </section>
  </div>

{:else if step === 'checkout'}
  <div class="max-w-2xl mx-auto space-y-4">
    <button
      onclick={goBack}
      class="flex items-center gap-2 text-xs text-base-content/50 hover:text-base-content transition-colors cursor-pointer bg-transparent border-none"
    >
      <ArrowLeft class="w-4 h-4" /> {$t('pricing.backToPlans')}
    </button>

    {#if checkoutError}
      <div class="rounded-xl bg-error/10 border border-error/20 p-3">
        <p class="text-sm text-error">{checkoutError}</p>
      </div>
    {/if}

    {#if checkoutLoading}
      <div class="flex items-center justify-center gap-3 py-24">
        <Spinner size={20} />
        <span class="text-xs text-base-content/70">{$t('pricing.loadingCheckout')}</span>
      </div>
    {/if}

    <div id="stripe-checkout"></div>
  </div>
{/if}
