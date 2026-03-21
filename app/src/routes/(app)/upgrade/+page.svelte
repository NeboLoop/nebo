<script lang="ts">
	import { onMount } from 'svelte';
	import { Check, Zap, ArrowRight, ArrowLeft, X as XIcon, CreditCard, ShieldCheck } from 'lucide-svelte';
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

	// Checkout flow: 'plans' | 'summary' | 'payment' | 'success'
	let step = $state<'plans' | 'summary' | 'payment' | 'success'>('plans');
	let checkoutLoading = $state(false);
	let checkoutError = $state('');

	// Selected plan for checkout
	let selectedPrice = $state<BillingPriceInfo | null>(null);
	let selectedBoost = $state<BillingPriceInfo | null>(null);

	// Stripe subscription data (from /billing/subscribe)
	let invoiceData = $state<any>(null);
	let stripeInstance = $state<any>(null);
	let elements = $state<any>(null);
	let paymentElement = $state<any>(null);
	let paymentMounted = $state(false);
	let paymentLoading = $state(false);
	let paymentError = $state('');

	const currentPlan = $derived((subscription?.plan || status?.plan || 'free').toLowerCase());

	// Personal prices only, filtered by billing interval
	const visiblePrices = $derived(
		allPrices
			.filter((p) => p.category === 'personal' && p.interval === billingInterval)
			.sort((a, b) => a.displayOrder - b.displayOrder)
	);
	const boostPrices = $derived(allPrices.filter((p) => p.category === 'boost'));
	const popularIndex = $derived(Math.floor(visiblePrices.length / 2));

	// Annual savings helper
	function monthlySavings(annualCents: number, monthlyNickname: string): number {
		const monthly = allPrices.find(p => p.nickname === monthlyNickname && p.interval === 'month');
		if (!monthly) return 0;
		return (monthly.amountCents * 12) - annualCents;
	}

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

	function fmt(cents: number, currency = 'usd'): string {
		return new Intl.NumberFormat('en-US', { style: 'currency', currency, minimumFractionDigits: 0 }).format(cents / 100);
	}

	// Step 1 → Step 2: Select plan, show summary
	function selectPlan(price: BillingPriceInfo) {
		selectedPrice = price;
		selectedBoost = boostSelections[price.id] ? getBoostPrice(price.boostPriceId) || null : null;
		step = 'summary';
		checkoutError = '';
	}

	// Step 2 → Step 3: Create subscription, show payment
	async function proceedToPayment() {
		if (!selectedPrice) return;
		checkoutLoading = true;
		checkoutError = '';

		try {
			const priceIds = [selectedPrice.stripePriceId];
			if (selectedBoost) priceIds.push(selectedBoost.stripePriceId);

			const resp = await fetch('/api/v1/neboloop/billing/subscribe', {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				credentials: 'include',
				body: JSON.stringify({ priceIds })
			});

			if (!resp.ok) {
				const err = await resp.json().catch(() => ({}));
				throw new Error(err.error || 'Failed to create subscription');
			}

			invoiceData = await resp.json();

			if (!invoiceData.clientSecret) {
				throw new Error('Payment setup failed — no client secret returned');
			}

			// Load Stripe.js
			if (!(window as any).Stripe) {
				await new Promise<void>((resolve) => {
					const s = document.createElement('script');
					s.src = 'https://js.stripe.com/v3/';
					s.onload = () => resolve();
					document.head.appendChild(s);
				});
			}
			stripeInstance = (window as any).Stripe(invoiceData.publishableKey);
			elements = stripeInstance.elements({
				clientSecret: invoiceData.clientSecret,
				appearance: { theme: 'flat' }
			});

			step = 'payment';

			// Mount after DOM update
			await new Promise(r => setTimeout(r, 50));
			const container = document.getElementById('payment-element');
			if (container) {
				paymentElement = elements.create('payment');
				paymentElement.mount(container);
				paymentMounted = true;
			}
		} catch (e: any) {
			checkoutError = e?.message || 'Something went wrong.';
		} finally {
			checkoutLoading = false;
		}
	}

	// Step 3 → Step 4: Confirm payment
	async function confirmPayment() {
		if (!stripeInstance || !elements) return;
		paymentLoading = true;
		paymentError = '';

		const { error } = await stripeInstance.confirmPayment({
			elements,
			confirmParams: { return_url: window.location.origin + '/settings/billing?success=true' },
			redirect: 'if_required'
		});

		if (error) {
			paymentError = error.message || 'Payment failed.';
			paymentLoading = false;
		} else {
			step = 'success';
			paymentLoading = false;
			setTimeout(() => { window.location.href = '/settings/billing?success=true'; }, 2500);
		}
	}

	function goBack() {
		if (step === 'summary') { step = 'plans'; selectedPrice = null; selectedBoost = null; }
		else if (step === 'payment') { step = 'summary'; if (paymentElement) { paymentElement.unmount(); paymentElement = null; } paymentMounted = false; }
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
<!-- STEP 2: ORDER SUMMARY -->
<!-- ═══════════════════════════════════════════════════════════ -->
{:else if step === 'summary' && selectedPrice}
	<div class="max-w-lg mx-auto space-y-6">
		<button onclick={goBack} class="flex items-center gap-2 text-sm text-base-content/50 hover:text-base-content transition-colors">
			<ArrowLeft class="w-4 h-4" /> Back to plans
		</button>

		<h1 class="font-display text-2xl font-bold text-base-content">Your order</h1>

		<div class="rounded-2xl border border-base-content/10 overflow-hidden">
			<!-- Line items -->
			<div class="p-5 space-y-4">
				<div class="flex items-center justify-between">
					<div>
						<p class="font-semibold text-base-content">{selectedPrice.displayName}</p>
						<p class="text-sm text-base-content/50">Billed {selectedPrice.interval === 'year' ? 'annually' : 'monthly'}</p>
					</div>
					<p class="font-semibold text-base-content">
						{#if selectedPrice.interval === 'year'}
							{fmt(selectedPrice.amountCents, selectedPrice.currency)}/yr
						{:else}
							{fmt(selectedPrice.amountCents, selectedPrice.currency)}/mo
						{/if}
					</p>
				</div>

				{#if selectedBoost}
					<div class="flex items-center justify-between">
						<div class="flex items-center gap-2">
							<Zap class="w-4 h-4 text-amber-500" />
							<div>
								<p class="font-semibold text-base-content">Advanced Compute</p>
								<p class="text-xs text-base-content/50">3x frontier model access</p>
							</div>
						</div>
						<p class="font-semibold text-base-content">
							{#if selectedBoost.interval === 'year'}
								+{fmt(selectedBoost.amountCents, selectedBoost.currency)}/yr
							{:else}
								+{fmt(selectedBoost.amountCents, selectedBoost.currency)}/mo
							{/if}
						</p>
					</div>
				{/if}
			</div>

			<!-- Totals -->
			<div class="border-t border-base-content/10 p-5 bg-base-200/30 space-y-2">
				<div class="flex justify-between text-sm text-base-content/60">
					<span>Subtotal</span>
					<span>{fmt((selectedPrice?.amountCents || 0) + (selectedBoost?.amountCents || 0), selectedPrice.currency)}</span>
				</div>
				<div class="flex justify-between text-sm text-base-content/60">
					<span>Tax</span>
					<span class="text-base-content/40">Calculated at payment</span>
				</div>
				<div class="flex justify-between text-base font-bold text-base-content pt-2 border-t border-base-content/10">
					<span>Due today</span>
					<span>{fmt((selectedPrice?.amountCents || 0) + (selectedBoost?.amountCents || 0), selectedPrice.currency)}{selectedPrice.interval === 'year' ? '' : '/mo'}</span>
				</div>
			</div>
		</div>

		{#if checkoutError}
			<div class="rounded-xl bg-error/10 border border-error/20 p-3">
				<p class="text-sm text-error">{checkoutError}</p>
			</div>
		{/if}

		<button
			disabled={checkoutLoading}
			onclick={proceedToPayment}
			class="w-full h-12 flex items-center justify-center gap-2 rounded-xl text-sm font-bold bg-primary text-primary-content hover:brightness-110 transition-all disabled:opacity-50"
		>
			{#if checkoutLoading}
				<Spinner size={16} /> Setting up payment...
			{:else}
				Continue to payment <ArrowRight class="w-4 h-4" />
			{/if}
		</button>

		<p class="text-xs text-base-content/30 text-center">Cancel anytime. No commitments.</p>
	</div>

<!-- ═══════════════════════════════════════════════════════════ -->
<!-- STEP 3: PAYMENT -->
<!-- ═══════════════════════════════════════════════════════════ -->
{:else if step === 'payment' && selectedPrice}
	<div class="max-w-lg mx-auto space-y-6">
		<button onclick={goBack} class="flex items-center gap-2 text-sm text-base-content/50 hover:text-base-content transition-colors">
			<ArrowLeft class="w-4 h-4" /> Back to summary
		</button>

		<h1 class="font-display text-2xl font-bold text-base-content">Payment details</h1>

		<!-- Compact order recap -->
		<div class="rounded-xl bg-base-200/30 border border-base-content/5 p-4 flex items-center justify-between">
			<div>
				<p class="text-sm font-semibold text-base-content">{selectedPrice.displayName}{selectedBoost ? ' + Advanced Compute' : ''}</p>
				<p class="text-xs text-base-content/50">Billed monthly</p>
			</div>
			<p class="text-lg font-bold text-base-content">
				{fmt((invoiceData?.total || (selectedPrice.amountCents + (selectedBoost?.amountCents || 0))), selectedPrice.currency)}<span class="text-xs text-base-content/40 font-normal">/mo</span>
			</p>
		</div>

		<!-- Stripe PaymentElement -->
		<div class="rounded-2xl border border-base-content/10 p-5">
			<div id="payment-element" class="min-h-[180px]"></div>
		</div>

		{#if paymentError}
			<div class="rounded-xl bg-error/10 border border-error/20 p-3">
				<p class="text-sm text-error">{paymentError}</p>
			</div>
		{/if}

		<button
			disabled={paymentLoading || !paymentMounted}
			onclick={confirmPayment}
			class="w-full h-12 flex items-center justify-center gap-2 rounded-xl text-sm font-bold bg-primary text-primary-content hover:brightness-110 transition-all disabled:opacity-50 disabled:cursor-not-allowed"
		>
			{#if paymentLoading}
				<Spinner size={16} /> Processing...
			{:else}
				<ShieldCheck class="w-4 h-4" /> Subscribe now
			{/if}
		</button>

		<div class="flex items-center justify-center gap-4 text-xs text-base-content/30">
			<span>Powered by Stripe</span>
			<span>·</span>
			<span>Cancel anytime</span>
		</div>
	</div>

<!-- ═══════════════════════════════════════════════════════════ -->
<!-- STEP 4: SUCCESS -->
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
