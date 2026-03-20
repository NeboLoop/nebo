<script lang="ts">
	import { onMount } from 'svelte';
	import { Check, Zap, ArrowRight } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import type {
		NeboLoopAccountStatusResponse,
		BillingPriceInfo
	} from '$lib/api/neboComponents';
	import Spinner from '$lib/components/ui/Spinner.svelte';

	let isLoading = $state(true);
	let status = $state<NeboLoopAccountStatusResponse | null>(null);
	let allPrices = $state<BillingPriceInfo[]>([]);
	let subscription = $state<{ plan: string; subscriptions: any[] } | null>(null);
	let actionLoading = $state('');
	let actionError = $state('');
	let boostSelections = $state<Record<string, boolean>>({});
	let activeTab = $state<'personal' | 'business'>('personal');

	const currentPlan = $derived((subscription?.plan || status?.plan || 'free').toLowerCase());

	const personalPrices = $derived(
		allPrices.filter((p) => p.category === 'personal').sort((a, b) => a.displayOrder - b.displayOrder)
	);
	const businessPrices = $derived(
		allPrices.filter((p) => p.category === 'business').sort((a, b) => a.displayOrder - b.displayOrder)
	);
	const boostPrices = $derived(allPrices.filter((p) => p.category === 'boost'));
	const visiblePrices = $derived(activeTab === 'personal' ? personalPrices : businessPrices);

	// Middle card is "popular"
	const popularIndex = $derived(Math.floor(visiblePrices.length / 2));

	function getBoostPrice(boostPriceId: string | undefined): BillingPriceInfo | undefined {
		if (!boostPriceId) return undefined;
		return boostPrices.find((p) => p.id === boostPriceId);
	}

	onMount(async () => {
		try {
			status = await api.neboLoopAccountStatus();
			if (status?.connected) {
				const [pricesResp, subResp] = await Promise.allSettled([
					api.neboLoopBillingPrices(),
					api.neboLoopBillingSubscription()
				]);
				if (pricesResp.status === 'fulfilled') {
					allPrices = pricesResp.value?.prices || [];
				}
				if (subResp.status === 'fulfilled') {
					subscription = subResp.value;
				}
			}
		} catch {
			status = null;
		} finally {
			isLoading = false;
		}

		const handler = (e: Event) => {
			const detail = (e as CustomEvent).detail;
			if (detail?.plan && status) {
				status = { ...status, plan: detail.plan };
			}
		};
		window.addEventListener('nebo:plan_changed', handler);
		return () => window.removeEventListener('nebo:plan_changed', handler);
	});

	function formatPrice(amountCents: number, currency: string): string {
		return new Intl.NumberFormat('en-US', {
			style: 'currency',
			currency: currency || 'usd',
			minimumFractionDigits: 0
		}).format(amountCents / 100);
	}

	async function handleCheckout(mainPriceId: string, boostStripePriceId?: string) {
		actionLoading = mainPriceId;
		actionError = '';
		try {
			// Build price list — main plan + boost if checked
			const priceIds = [mainPriceId];
			if (boostStripePriceId && boostSelections[mainPriceId]) {
				priceIds.push(boostStripePriceId);
			}

			// Single checkout session with all line items
			const resp = await fetch('/api/v1/neboloop/billing/checkout', {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				credentials: 'include',
				body: JSON.stringify({ priceIds })
			});
			if (!resp.ok) {
				throw new Error('Checkout failed');
			}
		} catch (e: any) {
			actionError = e?.message || 'Failed to open checkout. Please try again.';
		} finally {
			actionLoading = '';
		}
	}

	const includedFeatures = [
		'Runs on your machine',
		'Your data stays local',
		'Skills & roles marketplace',
		'Desktop automation',
		'MCP integrations',
		'Memory system'
	];
</script>

<svelte:head>
	<title>Choose your plan - Nebo</title>
</svelte:head>

