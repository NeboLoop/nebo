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
						onclick={handlePortal}
						disabled={actionLoading === 'portal'}
						class="text-base text-primary font-medium hover:brightness-110 transition-all flex items-center gap-1"
					>
						{#if actionLoading === 'portal'}
							<Spinner size={14} />
						{:else}
							Update
						{/if}
					</button>
				</div>

				<!-- Invoices -->
				<div class="flex items-center justify-between p-5">
					<div class="flex items-center gap-3">
						<Receipt class="w-4 h-4 text-base-content/60" />
						<span class="text-base text-base-content">{invoices.length} invoice{invoices.length !== 1 ? 's' : ''}</span>
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
							onclick={() => handleCancel(subscription!.subscriptions[0].id)}
							class="text-sm text-error/70 hover:text-error transition-colors"
						>
							{#if actionLoading === 'cancel'}<Spinner size={14} />{:else}Cancel{/if}
						</button>
					</div>
				</div>
			</section>
		{/if}
	</div>
{/if}

<!-- Invoices Modal -->
<Modal bind:show={showInvoices} title="Invoices" size="md">
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
					{#if inv.pdfUrl}
						<a
							href={inv.pdfUrl}
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
