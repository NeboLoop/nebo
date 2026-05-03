<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import Check from 'lucide-svelte/icons/check';
  import ArrowRight from 'lucide-svelte/icons/arrow-right';
  import ArrowLeft from 'lucide-svelte/icons/arrow-left';
  import Zap from 'lucide-svelte/icons/zap';

  interface Plan { id: string; name: string; price: number | null; priceYearly: number | null; features: string[]; current?: boolean; popular?: boolean; description: string }

  let plans = $state<Plan[]>([]);
  let billingInterval = $state<'month' | 'year'>('month');
  let step = $state<'plans' | 'checkout'>('plans');
  let selectedPlan = $state<Plan | null>(null);
  let checkoutLoading = $state(false);

  const currentPlan = 'pro';

  const includedFeatures = [
    'Runs on your machine',
    'Your data stays local',
    'Skills & roles marketplace',
    'Desktop automation',
    'MCP integrations',
    'Memory system'
  ];

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const res = await api.billingPrices() as Record<string, unknown> | null;
      const prices = res?.prices as Record<string, unknown>[] | undefined;
      if (prices?.length) {
        plans = prices.map((p: Record<string, unknown>) => ({
          id: String(p.id || p.slug || ''),
          name: String(p.name || ''),
          price: Number(p.priceMonthly ?? p.price ?? 0),
          priceYearly: Number(p.priceYearly ?? p.priceAnnual ?? 0),
          features: (p.features as string[]) ?? [],
          current: !!(p.current ?? false),
          popular: !!(p.popular ?? false),
          description: String(p.description ?? ''),
        }));
      }
    } catch { /* keep mock data */ }
  });

  async function selectPlan(plan: Plan) {
    selectedPlan = plan;
    step = 'checkout';
    checkoutLoading = true;
    try {
      const api = await import('$lib/api/nebo');
      const res = await api.billingCheckout({ priceIds: [plan.id], mode: 'embedded' }) as Record<string, unknown> | null;
      if (res?.url && typeof res.url === 'string') {
        window.location.href = res.url;
        return;
      }
    } catch { /* fall through to placeholder */ }
    checkoutLoading = false;
  }

  function goBack() {
    step = 'plans';
    selectedPlan = null;
  }
</script>

<svelte:head><title>Choose your plan - Nebo</title></svelte:head>

