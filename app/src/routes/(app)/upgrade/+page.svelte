<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { Check, Zap, ArrowRight, ArrowLeft } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import type {
		NeboLoopAccountStatusResponse,
		BillingPriceInfo
	} from '$lib/api/neboComponents';
	import Spinner from '$lib/components/ui/Spinner.svelte';

	// Page state
	let isLoading = $state(true);
	let status = $state<NeboLoopAccountStatusResponse | null>(null);
	let allPrices = $state<BillingPriceInfo[]>([]);
	let subscription = $state<{ plan: string; subscriptions: any[] } | null>(null);
	let billingInterval = $state<'month' | 'year'>('month');
	let boostSelections = $state<Record<string, boolean>>({});

	// Checkout flow: 'plans' | 'checkout' | 'success'
	let step = $state<'plans' | 'checkout' | 'success'>('plans');
	let checkoutLoading = $state(false);
	let checkoutError = $state('');

	// Selected plan
	let selectedPrice = $state<BillingPriceInfo | null>(null);
	let selectedBoost = $state<BillingPriceInfo | null>(null);

	// Stripe Embedded Checkout instance
	let embeddedCheckout = $state<any>(null);

	const currentPlan = $derived((subscription?.plan || status?.plan || 'free').toLowerCase());

	// Personal prices only, filtered by billing interval
	const visiblePrices = $derived(
		allPrices
			.filter((p) => p.category === 'personal' && p.interval === billingInterval)
			.sort((a, b) => a.displayOrder - b.displayOrder)
	);
	const boostPrices = $derived(allPrices.filter((p) => p.category === 'boost'));
	const popularIndex = $derived(Math.floor(visiblePrices.length / 2));

	function getBoostPrice(id: string | undefined): BillingPriceInfo | undefined {
		if (!id) return undefined;
		return boostPrices.find((p) => p.id === id);
	}

	onMount(async () => {
		try {
			status = await api.neboLoopAccountStatus();
			if (status?.connected) {
				const [pricesResp, subResp] = await Promise.allSettled([
					api.neboLoopBillingPrices(),
					api.neboLoopBillingSubscription()
				]);
				if (pricesResp.status === 'fulfilled') allPrices = pricesResp.value?.prices || [];
				if (subResp.status === 'fulfilled') subscription = subResp.value;
			}
		} catch { status = null; }
		finally { isLoading = false; }

		const handler = (e: Event) => {
			const detail = (e as CustomEvent).detail;
			if (detail?.plan && status) status = { ...status, plan: detail.plan };
		};
		window.addEventListener('nebo:plan_changed', handler);
		return () => window.removeEventListener('nebo:plan_changed', handler);
	});

	onDestroy(() => {
		if (embeddedCheckout) {
			embeddedCheckout.destroy();
			embeddedCheckout = null;
		}
	});

	function fmt(cents: number, currency = 'usd'): string {
		return new Intl.NumberFormat('en-US', { style: 'currency', currency, minimumFractionDigits: 0 }).format(cents / 100);
	}

	// Select plan → mount Stripe Embedded Checkout
	async function selectPlan(price: BillingPriceInfo) {
		selectedPrice = price;
		selectedBoost = boostSelections[price.id] ? getBoostPrice(price.boostPriceId) || null : null;
		step = 'checkout';
		checkoutLoading = true;
		checkoutError = '';

		try {
			// Load Stripe.js if needed
			if (!(window as any).Stripe) {
				await new Promise<void>((resolve, reject) => {
					const s = document.createElement('script');
					s.src = 'https://js.stripe.com/v3/';
					s.onload = () => resolve();
					s.onerror = () => reject(new Error('Failed to load Stripe'));
					document.head.appendChild(s);
				});
			}

			const priceIds = [price.stripePriceId];
			if (selectedBoost) priceIds.push(selectedBoost.stripePriceId);

			// Create embedded checkout session
			const resp = await fetch('/api/v1/neboloop/billing/checkout', {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				credentials: 'include',
				body: JSON.stringify({ priceIds, uiMode: 'embedded' })
			});

			if (!resp.ok) {
				const err = await resp.json().catch(() => ({}));
				throw new Error(err.error || 'Failed to create checkout session');
			}

			const data = await resp.json();
			console.log('[upgrade] checkout response:', JSON.stringify(data));

			if (!data.clientSecret) {
				throw new Error('The NeboLoop backend needs to support ui_mode: "embedded" on the checkout endpoint. Got: ' + JSON.stringify(Object.keys(data)));
			}

			const stripe = (window as any).Stripe(data.publishableKey);

			// Mount embedded checkout — Stripe handles Link, address, tax, payment
			embeddedCheckout = await stripe.initEmbeddedCheckout({
				clientSecret: data.clientSecret,
				onComplete: () => {
					step = 'success';
					setTimeout(() => { window.location.href = '/settings/billing?success=true'; }, 2500);
				}
			});

			// Wait for DOM update then mount
			await new Promise(r => setTimeout(r, 50));
			const container = document.getElementById('stripe-checkout');
			if (container) {
				embeddedCheckout.mount(container);
			}
		} catch (e: any) {
			checkoutError = e?.message || 'Something went wrong.';
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

	const includedFeatures = ['Runs on your machine', 'Your data stays local', 'Skills & roles marketplace', 'Desktop automation', 'MCP integrations', 'Memory system'];
</script>

<svelte:head><title>Choose your plan - Nebo</title></svelte:head>

{#if isLoading}
	<div class="flex items-center justify-center gap-3 py-24">
		<Spinner size={20} />
		<span class="text-base text-base-content/80">Loading plans...</span>
	</div>
{:else if !status?.connected}
	<div class="text-center py-24">
		<h1 class="font-display text-2xl font-bold text-base-content mb-2">Connect NeboLoop</h1>
		<p class="text-base text-base-content/80 mb-6">Connect your NeboLoop account to view plans and upgrade.</p>
		<a href="/settings/account" class="inline-flex h-9 px-4 items-center rounded-xl bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all">Go to Account</a>
	</div>

<!-- ═══════════════════════════════════════════════════════════ -->
<!-- STEP 1: CHOOSE PLAN -->
<!-- ═══════════════════════════════════════════════════════════ -->
{:else if step === 'plans'}
	<div class="space-y-8">
		<div class="text-center">
			<h1 class="font-display text-3xl font-bold text-base-content">Plans that grow with you</h1>
			<p class="text-base text-base-content/50 max-w-md mx-auto mt-2">AI that runs on your machine. Pick a plan, get instant access.</p>
		</div>

		<div class="flex justify-center">
			<div class="inline-flex rounded-full bg-base-200/80 p-1">
				<button onclick={() => (billingInterval = 'month')} class="px-6 py-2 rounded-full text-sm font-semibold transition-all {billingInterval === 'month' ? 'bg-base-100 text-base-content shadow-sm' : 'text-base-content/40 hover:text-base-content/60'}">Monthly</button>
				<button onclick={() => (billingInterval = 'year')} class="px-6 py-2 rounded-full text-sm font-semibold transition-all {billingInterval === 'year' ? 'bg-base-100 text-base-content shadow-sm' : 'text-base-content/40 hover:text-base-content/60'}">
					Annual
					<span class="ml-1 text-xs font-bold text-green-600">Save 17%</span>
				</button>
			</div>
		</div>

		{#if visiblePrices.length > 0}
			<div class="grid sm:grid-cols-3 gap-5">
				{#each visiblePrices as price, i (price.id)}
					{@const boost = getBoostPrice(price.boostPriceId)}
					{@const boostChecked = boostSelections[price.id] || false}
					{@const isPopular = i === popularIndex}
					{@const isCurrent = price.nickname === currentPlan}

					<div class="relative rounded-2xl border p-6 flex flex-col transition-all {isPopular ? 'bg-primary/5 border-primary/30 ring-1 ring-primary/20 scale-[1.02]' : 'bg-base-200/50 border-base-content/10 hover:border-base-content/20'}">
						{#if isPopular}<div class="absolute -top-3 left-1/2 -translate-x-1/2"><span class="px-3 py-1 rounded-full bg-primary text-primary-content text-xs font-bold shadow-sm">Most popular</span></div>{/if}
						{#if isCurrent}<div class="absolute -top-3 right-4"><span class="px-3 py-1 rounded-full bg-base-content/10 text-base-content/60 text-xs font-bold">Current</span></div>{/if}

						<h3 class="text-xl font-bold text-base-content {isPopular ? 'mt-1' : ''}">{price.displayName || price.nickname}</h3>
						{#if price.description}<p class="text-sm text-base-content/50 mt-1">{price.description}</p>{/if}

						<div class="mt-5 mb-5">
							{#if price.interval === 'year'}
								<span class="text-4xl font-bold text-base-content tracking-tight">{fmt(Math.round(price.amountCents / 12), price.currency)}</span>
								<span class="text-sm text-base-content/40 ml-1">/mo</span>
								<p class="text-xs text-base-content/40 mt-1">{fmt(price.amountCents, price.currency)} billed annually</p>
							{:else}
								<span class="text-4xl font-bold text-base-content tracking-tight">{fmt(price.amountCents, price.currency)}</span>
								<span class="text-sm text-base-content/40 ml-1">/mo</span>
							{/if}
						</div>

						{#if price.features && price.features.length > 0}
							<ul class="space-y-2.5 mb-5 flex-1">
								{#each price.features as feature}
									<li class="flex items-start gap-2 text-sm text-base-content/70">
										<Check class="w-4 h-4 shrink-0 mt-0.5 {isPopular ? 'text-primary' : 'text-base-content/30'}" />
										{feature}
									</li>
								{/each}
							</ul>
						{:else}<div class="flex-1"></div>{/if}

						{#if boost}
							<label class="flex items-start gap-2.5 mb-5 p-3 rounded-xl border cursor-pointer select-none group transition-all {boostChecked ? 'bg-amber-500/10 border-amber-500/30' : 'bg-base-content/3 border-transparent hover:border-base-content/10'}">
								<input type="checkbox" class="checkbox checkbox-sm checkbox-warning mt-0.5" checked={boostChecked} onchange={() => (boostSelections[price.id] = !boostChecked)} />
								<div class="flex-1">
									<div class="flex items-center gap-1.5">
										<Zap class="w-3.5 h-3.5 text-amber-500" />
										<span class="text-xs font-bold text-base-content uppercase tracking-wide">Advanced Compute</span>
									</div>
									<p class="text-xs text-base-content/50 mt-1">{boost.description || '3x access to frontier models.'}</p>
									<p class="text-xs font-bold text-amber-600 mt-1">
										{#if boost.interval === 'year'}
											+{fmt(Math.round(boost.amountCents / 12), boost.currency)}/mo ({fmt(boost.amountCents, boost.currency)}/yr)
										{:else}
											+{fmt(boost.amountCents, boost.currency)}/mo
										{/if}
									</p>
								</div>
							</label>
						{/if}

						{#if isCurrent}
							<button disabled class="w-full h-11 rounded-xl text-sm font-bold bg-base-content/10 text-base-content/40 cursor-not-allowed">Current plan</button>
						{:else}
							<button onclick={() => selectPlan(price)} class="w-full h-11 flex items-center justify-center gap-2 rounded-xl text-sm font-bold transition-all mt-auto {isPopular ? 'bg-primary text-primary-content hover:brightness-110 shadow-md shadow-primary/20' : 'bg-primary text-primary-content hover:brightness-110'}">
								Get started <ArrowRight class="w-4 h-4" />
							</button>
						{/if}
					</div>
				{/each}
			</div>
		{/if}

		<section class="pt-4 pb-6">
			<div class="rounded-2xl bg-base-200/30 border border-base-content/5 p-6">
				<h2 class="text-xs font-bold text-base-content/30 uppercase tracking-widest mb-4">Every plan includes</h2>
				<div class="grid grid-cols-2 sm:grid-cols-3 gap-3">
					{#each includedFeatures as feature}
						<div class="flex items-center gap-2 text-sm text-base-content/60"><Check class="w-4 h-4 text-primary/60 shrink-0" />{feature}</div>
					{/each}
				</div>
			</div>
		</section>
	</div>

<!-- ═══════════════════════════════════════════════════════════ -->
<!-- STEP 2: STRIPE EMBEDDED CHECKOUT -->
<!-- ═══════════════════════════════════════════════════════════ -->
{:else if step === 'checkout'}
	<div class="max-w-2xl mx-auto space-y-4">
		<button onclick={goBack} class="flex items-center gap-2 text-sm text-base-content/50 hover:text-base-content transition-colors">
			<ArrowLeft class="w-4 h-4" /> Back to plans
		</button>

		{#if checkoutError}
			<div class="rounded-xl bg-error/10 border border-error/20 p-3">
				<p class="text-sm text-error">{checkoutError}</p>
			</div>
		{/if}

		{#if checkoutLoading}
			<div class="flex items-center justify-center gap-3 py-24">
				<Spinner size={20} />
				<span class="text-base text-base-content/80">Loading checkout...</span>
			</div>
		{/if}

		<!-- Stripe mounts its full checkout experience here: email, Link, address, tax, payment -->
		<div id="stripe-checkout"></div>
	</div>

<!-- ═══════════════════════════════════════════════════════════ -->
<!-- STEP 3: SUCCESS -->
<!-- ═══════════════════════════════════════════════════════════ -->
{:else if step === 'success'}
	<div class="max-w-lg mx-auto text-center py-16 space-y-4">
		<div class="w-20 h-20 mx-auto rounded-full bg-green-100 flex items-center justify-center">
			<Check class="w-10 h-10 text-green-600" />
		</div>
		<h1 class="font-display text-2xl font-bold text-base-content">You're all set!</h1>
		<p class="text-base text-base-content/60">Your {selectedPrice?.displayName} plan is now active. Redirecting to your account...</p>
		<Spinner size={20} />
	</div>
{/if}