{#if isLoading}
	<div class="flex items-center justify-center gap-3 py-24">
		<Spinner size={20} />
		<span class="text-base text-base-content/80">Loading plans...</span>
	</div>
{:else if !status?.connected}
	<div class="text-center py-24">
		<h1 class="font-display text-2xl font-bold text-base-content mb-2">Connect NeboLoop</h1>
		<p class="text-base text-base-content/80 mb-6">Connect your NeboLoop account to view plans and upgrade.</p>
		<a
			href="/settings/account"
			class="inline-flex h-9 px-4 items-center rounded-xl bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all"
		>
			Go to Account
		</a>
	</div>
{:else}
	<div class="space-y-8">
		<!-- Header -->
		<div class="text-center space-y-2">
			<h1 class="font-display text-3xl font-bold text-base-content">Plans that grow with you</h1>
			<p class="text-base text-base-content/50 max-w-md mx-auto">AI that runs on your machine. Pick a plan, get instant access. Upgrade or cancel anytime.</p>
		</div>

		<!-- Toggle -->
		<div class="flex justify-center">
			<div class="inline-flex rounded-full bg-base-200/80 p-1">
				<button
					onclick={() => (activeTab = 'personal')}
					class="px-6 py-2 rounded-full text-sm font-semibold transition-all {activeTab === 'personal'
						? 'bg-base-100 text-base-content shadow-sm'
						: 'text-base-content/40 hover:text-base-content/60'}"
				>
					Personal
				</button>
				<button
					onclick={() => (activeTab = 'business')}
					class="px-6 py-2 rounded-full text-sm font-semibold transition-all {activeTab === 'business'
						? 'bg-base-100 text-base-content shadow-sm'
						: 'text-base-content/40 hover:text-base-content/60'}"
				>
					Business
				</button>
			</div>
		</div>

		<!-- Error -->
		{#if actionError}
			<div class="rounded-2xl bg-error/10 border border-error/20 p-4 flex items-center justify-between">
				<p class="text-sm text-error">{actionError}</p>
				<button onclick={() => (actionError = '')} class="text-sm text-error/70 hover:text-error">Dismiss</button>
			</div>
		{/if}

		<!-- Plan Cards -->
		{#if visiblePrices.length > 0}
			<div class="grid sm:grid-cols-3 gap-5">
				{#each visiblePrices as price, i (price.id)}
					{@const boost = getBoostPrice(price.boostPriceId)}
					{@const boostChecked = boostSelections[price.id] || false}
					{@const isPopular = i === popularIndex}
					{@const isCurrent = price.nickname === currentPlan}

					<div class="relative rounded-2xl border p-6 flex flex-col transition-all
						{isPopular
							? 'bg-primary/5 border-primary/30 ring-1 ring-primary/20 scale-[1.02]'
							: 'bg-base-200/50 border-base-content/10 hover:border-base-content/20'}">

						<!-- Popular badge -->
						{#if isPopular}
							<div class="absolute -top-3 left-1/2 -translate-x-1/2">
								<span class="px-3 py-1 rounded-full bg-primary text-primary-content text-xs font-bold shadow-sm">
									Most popular
								</span>
							</div>
						{/if}

						<!-- Current badge -->
						{#if isCurrent}
							<div class="absolute -top-3 right-4">
								<span class="px-3 py-1 rounded-full bg-base-content/10 text-base-content/60 text-xs font-bold">
									Current plan
								</span>
							</div>
						{/if}

						<!-- Tier name + description -->
						<h3 class="text-xl font-bold text-base-content {isPopular ? 'mt-1' : ''}">{price.displayName || price.nickname}</h3>
						{#if price.description}
							<p class="text-sm text-base-content/50 mt-1 leading-relaxed">{price.description}</p>
						{/if}

						<!-- Price -->
						<div class="mt-5 mb-5">
							<span class="text-4xl font-bold text-base-content tracking-tight">{formatPrice(price.amountCents, price.currency)}</span>
							<span class="text-sm text-base-content/40 ml-1">/{price.interval}</span>
						</div>

						<!-- Features -->
						{#if price.features && price.features.length > 0}
							<ul class="space-y-2.5 mb-5 flex-1">
								{#each price.features as feature}
									<li class="flex items-start gap-2 text-sm text-base-content/70">
										<Check class="w-4 h-4 shrink-0 mt-0.5 {isPopular ? 'text-primary' : 'text-base-content/30'}" />
										<span>{feature}</span>
									</li>
								{/each}
							</ul>
						{:else}
							<div class="flex-1"></div>
						{/if}

						<!-- Boost bump upsell -->
						{#if boost}
							<label class="flex items-start gap-2.5 mb-5 p-3 rounded-xl border cursor-pointer select-none group transition-all
								{boostChecked
									? 'bg-amber-500/10 border-amber-500/30'
									: 'bg-base-content/3 border-transparent hover:border-base-content/10'}">
								<input
									type="checkbox"
									class="checkbox checkbox-sm checkbox-warning mt-0.5"
									checked={boostChecked}
									onchange={() => (boostSelections[price.id] = !boostChecked)}
								/>
								<div class="flex-1">
									<div class="flex items-center gap-1.5">
										<Zap class="w-3.5 h-3.5 text-amber-500" />
										<span class="text-xs font-bold text-base-content uppercase tracking-wide">Advanced Compute</span>
									</div>
									<p class="text-xs text-base-content/50 mt-1 leading-relaxed">
										{boost.description || '3x access to frontier models — Opus 4.6, GPT-5.4, and more.'}
									</p>
									<p class="text-xs font-bold text-amber-600 mt-1">+{formatPrice(boost.amountCents, boost.currency)}/mo</p>
								</div>
							</label>
						{/if}

						<!-- CTA -->
						{#if isCurrent}
							<button
								disabled
								class="w-full h-11 flex items-center justify-center rounded-xl text-sm font-bold bg-base-content/10 text-base-content/40 cursor-not-allowed"
							>
								Current plan
							</button>
						{:else}
							<button
								disabled={actionLoading !== ''}
								onclick={() => handleCheckout(price.stripePriceId, boost?.stripePriceId)}
								class="w-full h-11 flex items-center justify-center gap-2 rounded-xl text-sm font-bold transition-all
									{isPopular
										? 'bg-primary text-primary-content hover:brightness-110 shadow-md shadow-primary/20'
										: activeTab === 'personal'
											? 'bg-primary text-primary-content hover:brightness-110'
											: 'bg-base-content text-base-100 hover:brightness-110'}"
							>
								{#if actionLoading === price.stripePriceId}
									<Spinner size={14} />
								{:else}
									Get started
									<ArrowRight class="w-4 h-4" />
								{/if}
							</button>
						{/if}
					</div>
				{/each}
			</div>
		{:else}
			<div class="text-center py-12 text-base-content/40">
				<p>No plans available. Please check back later.</p>
			</div>
		{/if}

		<!-- All plans include -->
		<section class="pt-4 pb-6">
			<div class="rounded-2xl bg-base-200/30 border border-base-content/5 p-6">
				<h2 class="text-xs font-bold text-base-content/30 uppercase tracking-widest mb-4">Every plan includes</h2>
				<div class="grid grid-cols-2 sm:grid-cols-3 gap-3">
					{#each includedFeatures as feature}
						<div class="flex items-center gap-2 text-sm text-base-content/60">
							<Check class="w-4 h-4 text-primary/60 shrink-0" />
							{feature}
						</div>
					{/each}
				</div>
			</div>
		</section>
	</div>
{/if}