{#if step === 'plans'}
  <div class="space-y-8">
    <div class="text-center">
      <h1 class="text-3xl font-bold tracking-tight text-base-content">Plans that grow with you</h1>
      <p class="text-xs text-base-content/50 max-w-md mx-auto mt-2">AI that runs on your machine. Pick a plan, get instant access.</p>
    </div>

    <!-- Billing interval toggle -->
    <div class="flex justify-center">
      <div class="inline-flex rounded-full bg-base-200/80 p-1">
        <button
          onclick={() => (billingInterval = 'month')}
          class="px-6 py-2 rounded-full text-sm font-semibold transition-all cursor-pointer bg-transparent border-none {billingInterval === 'month' ? 'bg-base-100 text-base-content shadow-sm' : 'text-base-content/40 hover:text-base-content/60'}"
        >
          Monthly
        </button>
        <button
          onclick={() => (billingInterval = 'year')}
          class="px-6 py-2 rounded-full text-sm font-semibold transition-all cursor-pointer bg-transparent border-none {billingInterval === 'year' ? 'bg-base-100 text-base-content shadow-sm' : 'text-base-content/40 hover:text-base-content/60'}"
        >
          Annual
          <span class="ml-1 text-sm font-bold text-success">Save 17%</span>
        </button>
      </div>
    </div>

    <!-- Plan cards -->
    <div class="grid sm:grid-cols-3 gap-5">
      {#each plans.filter(p => p.id !== 'enterprise') as plan, i}
        {@const isCurrent = plan.id === currentPlan}
        {@const isPopular = plan.popular}
        {@const price = billingInterval === 'year' ? plan.priceYearly : plan.price}

        <div class="relative rounded-2xl border p-6 flex flex-col transition-all {isPopular ? 'bg-primary/5 border-primary/30 ring-1 ring-primary/20 scale-[1.02]' : 'bg-base-200/50 border-base-content/10 hover:border-base-content/20'}">
          {#if isPopular}
            <div class="absolute -top-3 left-1/2 -translate-x-1/2">
              <span class="px-3 py-1 rounded-full bg-primary text-primary-content text-sm font-bold shadow-sm">Most popular</span>
            </div>
          {/if}
          {#if isCurrent}
            <div class="absolute -top-3 right-4">
              <span class="px-3 py-1 rounded-full bg-base-content/10 text-base-content/60 text-sm font-bold">Current</span>
            </div>
          {/if}

          <h3 class="text-xl font-bold text-base-content {isPopular ? 'mt-1' : ''}">{plan.name}</h3>
          {#if plan.description}
            <p class="text-xs text-base-content/50 mt-1">{plan.description}</p>
          {/if}

          <div class="mt-5 mb-5">
            {#if price === 0}
              <span class="text-4xl font-bold text-base-content tracking-tight">Free</span>
            {:else if price !== null}
              <span class="text-4xl font-bold text-base-content tracking-tight">${price}</span>
              <span class="text-sm text-base-content/40 ml-1">/mo</span>
              {#if billingInterval === 'year' && plan.price}
                <p class="text-sm text-base-content/40 mt-1">${Math.round(plan.price * 12 * 0.83)} billed annually</p>
              {/if}
            {/if}
          </div>

          <ul class="space-y-2.5 mb-5 flex-1">
            {#each plan.features as feature}
              <li class="flex items-start gap-2 text-sm text-base-content/70">
                <Check class="w-4 h-4 shrink-0 mt-0.5 {isPopular ? 'text-primary' : 'text-base-content/30'}" />
                {feature}
              </li>
            {/each}
          </ul>

          {#if isCurrent}
            <button disabled class="w-full h-11 rounded-xl text-sm font-bold bg-base-content/10 text-base-content/40 cursor-not-allowed">
              Current plan
            </button>
          {:else}
            <button
              onclick={() => selectPlan(plan)}
              class="w-full h-11 flex items-center justify-center gap-2 rounded-xl text-sm font-bold transition-all mt-auto cursor-pointer border-none {isPopular ? 'bg-primary text-primary-content hover:brightness-110 shadow-md shadow-primary/20' : 'bg-primary text-primary-content hover:brightness-110'}"
            >
              Get started <ArrowRight class="w-4 h-4" />
            </button>
          {/if}
        </div>
      {/each}
    </div>

    <!-- Every plan includes -->
    <div class="rounded-2xl bg-base-200/30 border border-base-content/5 p-6">
      <h2 class="text-sm font-bold text-base-content/30 uppercase tracking-widest mb-4">Every plan includes</h2>
      <div class="grid grid-cols-2 sm:grid-cols-3 gap-3">
        {#each includedFeatures as feature}
          <div class="flex items-center gap-2 text-xs text-base-content/50">
            <Check class="w-4 h-4 text-primary/60 shrink-0" />
            {feature}
          </div>
        {/each}
      </div>
    </div>
  </div>

{:else if step === 'checkout'}
  <!-- Stripe Embedded Checkout step -->
  <div class="max-w-2xl mx-auto space-y-4">
    <button
      onclick={goBack}
      class="flex items-center gap-2 text-xs text-base-content/50 hover:text-base-content transition-colors cursor-pointer bg-transparent border-none"
    >
      <ArrowLeft class="w-4 h-4" /> Back to plans
    </button>

    {#if checkoutLoading}
      <div class="flex items-center justify-center gap-3 py-24">
        <span class="loading loading-spinner loading-md text-primary"></span>
        <span class="text-xs text-base-content/70">Loading checkout...</span>
      </div>
    {:else}
      <!-- Stripe mounts its full checkout experience here -->
      <div id="stripe-checkout" class="min-h-[400px] rounded-2xl border border-base-content/10 bg-base-200/50 flex items-center justify-center">
        <div class="text-center">
          <Zap class="w-8 h-8 text-primary mx-auto mb-3" />
          <p class="text-sm font-semibold text-base-content mb-1">Upgrade to {selectedPlan?.name}</p>
          <p class="text-xs text-base-content/50">Stripe Embedded Checkout mounts here when connected to the backend.</p>
        </div>
      </div>
    {/if}
  </div>
{/if}
