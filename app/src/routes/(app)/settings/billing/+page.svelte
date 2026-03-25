<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { ExternalLink, CreditCard, AlertTriangle, Receipt } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import type {
		NeboLoopAccountStatusResponse,
		PaymentMethodInfo,
		InvoiceInfo
	} from '$lib/api/neboComponents';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import Modal from '$lib/components/ui/Modal.svelte';
	import GiveNebo from '$lib/components/GiveNebo.svelte';

	let isLoading = $state(true);
	let status = $state<NeboLoopAccountStatusResponse | null>(null);
	let subscription = $state<{ plan: string; subscriptions: any[] } | null>(null);
	let paymentMethods = $state<PaymentMethodInfo[]>([]);
	let invoices = $state<InvoiceInfo[]>([]);
	let actionLoading = $state('');
	let actionError = $state('');
	let showInvoices = $state(false);
	let showCancelConfirm = $state(false);
	let showPaymentModal = $state(false);
	let stripeLoading = $state(false);
	let stripeError = $state('');
	let stripeSuccess = $state(false);
	let paymentElementContainer: HTMLDivElement | undefined;
	let stripeInstance: any = null;
	let elementsInstance: any = null;

	onMount(async () => {
		try {
			status = await api.neboLoopAccountStatus();
			if (status?.connected) {
				const [subResp, pmResp, invResp] = await Promise.allSettled([
					api.neboLoopBillingSubscription(),
					api.neboLoopBillingPaymentMethods(),
					api.neboLoopBillingInvoices()
				]);
				if (subResp.status === 'fulfilled') subscription = subResp.value;
				if (pmResp.status === 'fulfilled') paymentMethods = pmResp.value?.methods || [];
				if (invResp.status === 'fulfilled') invoices = invResp.value?.invoices || [];
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
	const defaultPayment = $derived(paymentMethods.find(pm => pm.isDefault) || paymentMethods[0]);

	function formatPrice(amountCents: number, currency: string): string {
		return new Intl.NumberFormat('en-US', {
			style: 'currency',
			currency: currency || 'usd',
			minimumFractionDigits: 0
		}).format(amountCents / 100);
	}

	function formatDate(dateStr: string): string {
		return new Date(dateStr).toLocaleDateString('en-US', {
			month: 'short',
			day: 'numeric',
			year: 'numeric'
		});
	}

	async function handlePortal() {
		actionLoading = 'portal';
		actionError = '';
		try {
			await api.neboLoopBillingPortal();
		} catch (e: any) {
			actionError = e?.message || 'Failed to open payment portal.';
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
			actionError = e?.message || 'Failed to cancel subscription.';
		} finally {
			actionLoading = '';
		}
	}

	async function loadStripe(): Promise<any> {
		if ((window as any).Stripe) return (window as any).Stripe;
		return new Promise((resolve, reject) => {
			const script = document.createElement('script');
			script.src = 'https://js.stripe.com/v3/';
			script.onload = () => resolve((window as any).Stripe);
			script.onerror = () => reject(new Error('Failed to load Stripe.js'));
			document.head.appendChild(script);
		});
	}

	async function openPaymentModal() {
		showPaymentModal = true;
		stripeLoading = true;
		stripeError = '';
		stripeSuccess = false;

		try {
			// Get SetupIntent from NeboLoop
			const { clientSecret, publishableKey } = await api.neboLoopBillingSetupIntent();

			// Load Stripe.js
			const Stripe = await loadStripe();
			stripeInstance = Stripe(publishableKey);
			elementsInstance = stripeInstance.elements({
				clientSecret,
				appearance: {
					theme: 'night',
					variables: {
						colorPrimary: '#14b8a6',
						colorBackground: '#1e1e2e',
						colorText: '#cdd6f4',
						colorTextSecondary: '#6c7086',
						borderRadius: '12px',
						fontFamily: 'system-ui, -apple-system, sans-serif',
					}
				}
			});

			// Mount PaymentElement after DOM is ready
			await new Promise(r => setTimeout(r, 50));
			if (paymentElementContainer) {
				const paymentElement = elementsInstance.create('payment');
				paymentElement.mount(paymentElementContainer);
			}
		} catch (e: any) {
			stripeError = e?.message || 'Failed to initialize payment form';
		} finally {
			stripeLoading = false;
		}
	}

	async function confirmPayment() {
		if (!stripeInstance || !elementsInstance) return;
		stripeLoading = true;
		stripeError = '';

		try {
			const { error } = await stripeInstance.confirmSetup({
				elements: elementsInstance,
				confirmParams: {
					return_url: `http://localhost:${location.port}/settings/billing`,
				},
				redirect: 'if_required',
			});

			if (error) {
				stripeError = error.message || 'Payment setup failed';
			} else {
				stripeSuccess = true;
				// Reload payment methods after a short delay (Stripe webhook may take a moment)
				setTimeout(async () => {
					try {
						const pmResp = await api.neboLoopBillingPaymentMethods();
						paymentMethods = pmResp?.methods || [];
					} catch { /* ignore */ }
					showPaymentModal = false;
					stripeSuccess = false;
				}, 2000);
			}
		} catch (e: any) {
			stripeError = e?.message || 'Payment confirmation failed';
		} finally {
			stripeLoading = false;
		}
	}

	function closePaymentModal() {
		showPaymentModal = false;
		stripeInstance = null;
		elementsInstance = null;
		stripeError = '';
	}
</script>

<div class="mb-6">
	<h2 class="font-display text-xl font-bold text-base-content mb-1">Billing</h2>
	<p class="text-base text-base-content/80">Subscription, payment, and invoices</p>
</div>

{#if isLoading}
	<div class="flex items-center justify-center gap-3 py-16">
		<Spinner size={20} />
		<span class="text-base text-base-content/80">Loading billing...</span>
	</div>
{:else if !status?.connected}
	<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
		<p class="text-base text-base-content/80">Connect your NeboLoop account to manage billing.</p>
		<a href="/settings/account" class="inline-block mt-3 text-base font-medium text-primary hover:brightness-110 transition-all">
			Go to Account
		</a>
	</div>
{:else}
	<div class="space-y-6">
		{#if actionError}
			<div class="rounded-xl bg-error/10 border border-error/20 p-3 flex items-center gap-2">
				<AlertTriangle class="w-4 h-4 text-error shrink-0" />
				<p class="text-sm text-error flex-1">{actionError}</p>
				<button onclick={() => (actionError = '')} class="text-sm text-error/60 hover:text-error">Dismiss</button>
			</div>
		{/if}

		<!-- Plan + Payment + Invoices — one card like Claude -->
		<section>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 divide-y divide-base-content/10">
				<!-- Plan -->
				<div class="flex items-center justify-between p-5">
					<div>
						<p class="text-base font-medium text-base-content">{planName} plan</p>
						{#if subscription?.subscriptions?.length}
							<p class="text-sm text-base-content/50">Auto-renews</p>
						{/if}
					</div>
					<button
						onclick={() => goto('/upgrade')}
						class="text-base text-primary font-medium hover:brightness-110 transition-all"
					>
						Adjust plan
					</button>
				</div>

				<!-- Payment -->
				<div class="flex items-center justify-between p-5">
					<div class="flex items-center gap-3">
						{#if defaultPayment}
							<CreditCard class="w-4 h-4 text-base-content/60" />
							<span class="text-base text-base-content">{defaultPayment.brand || defaultPayment.type} ending in {defaultPayment.lastFour || '****'}</span>
						{:else}
							<CreditCard class="w-4 h-4 text-base-content/40" />
							<span class="text-base text-base-content/60">No payment method</span>
						{/if}
					</div>
					<button
						onclick={openPaymentModal}
						class="text-base text-primary font-medium hover:brightness-110 transition-all"
					>
						Update
					</button>
				</div>

				<!-- Receipts -->
				<div class="flex items-center justify-between p-5">
					<div class="flex items-center gap-3">
						<Receipt class="w-4 h-4 text-base-content/60" />
						<span class="text-base text-base-content">{invoices.length} receipt{invoices.length !== 1 ? 's' : ''}</span>
					</div>
					{#if invoices.length > 0}
						<button
							onclick={() => (showInvoices = true)}
							class="text-base text-primary font-medium hover:brightness-110 transition-all"
						>
							View
						</button>
					{/if}
				</div>
			</div>
		</section>

		<!-- Give Nebo -->
		<GiveNebo />

		<!-- Cancel -->
		{#if currentPlan !== 'free' && subscription?.subscriptions?.length}
			<section>
				<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
					<div class="flex items-center justify-between">
						<p class="text-base text-base-content/60">Cancel plan</p>
						<button
							disabled={actionLoading !== ''}
							onclick={() => (showCancelConfirm = true)}
							class="text-sm text-error/70 hover:text-error transition-colors"
						>
							Cancel
						</button>
					</div>
				</div>
			</section>
		{/if}
	</div>
{/if}

<!-- Payment Method Modal (Stripe Elements) -->
{#if showPaymentModal}
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div class="nebo-modal-backdrop" role="dialog" aria-modal="true" tabindex="-1" onkeydown={(e) => e.key === 'Escape' && closePaymentModal()}>
		<button type="button" class="nebo-modal-overlay" onclick={closePaymentModal}></button>
		<div class="nebo-modal-card max-w-md">
			<div class="flex items-center justify-between px-5 py-4 border-b border-base-content/10">
				<h3 class="font-display text-lg font-bold">Payment method</h3>
				<button type="button" onclick={closePaymentModal} class="nebo-modal-close" aria-label="Close">
					<span class="text-base-content/60 text-xl">&times;</span>
				</button>
			</div>

			<div class="px-5 py-5">
				{#if stripeSuccess}
					<div class="py-8 text-center">
						<div class="w-12 h-12 rounded-full bg-success/10 flex items-center justify-center mx-auto mb-3">
							<CreditCard class="w-6 h-6 text-success" />
						</div>
						<p class="text-base font-medium text-base-content">Payment method saved</p>
						<p class="text-sm text-base-content/60 mt-1">Closing...</p>
					</div>
				{:else if stripeLoading && !elementsInstance}
					<div class="flex items-center justify-center gap-3 py-12">
						<Spinner size={20} />
						<span class="text-base text-base-content/80">Loading payment form...</span>
					</div>
				{:else}
					<div bind:this={paymentElementContainer} class="min-h-[200px]"></div>

					{#if stripeError}
						<div class="mt-3 rounded-xl bg-error/10 border border-error/20 p-3">
							<p class="text-sm text-error">{stripeError}</p>
						</div>
					{/if}
				{/if}
			</div>

			{#if !stripeSuccess}
				<div class="flex items-center justify-end gap-3 px-5 py-4 border-t border-base-content/10">
					<button
						type="button"
						class="h-10 px-5 rounded-full border border-base-content/10 text-base font-medium hover:bg-base-content/5 transition-colors"
						onclick={closePaymentModal}
					>
						Cancel
					</button>
					<button
						type="button"
						class="h-10 px-6 rounded-full bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all disabled:opacity-50"
						onclick={confirmPayment}
						disabled={stripeLoading || !elementsInstance}
					>
						{#if stripeLoading}<Spinner size={14} />{:else}Save{/if}
					</button>
				</div>
			{/if}
		</div>
	</div>
{/if}

<!-- Receipts Modal -->
<Modal bind:show={showInvoices} title="Receipts" size="md">
	<div class="divide-y divide-base-content/10">
		{#each invoices as inv}
			<div class="flex items-center justify-between py-3">
				<div>
					<p class="text-base text-base-content">{formatDate(inv.createdAt)}</p>
					{#if inv.description}
						<p class="text-sm text-base-content/60">{inv.description}</p>
					{/if}
				</div>
				<div class="flex items-center gap-3">
					<span class="text-base font-medium text-base-content tabular-nums">
						{formatPrice(inv.amountCents, inv.currency)}
					</span>
					<span class="text-sm text-base-content/50">{inv.status === 'paid' ? 'Paid' : inv.status}</span>
					{#if inv.hostedUrl || inv.pdfUrl}
						<a
							href={inv.hostedUrl || inv.pdfUrl}
							target="_blank"
							rel="noopener noreferrer"
							class="text-sm text-primary hover:brightness-110 transition-all"
						>
							View
						</a>
					{/if}
				</div>
			</div>
		{/each}
	</div>
</Modal>

<!-- Cancel Confirmation Modal -->
<Modal bind:show={showCancelConfirm} title="Cancel your plan?" size="sm">
	<div class="space-y-3">
		<p class="text-base text-base-content/80">Are you sure you want to cancel your <strong>{planName}</strong> plan? You'll lose access to your current token limits at the end of the billing period.</p>
		{#if actionError}
			<div class="rounded-xl bg-error/10 border border-error/20 p-3">
				<p class="text-sm text-error">{actionError}</p>
			</div>
		{/if}
	</div>

	{#snippet footer()}
		<button
			class="btn btn-ghost"
			onclick={() => { showCancelConfirm = false; actionError = ''; }}
			disabled={actionLoading === 'cancel'}
		>
			Keep plan
		</button>
		<button
			class="btn btn-error"
			disabled={actionLoading === 'cancel'}
			onclick={async () => {
				await handleCancel(subscription!.subscriptions[0].id);
				if (!actionError) showCancelConfirm = false;
			}}
		>
			{#if actionLoading === 'cancel'}<Spinner size={14} />{:else}Yes, cancel{/if}
		</button>
	{/snippet}
</Modal>
