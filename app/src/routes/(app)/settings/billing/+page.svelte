<script lang="ts">
	import { onMount } from 'svelte';
	import { ExternalLink, Check, CreditCard, Receipt, AlertTriangle } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import type {
		NeboLoopAccountStatusResponse,
		BillingPriceInfo,
		PaymentMethodInfo,
		InvoiceInfo
	} from '$lib/api/neboComponents';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import Modal from '$lib/components/ui/Modal.svelte';

	let isLoading = $state(true);
	let status = $state<NeboLoopAccountStatusResponse | null>(null);
	let prices = $state<BillingPriceInfo[]>([]);
	let subscription = $state<{ plan: string; subscriptions: any[] } | null>(null);
	let paymentMethods = $state<PaymentMethodInfo[]>([]);
	let invoices = $state<InvoiceInfo[]>([]);
	let showPlans = $state(false);
	let actionLoading = $state('');
	let actionError = $state('');

	// Delete account modal
	let showDeleteModal = $state(false);
	let deleteConfirmText = $state('');
	let deleteLoading = $state(false);
	const canDelete = $derived(deleteConfirmText === 'DELETE');

	// Fallback plans when the API returns nothing
	interface FallbackPlan {
		id: string;
		name: string;
		description: string;
		price: string;
		period: string;
		features: string[];
	}

	const fallbackPlans: FallbackPlan[] = [
		{
			id: 'free',
			name: 'Free',
			description: 'Get started with Nebo',
			price: 'Pay as you go',
			period: '',
			features: ['Basic AI access', 'Community skills', 'Pay per token']
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
			]
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
			]
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
			]
		}
	];

	const useFallback = $derived(prices.length === 0);

	onMount(async () => {
		try {
			status = await api.neboLoopAccountStatus();
			if (status?.connected) {
				const [pricesResp, subResp, pmResp, invResp] = await Promise.allSettled([
					api.neboLoopBillingPrices(),
					api.neboLoopBillingSubscription(),
					api.neboLoopBillingPaymentMethods(),
					api.neboLoopBillingInvoices()
				]);
				if (pricesResp.status === 'fulfilled') {
					prices = (pricesResp.value?.prices || []).sort(
						(a: BillingPriceInfo, b: BillingPriceInfo) => a.displayOrder - b.displayOrder
					);
				}
				if (subResp.status === 'fulfilled') {
					subscription = subResp.value;
				}
				if (pmResp.status === 'fulfilled') {
					paymentMethods = pmResp.value?.methods || [];
				}
				if (invResp.status === 'fulfilled') {
					invoices = invResp.value?.invoices || [];
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

	const currentPlan = $derived((subscription?.plan || status?.plan || 'free').toLowerCase());
	const planName = $derived(currentPlan.charAt(0).toUpperCase() + currentPlan.slice(1));

	// Dynamic prices
	const currentPriceInfo = $derived(prices.find(p => p.productName?.toLowerCase() === currentPlan));
	const otherPrices = $derived(prices.filter(p => p.productName?.toLowerCase() !== currentPlan));

	// Fallback plans
	const currentFallback = $derived(fallbackPlans.find(p => p.id === currentPlan) || fallbackPlans[0]);
	const otherFallbacks = $derived(fallbackPlans.filter(p => p.id !== currentPlan));

	function formatPrice(amountCents: number, currency: string): string {
		return new Intl.NumberFormat('en-US', {
			style: 'currency',
			currency: currency || 'usd',
			minimumFractionDigits: 0
		}).format(amountCents / 100);
	}

	function isUpgrade(price: BillingPriceInfo): boolean {
		if (!currentPriceInfo) return true;
		return price.displayOrder > currentPriceInfo.displayOrder;
	}

	function isFallbackUpgrade(planId: string): boolean {
		const planIdx = fallbackPlans.findIndex(p => p.id === planId);
		const currentIdx = fallbackPlans.findIndex(p => p.id === currentPlan);
		return planIdx > currentIdx;
	}

	async function handleCheckout(priceId: string) {
		actionLoading = 'checkout';
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

	async function handleCancel(subscriptionId: string) {
		actionLoading = 'cancel';
		actionError = '';
		try {
			await api.neboLoopBillingCancel(subscriptionId);
			try {
				subscription = await api.neboLoopBillingSubscription();
			} catch { /* ignore */ }
		} catch (e: any) {
			actionError = e?.message || 'Failed to cancel subscription. Please try again.';
		} finally {
			actionLoading = '';
		}
	}

	async function handleDeleteAccount() {
		if (!canDelete) return;
		deleteLoading = true;
		try {
			await api.deleteAccount({ password: '' });
			await api.neboLoopDisconnect();
			showDeleteModal = false;
			window.location.href = '/';
		} catch (e: any) {
			actionError = e?.message || 'Failed to delete account. Please try again.';
		} finally {
			deleteLoading = false;
		}
	}

	function openDeleteModal() {
		deleteConfirmText = '';
		actionError = '';
		showDeleteModal = true;
	}

	function formatDate(dateStr: string): string {
		return new Date(dateStr).toLocaleDateString('en-US', {
			month: 'short',
			day: 'numeric',
			year: 'numeric'
		});
	}
</script>

<div class="mb-6">
	<h2 class="font-display text-xl font-bold text-base-content mb-1">Billing</h2>
	<p class="text-sm text-base-content/70">Manage your subscription and payment method</p>
</div>

{#if isLoading}
	<div class="flex items-center justify-center gap-3 py-16">
		<Spinner size={20} />
		<span class="text-sm text-base-content/70">Loading billing...</span>
	</div>
{:else if !status?.connected}
	<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
		<p class="text-sm text-base-content/70">Connect your NeboLoop account to manage billing.</p>
		<a href="/settings/account" class="inline-block mt-3 text-sm font-medium text-primary hover:brightness-110 transition-all">
			Go to Account
		</a>
	</div>
{:else}
	<div class="space-y-8">
		<!-- Error Banner -->
		{#if actionError}
			<div class="rounded-2xl bg-error/10 border border-error/20 p-4 flex items-start gap-3">
				<AlertTriangle class="w-5 h-5 text-error shrink-0 mt-0.5" />
				<div class="flex-1">
					<p class="text-sm text-error">{actionError}</p>
				</div>
				<button onclick={() => (actionError = '')} class="text-sm text-error/70 hover:text-error">Dismiss</button>
			</div>
		{/if}

		<!-- Current Plan -->
		<section>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
				<div class="flex items-center justify-between">
					<div>
						<p class="text-lg font-bold text-base-content">{planName} plan</p>
						<p class="text-sm text-base-content/70 mt-0.5">
							{#if currentPriceInfo}
								{currentPriceInfo.productDescription || currentPriceInfo.productDisplayName}
							{:else}
								{currentFallback.description}
							{/if}
						</p>
					</div>
					<button
						class="h-9 px-4 rounded-xl border border-base-content/10 text-sm font-medium text-base-content hover:bg-base-content/5 transition-colors"
						onclick={() => (showPlans = !showPlans)}
					>
						{showPlans ? 'Hide plans' : 'Adjust plan'}
					</button>
				</div>

				<!-- Current plan features -->
				{#if !showPlans}
					{#if currentPriceInfo?.productFeatures?.length}
						<ul class="mt-4 space-y-1.5">
							{#each currentPriceInfo.productFeatures as feature}
								<li class="flex items-start gap-2 text-sm text-base-content/70">
									<Check class="w-4 h-4 text-primary shrink-0 mt-0.5" />
									{feature}
								</li>
							{/each}
						</ul>
					{:else}
						<ul class="mt-4 space-y-1.5">
							{#each currentFallback.features as feature}
								<li class="flex items-start gap-2 text-sm text-base-content/70">
									<Check class="w-4 h-4 text-primary shrink-0 mt-0.5" />
									{feature}
								</li>
							{/each}
						</ul>
					{/if}
				{/if}
			</div>
		</section>

		<!-- Plan Selection -->
		{#if showPlans}
			<section>
				{#if !useFallback && otherPrices.length > 0}
					<!-- Dynamic plans from API -->
					<div class="grid sm:grid-cols-2 gap-3">
						{#each otherPrices as price}
							<div class="rounded-2xl border bg-base-200/50 border-base-content/10 hover:border-base-content/20 p-5 transition-all">
								<p class="text-lg font-bold text-base-content">{price.productDisplayName || price.displayName}</p>
								<p class="text-sm text-base-content/70 mt-0.5">{price.productDescription || ''}</p>
								<div class="mt-3 mb-4">
									{#if price.amountCents > 0}
										<span class="text-2xl font-bold text-base-content">{formatPrice(price.amountCents, price.currency)}</span>
										<span class="text-sm text-base-content/70">/{price.interval}</span>
									{:else}
										<span class="text-2xl font-bold text-base-content">Free</span>
									{/if}
								</div>
								<button
									disabled={actionLoading !== ''}
									onclick={() => handleCheckout(price.stripePriceId)}
									class="w-full h-9 flex items-center justify-center rounded-xl text-sm font-bold transition-all
										{isUpgrade(price)
											? 'bg-primary text-primary-content hover:brightness-110'
											: 'border border-base-content/10 text-base-content hover:bg-base-content/5'}"
								>
									{#if actionLoading === 'checkout'}
										<Spinner size={14} />
									{:else}
										{isUpgrade(price) ? 'Upgrade' : 'Downgrade'}
									{/if}
								</button>
								{#if price.productFeatures && price.productFeatures.length > 0}
									<ul class="mt-4 space-y-2">
										{#each price.productFeatures as feature}
											<li class="flex items-start gap-2 text-sm text-base-content/70">
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
					<div class="grid sm:grid-cols-2 gap-3">
						{#each otherFallbacks as plan}
							<div class="rounded-2xl border bg-base-200/50 border-base-content/10 hover:border-base-content/20 p-5 transition-all">
								<p class="text-lg font-bold text-base-content">{plan.name}</p>
								<p class="text-sm text-base-content/70 mt-0.5">{plan.description}</p>
								<div class="mt-3 mb-4">
									<span class="text-2xl font-bold text-base-content">{plan.price}</span>
									{#if plan.period}
										<span class="text-sm text-base-content/70">{plan.period}</span>
									{/if}
								</div>
								<button
									disabled={actionLoading !== ''}
									onclick={handlePortal}
									class="w-full h-9 flex items-center justify-center rounded-xl text-sm font-bold transition-all
										{isFallbackUpgrade(plan.id)
											? 'bg-primary text-primary-content hover:brightness-110'
											: 'border border-base-content/10 text-base-content hover:bg-base-content/5'}"
								>
									{#if actionLoading === 'portal'}
										<Spinner size={14} />
									{:else}
										{isFallbackUpgrade(plan.id) ? 'Upgrade' : 'Downgrade'}
									{/if}
								</button>
								<ul class="mt-4 space-y-2">
									{#each plan.features as feature}
										<li class="flex items-start gap-2 text-sm text-base-content/70">
											<Check class="w-4 h-4 text-primary shrink-0 mt-0.5" />
											{feature}
										</li>
									{/each}
								</ul>
							</div>
						{/each}
					</div>
				{/if}
			</section>
		{/if}

		<!-- Payment Methods -->
		<section>
			<h3 class="text-sm font-semibold text-base-content/70 uppercase tracking-wider mb-3">Payment</h3>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
				{#if paymentMethods.length > 0}
					<div class="space-y-3 mb-4">
						{#each paymentMethods as pm}
							<div class="flex items-center justify-between">
								<div class="flex items-center gap-3">
									<CreditCard class="w-5 h-5 text-base-content/50" />
									<div>
										<p class="text-sm font-medium text-base-content">
											{pm.brand || pm.type} ending in {pm.lastFour || '****'}
										</p>
										{#if pm.expiresAt}
											<p class="text-xs text-base-content/50">Expires {pm.expiresAt}</p>
										{/if}
									</div>
								</div>
								{#if pm.isDefault}
									<span class="text-xs text-primary font-medium">Default</span>
								{/if}
							</div>
						{/each}
					</div>
				{:else}
					<p class="text-sm text-base-content/70 mb-4">No payment method on file</p>
				{/if}
				<button
					disabled={actionLoading === 'portal'}
					onclick={handlePortal}
					class="h-9 px-4 rounded-xl border border-base-content/10 text-sm font-medium text-base-content hover:bg-base-content/5 transition-colors flex items-center gap-1.5"
				>
					{#if actionLoading === 'portal'}
						<Spinner size={14} />
						<span>Opening...</span>
					{:else}
						Manage payment
						<ExternalLink class="w-3.5 h-3.5 text-base-content/50" />
					{/if}
				</button>
				<p class="text-xs text-base-content/50 mt-2">Opens Stripe in your browser</p>
			</div>
		</section>

		<!-- Invoices -->
		{#if invoices.length > 0}
			<section>
				<h3 class="text-sm font-semibold text-base-content/70 uppercase tracking-wider mb-3">Invoices</h3>
				<div class="rounded-2xl bg-base-200/50 border border-base-content/10 divide-y divide-base-content/5">
					{#each invoices.slice(0, 5) as inv}
						<div class="flex items-center justify-between p-4">
							<div class="flex items-center gap-3">
								<Receipt class="w-4 h-4 text-base-content/50" />
								<div>
									<p class="text-sm text-base-content">{inv.description || 'Invoice'}</p>
									<p class="text-xs text-base-content/50">{formatDate(inv.createdAt)}</p>
								</div>
							</div>
							<div class="flex items-center gap-3">
								<span class="text-sm font-medium text-base-content tabular-nums">
									{formatPrice(inv.amountCents, inv.currency)}
								</span>
								{#if inv.pdfUrl}
									<a
										href={inv.pdfUrl}
										target="_blank"
										rel="noopener noreferrer"
										class="text-sm text-primary hover:brightness-110 transition-all flex items-center gap-1"
									>
										PDF
										<ExternalLink class="w-3 h-3" />
									</a>
								{/if}
							</div>
						</div>
					{/each}
				</div>
			</section>
		{/if}

		<!-- Cancel / Delete Account -->
		<section>
			{#if currentPlan !== 'free' && subscription?.subscriptions?.length}
				<h3 class="text-sm font-semibold text-base-content/70 uppercase tracking-wider mb-3">Cancellation</h3>
				<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
					<div class="flex items-center justify-between">
						<p class="text-sm text-base-content/70">Cancel your {planName} plan</p>
						<button
							disabled={actionLoading !== ''}
							onclick={() => handleCancel(subscription!.subscriptions[0].id)}
							class="text-sm font-medium text-error hover:brightness-110 transition-colors"
						>
							{#if actionLoading === 'cancel'}
								<Spinner size={14} />
							{:else}
								Cancel plan
							{/if}
						</button>
					</div>
				</div>
			{:else}
				<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
					<div class="flex items-center justify-between">
						<p class="text-sm text-base-content/70">Want to remove your account and all data?</p>
						<button
							onclick={openDeleteModal}
							class="text-sm font-medium text-error hover:brightness-110 transition-colors"
						>
							Delete account
						</button>
					</div>
				</div>
			{/if}
		</section>
	</div>
{/if}

<!-- Delete Account Confirmation Modal -->
<Modal bind:show={showDeleteModal} title="Delete Account" size="sm">
	<div class="space-y-4">
		<div class="rounded-xl bg-error/10 border border-error/20 p-4">
			<div class="flex gap-3">
				<AlertTriangle class="w-5 h-5 text-error shrink-0 mt-0.5" />
				<div>
					<p class="text-sm font-medium text-error">This action is permanent</p>
					<p class="text-sm text-error/80 mt-1">
						Your account, settings, memories, and all associated data will be permanently deleted. This cannot be undone.
					</p>
				</div>
			</div>
		</div>

		<div>
			<label class="block text-sm font-medium text-base-content mb-1" for="confirm-delete">
				Type <code class="bg-base-200 px-1.5 py-0.5 rounded text-error font-bold">DELETE</code> to confirm
			</label>
			<input
				id="confirm-delete"
				type="text"
				class="input input-bordered w-full text-sm"
				placeholder="Type DELETE to confirm"
				bind:value={deleteConfirmText}
				onkeydown={(e) => {
					if (e.key === 'Enter' && canDelete) handleDeleteAccount();
				}}
			/>
		</div>
	</div>

	{#snippet footer()}
		<button
			onclick={() => (showDeleteModal = false)}
			class="h-9 px-4 rounded-xl border border-base-content/10 text-sm font-medium text-base-content hover:bg-base-content/5 transition-colors"
		>
			Cancel
		</button>
		<button
			disabled={!canDelete || deleteLoading}
			onclick={handleDeleteAccount}
			class="h-9 px-4 rounded-xl text-sm font-bold transition-all flex items-center gap-2
				{canDelete ? 'bg-error text-error-content hover:brightness-110' : 'bg-base-content/10 text-base-content/30 cursor-not-allowed'}"
		>
			{#if deleteLoading}
				<Spinner size={14} />
			{/if}
			Delete my account
		</button>
	{/snippet}
</Modal>
