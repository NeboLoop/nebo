<script lang="ts">
	import { onMount } from 'svelte';
	import { Check, Zap, Crown, Star, Sparkles } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import type {
		NeboLoopAccountStatusResponse,
		BillingPriceInfo
	} from '$lib/api/neboComponents';
	import Spinner from '$lib/components/ui/Spinner.svelte';

	let isLoading = $state(true);
	let status = $state<NeboLoopAccountStatusResponse | null>(null);
	let prices = $state<BillingPriceInfo[]>([]);
	let subscription = $state<{ plan: string; subscriptions: any[] } | null>(null);
	let actionLoading = $state('');
	let actionError = $state('');

	interface FallbackPlan {
		id: string;
		name: string;
		description: string;
		price: string;
		period: string;
		features: string[];
		icon: typeof Zap;
	}

	const fallbackPlans: FallbackPlan[] = [
		{
			id: 'free',
			name: 'Free',
			description: 'Get started with Nebo',
			price: 'Pay as you go',
			period: '',
			features: ['Basic AI access', 'Community skills', 'Pay per token', 'Local inference'],
			icon: Sparkles
		},
		{
			id: 'pro',
			name: 'Pro',
			description: 'For individuals who use Nebo daily',
			price: '$20',
			period: '/month',
			features: [
				'Everything in Free',
				'Included usage allocation',
				'Priority access',
				'Unlimited roles',
				'Workflow automation'
			],
			icon: Zap
		},
		{
			id: 'max',
			name: 'Max',
			description: 'Maximum power for professionals',
			price: '$100',
			period: '/month',
			features: [
				'Everything in Pro',
				'5x more usage',
				'Higher output limits',
				'Early access to features',
				'Priority support'
			],
			icon: Crown
		},
		{
			id: 'team',
			name: 'Team',
			description: 'Collaboration for teams',
			price: '$200',
			period: '/month',
			features: [
				'Everything in Max',
				'Team workspace',
				'Shared roles and skills',
				'Admin controls',
				'Dedicated support'
			],
			icon: Star
		}
	];

	const currentPlan = $derived((subscription?.plan || status?.plan || 'free').toLowerCase());
	const useFallback = $derived(prices.length === 0);

	onMount(async () => {
		try {
			status = await api.neboLoopAccountStatus();
			if (status?.connected) {
				const [pricesResp, subResp] = await Promise.allSettled([
					api.neboLoopBillingPrices(),
					api.neboLoopBillingSubscription()
				]);
				if (pricesResp.status === 'fulfilled') {
					prices = (pricesResp.value?.prices || []).sort(
						(a: BillingPriceInfo, b: BillingPriceInfo) => a.displayOrder - b.displayOrder
					);
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

	function isCurrentPlan(planId: string): boolean {
		return planId === currentPlan;
	}

	function isUpgrade(planId: string): boolean {
		const order = ['free', 'pro', 'max', 'team'];
		return order.indexOf(planId) > order.indexOf(currentPlan);
	}

	function isUpgradePrice(price: BillingPriceInfo): boolean {
		const currentPrice = prices.find(p => p.productName?.toLowerCase() === currentPlan);
		if (!currentPrice) return true;
		return price.displayOrder > currentPrice.displayOrder;
	}

	async function handleCheckout(priceId: string) {
		actionLoading = priceId;
		actionError = '';
		try {
			await api.neboLoopBillingCheckout(priceId);
		} catch (e: any) {
			actionError = e?.message || 'Failed to open checkout. Please try again.';
		} finally {
			actionLoading = '';
		}
	}

	async function handlePortal() {
		actionLoading = 'portal';
		actionError = '';
		try {
			await api.neboLoopBillingPortal();
		} catch (e: any) {
			actionError = e?.message || 'Failed to open payment portal. Please try again.';
		} finally {
			actionLoading = '';
		}
	}

	const includedFeatures = [
		'Local inference',
		'Skills platform',
		'Roles & workflows',
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
	<div class="space-y-12">
		<!-- Header -->
		<div class="text-center">
			<h1 class="font-display text-2xl font-bold text-base-content">Choose your plan</h1>
			<p class="text-base text-base-content/80 mt-1">Simple pricing for every stage</p>
		</div>

		<!-- Error -->
		{#if actionError}
			<div class="rounded-2xl bg-error/10 border border-error/20 p-4 flex items-center justify-between">
				<p class="text-base text-error">{actionError}</p>
				<button onclick={() => (actionError = '')} class="text-base text-error/70 hover:text-error">Dismiss</button>
			</div>
		{/if}

		<!-- Plan Cards -->
		{#if !useFallback && prices.length > 0}
			<div class="grid sm:grid-cols-2 gap-4">
				{#each prices as price}
					{@const isCurrent = price.productName?.toLowerCase() === currentPlan}
					{@const upgrade = isUpgradePrice(price)}
					<div class="rounded-2xl bg-base-200/50 border p-5 transition-all
						{isCurrent ? 'ring-2 ring-primary border-primary/20' : 'border-base-content/10 hover:border-base-content/30'}">
						<div class="flex items-center justify-between mb-1">
							<p class="text-lg font-bold text-base-content">{price.productDisplayName || price.displayName}</p>
							{#if isCurrent}
								<span class="text-xs font-bold text-primary bg-primary/10 px-2 py-0.5 rounded-full">Current plan</span>
							{/if}
						</div>
						<p class="text-base text-base-content/80">{price.productDescription || ''}</p>
						<div class="mt-3 mb-4">
							{#if price.amountCents > 0}
								<span class="text-2xl font-bold text-base-content">{formatPrice(price.amountCents, price.currency)}</span>
								<span class="text-base text-base-content/80">/{price.interval}</span>
							{:else}
								<span class="text-2xl font-bold text-base-content">Free</span>
							{/if}
						</div>
						{#if isCurrent}
							<button
								disabled
								class="w-full h-9 flex items-center justify-center rounded-xl text-base font-bold bg-base-content/10 text-base-content/40 cursor-not-allowed"
							>
								Current plan
							</button>
						{:else if upgrade}
							<button
								disabled={actionLoading !== ''}
								onclick={() => handleCheckout(price.stripePriceId)}
								class="w-full h-9 flex items-center justify-center rounded-xl text-base font-bold bg-primary text-primary-content hover:brightness-110 transition-all"
							>
								{#if actionLoading === price.stripePriceId}
									<Spinner size={14} />
								{:else}
									Upgrade
								{/if}
							</button>
						{:else}
							<button
								disabled={actionLoading !== ''}
								onclick={() => handleCheckout(price.stripePriceId)}
								class="w-full h-9 flex items-center justify-center rounded-xl text-base font-bold border border-base-content/10 text-base-content hover:bg-base-content/5 transition-all"
							>
								{#if actionLoading === price.stripePriceId}
									<Spinner size={14} />
								{:else}
									Downgrade
								{/if}
							</button>
						{/if}
						{#if price.productFeatures && price.productFeatures.length > 0}
							<ul class="mt-4 space-y-2">
								{#each price.productFeatures as feature}
									<li class="flex items-start gap-2 text-base text-base-content/80">
										<Check class="w-4 h-4 text-primary shrink-0 mt-0.5" />
										{feature}
									</li>
								{/each}
							</ul>
						{/if}
					</div>
				{/each}
			</div>
		{:else}
			<!-- Fallback plans -->
			<div class="grid sm:grid-cols-2 gap-4">
				{#each fallbackPlans as plan}
					{@const isCurrent = isCurrentPlan(plan.id)}
					{@const upgrade = isUpgrade(plan.id)}
					<div class="rounded-2xl bg-base-200/50 border p-5 transition-all
						{isCurrent ? 'ring-2 ring-primary border-primary/20' : 'border-base-content/10 hover:border-base-content/30'}">
						<div class="flex items-center justify-between mb-1">
							<div class="flex items-center gap-2">
								<plan.icon class="w-5 h-5 text-primary" />
								<p class="text-lg font-bold text-base-content">{plan.name}</p>
							</div>
							{#if isCurrent}
								<span class="text-xs font-bold text-primary bg-primary/10 px-2 py-0.5 rounded-full">Current plan</span>
							{/if}
						</div>
						<p class="text-base text-base-content/80">{plan.description}</p>
						<div class="mt-3 mb-4">
							<span class="text-2xl font-bold text-base-content">{plan.price}</span>
							{#if plan.period}
								<span class="text-base text-base-content/80">{plan.period}</span>
							{/if}
						</div>
						{#if isCurrent}
							<button
								disabled
								class="w-full h-9 flex items-center justify-center rounded-xl text-base font-bold bg-base-content/10 text-base-content/40 cursor-not-allowed"
							>
								Current plan
							</button>
						{:else if upgrade}
							<button
								disabled={actionLoading !== ''}
								onclick={handlePortal}
								class="w-full h-9 flex items-center justify-center rounded-xl text-base font-bold bg-primary text-primary-content hover:brightness-110 transition-all"
							>
								{#if actionLoading === 'portal'}
									<Spinner size={14} />
								{:else}
									Upgrade
								{/if}
							</button>
						{:else}
							<button
								disabled={actionLoading !== ''}
								onclick={handlePortal}
								class="w-full h-9 flex items-center justify-center rounded-xl text-base font-bold border border-base-content/10 text-base-content hover:bg-base-content/5 transition-all"
							>
								{#if actionLoading === 'portal'}
									<Spinner size={14} />
								{:else}
									Downgrade
								{/if}
							</button>
						{/if}
						<ul class="mt-4 space-y-2">
							{#each plan.features as feature}
								<li class="flex items-start gap-2 text-base text-base-content/80">
									<Check class="w-4 h-4 text-primary shrink-0 mt-0.5" />
									{feature}
								</li>
							{/each}
						</ul>
					</div>
				{/each}
			</div>
		{/if}

		<!-- Credit Packs (placeholder) -->
		<section>
			<h2 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">Prepaid Credits</h2>
			<div class="rounded-2xl bg-base-200/30 border border-base-content/10 border-dashed p-5">
				<div class="flex items-center justify-between">
					<div>
						<p class="text-base font-medium text-base-content/60">Buy credits for pay-as-you-go usage</p>
					</div>
					<span class="text-xs font-bold text-base-content/40 bg-base-content/5 px-2 py-0.5 rounded-full">Coming soon</span>
				</div>
			</div>
		</section>

		<!-- Developer Program (placeholder) -->
		<section>
			<h2 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">Developer Program</h2>
			<div class="rounded-2xl bg-base-200/30 border border-base-content/10 border-dashed p-5">
				<div class="flex items-center justify-between">
					<div>
						<p class="text-base font-medium text-base-content/60">$29/year</p>
						<p class="text-base text-base-content/40 mt-0.5">Build and publish skills and apps for the Nebo marketplace</p>
					</div>
					<span class="text-xs font-bold text-base-content/40 bg-base-content/5 px-2 py-0.5 rounded-full">Coming soon</span>
				</div>
			</div>
		</section>

		<!-- All plans include -->
		<section class="pb-6">
			<h2 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-4">All plans include</h2>
			<div class="flex flex-wrap gap-x-6 gap-y-2">
				{#each includedFeatures as feature}
					<div class="flex items-center gap-2 text-base text-base-content/80">
						<Check class="w-4 h-4 text-primary shrink-0" />
						{feature}
					</div>
				{/each}
			</div>
		</section>
	</div>
{/if}
