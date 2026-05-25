<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import CreditCard from 'lucide-svelte/icons/credit-card';
  import Receipt from 'lucide-svelte/icons/receipt';
  import ExternalLink from 'lucide-svelte/icons/external-link';
  import TriangleAlert from 'lucide-svelte/icons/triangle-alert';
  import * as api from '$lib/api/nebo';
  import { listMarketplaceSubscriptions, cancelMarketplaceSubscription, type MarketplaceSubscriptionInfo } from '$lib/api/index';
  import type {
    AccountStatusResponse,
    NeboLoopBillingSubscriptionResponse,
    PaymentMethodInfo,
    InvoiceInfo
  } from '$lib/api/neboComponents';
  import Spinner from '$lib/components/ui/Spinner.svelte';
  import GiveNebo from '$lib/components/GiveNebo.svelte';

  let isLoading = $state(true);
  let status = $state<AccountStatusResponse | null>(null);
  let subscription = $state<NeboLoopBillingSubscriptionResponse | null>(null);
  let paymentMethods = $state<PaymentMethodInfo[]>([]);
  let invoices = $state<InvoiceInfo[]>([]);
  let marketplaceSubs = $state<MarketplaceSubscriptionInfo[]>([]);
  let actionLoading = $state('');
  let actionError = $state('');
  let showInvoices = $state(false);
  let showCancelConfirm = $state(false);
  let showPaymentModal = $state(false);
  let stripeLoading = $state(false);
  let stripeError = $state('');
  let stripeSuccess = $state(false);
  let paymentElementContainer = $state<HTMLDivElement | undefined>(undefined);
  let stripeInstance: any = null;
  let elementsInstance = $state<any>(null);
  let cancellingSubId = $state('');

  onMount(() => {
    (async () => {
      try {
        status = (await api.neboLoopAccountStatus()) as AccountStatusResponse;
        if (status?.connected) {
          const [subResp, pmResp, invResp, mktResp] = await Promise.allSettled([
            api.neboLoopBillingSubscription(),
            api.neboLoopBillingPaymentMethods(),
            api.neboLoopBillingInvoices(),
            listMarketplaceSubscriptions()
          ]);
          if (subResp.status === 'fulfilled') subscription = subResp.value;
          if (pmResp.status === 'fulfilled') paymentMethods = pmResp.value?.methods || [];
          if (invResp.status === 'fulfilled') invoices = invResp.value?.invoices || [];
          if (mktResp.status === 'fulfilled') marketplaceSubs = mktResp.value?.subscriptions || [];
        }
      } catch {
        status = null;
      } finally {
        isLoading = false;
      }
    })();

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

  async function openBillingPortal() {
    actionLoading = 'portal';
    actionError = '';
    try {
      await api.neboLoopBillingPortal();
    } catch (e: any) {
      actionError = e?.message || 'Failed to open billing portal';
    } finally {
      actionLoading = '';
    }
  }

  async function handleCancel(subscriptionId: string) {
    actionLoading = 'cancel';
    actionError = '';
    try {
      await api.neboLoopBillingCancel({ subscriptionId });
      try {
        subscription = await api.neboLoopBillingSubscription();
      } catch { /* ignore */ }
      showCancelConfirm = false;
    } catch (e: any) {
      actionError = e?.message || 'Failed to cancel subscription';
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
      script.onerror = () => reject(new Error('Failed to initialize payment form'));
      document.head.appendChild(script);
    });
  }

  async function openPaymentModal() {
    showPaymentModal = true;
    stripeLoading = true;
    stripeError = '';
    stripeSuccess = false;

    try {
      const { clientSecret, publishableKey } = await api.neboLoopBillingSetupIntent();

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
  <h2 class="text-lg font-bold mb-1">Billing</h2>
  <p class="text-xs text-base-content/70">Manage your subscription and payment methods.</p>
</div>

{#if isLoading}
  <div class="flex items-center justify-center gap-3 py-16">
    <Spinner size={20} />
    <span class="text-xs text-base-content/70">Loading billing...</span>
  </div>
{:else if !status?.connected}
  <div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
    <p class="text-xs text-base-content/70">Connect your NeboLoop account to manage billing.</p>
    <a href="/settings/account" class="inline-block mt-3 text-sm font-medium text-primary hover:brightness-110 transition-all">
      Go to Account
    </a>
  </div>
{:else}
  <div class="space-y-6">
    {#if actionError}
      <div class="rounded-xl bg-error/10 border border-error/20 p-3 flex items-center gap-2">
        <TriangleAlert class="w-4 h-4 text-error shrink-0" />
        <p class="text-sm text-error flex-1">{actionError}</p>
        <button onclick={() => (actionError = '')} class="text-xs text-error/60 hover:text-error cursor-pointer bg-transparent border-none">Dismiss</button>
      </div>
    {/if}

    <!-- Plan + Payment + Invoices -->
    <section>
      <div class="rounded-2xl bg-base-200/50 border border-base-content/10 divide-y divide-base-content/10">
        <!-- Plan -->
        <div class="flex items-center justify-between p-5">
          <div>
            <p class="text-sm font-semibold text-base-content">{planName} Plan</p>
            {#if subscription?.subscriptions?.length}
              <p class="text-xs text-base-content/50">Auto-renews</p>
            {/if}
          </div>
          <button
            onclick={() => goto('/upgrade')}
            class="text-sm text-primary font-medium hover:brightness-110 transition-all cursor-pointer bg-transparent border-none"
          >
            Adjust plan
          </button>
        </div>

        <!-- Payment -->
        <div class="flex items-center justify-between p-5">
          <div class="flex items-center gap-3">
            {#if defaultPayment}
              <CreditCard class="w-4 h-4 text-base-content/60" />
              <span class="text-sm text-base-content">{defaultPayment.brand || defaultPayment.type} ending in {defaultPayment.lastFour || '****'}</span>
            {:else}
              <CreditCard class="w-4 h-4 text-base-content/40" />
              <span class="text-xs text-base-content/50">No payment method on file</span>
            {/if}
          </div>
          <button
            onclick={openPaymentModal}
            class="text-sm text-primary font-medium hover:brightness-110 transition-all cursor-pointer bg-transparent border-none"
          >
            Update
          </button>
        </div>

        <!-- Receipts -->
        <div class="flex items-center justify-between p-5">
          <div class="flex items-center gap-3">
            <Receipt class="w-4 h-4 text-base-content/60" />
            <span class="text-sm text-base-content">{invoices.length} receipts</span>
          </div>
          {#if invoices.length > 0}
            <button
              onclick={() => (showInvoices = !showInvoices)}
              class="text-sm text-primary font-medium hover:brightness-110 transition-all cursor-pointer bg-transparent border-none"
            >
              {showInvoices ? 'Hide' : 'View'}
            </button>
          {/if}
        </div>
      </div>
    </section>

    <!-- Expanded invoices list -->
    {#if showInvoices}
      <div class="rounded-2xl bg-base-200/50 border border-base-content/10 divide-y divide-base-content/10">
        {#each invoices as inv}
          <div class="flex items-center justify-between p-4">
            <div>
              <p class="text-sm text-base-content">{formatDate(inv.createdAt ?? '')}</p>
              {#if inv.description}
                <p class="text-xs text-base-content/50">{inv.description}</p>
              {/if}
            </div>
            <div class="flex items-center gap-4">
              <span class="text-sm font-medium text-base-content tabular-nums">
                {formatPrice(inv.amountCents ?? 0, inv.currency ?? 'usd')}
              </span>
              <span class="px-2 py-0.5 rounded text-xs font-semibold {inv.status === 'paid' ? 'bg-success/10 text-success' : 'bg-warning/10 text-warning'}">
                {inv.status === 'paid' ? 'Paid' : inv.status}
              </span>
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
    {/if}

    <!-- Marketplace Subscriptions -->
    {#if marketplaceSubs.length > 0}
      <section>
        <p class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">Marketplace Subscriptions</p>
        <div class="rounded-2xl bg-base-200/50 border border-base-content/10 divide-y divide-base-content/10">
          {#each marketplaceSubs as sub}
            <div class="flex items-center justify-between p-4">
              <div>
                <div class="flex items-center gap-2">
                  <p class="text-sm font-medium text-base-content">{sub.artifactName || sub.targetId}</p>
                  <span class="px-1.5 py-0.5 rounded text-xs font-mono bg-base-200 text-base-content/70">{sub.targetType}</span>
                </div>
                <div class="flex items-center gap-3 mt-1">
                  {#if sub.priceCents}
                    <span class="text-xs text-base-content/50">{formatPrice(sub.priceCents, sub.billingInterval || 'usd')}/{sub.billingInterval === 'year' ? 'yr' : 'mo'}</span>
                  {/if}
                  {#if sub.currentPeriodEnd}
                    <span class="text-xs text-base-content/50">Renews {formatDate(sub.currentPeriodEnd)}</span>
                  {/if}
                  <span class="px-1.5 py-0.5 rounded text-xs font-semibold {sub.status === 'active' ? 'bg-success/10 text-success' : sub.status === 'cancelled' ? 'bg-error/10 text-error' : 'bg-warning/10 text-warning'}">{sub.status}</span>
                </div>
              </div>
              {#if sub.status === 'active'}
                {#if cancellingSubId === sub.id}
                  <div class="flex items-center gap-2">
                    <button onclick={() => (cancellingSubId = '')} class="px-2 py-1 rounded-lg border border-base-content/10 text-xs cursor-pointer hover:bg-base-200 transition-colors bg-transparent">Keep</button>
                    <button
                      class="px-2 py-1 rounded-lg bg-error text-error-content text-xs font-medium cursor-pointer hover:brightness-110 transition-all border-none disabled:opacity-50"
                      disabled={actionLoading === `cancel-mkt-${sub.id}`}
                      onclick={async () => {
                        actionLoading = `cancel-mkt-${sub.id}`;
                        try {
                          await cancelMarketplaceSubscription(sub.id);
                          marketplaceSubs = marketplaceSubs.map(s => s.id === sub.id ? { ...s, status: 'cancelled' } : s);
                          cancellingSubId = '';
                        } catch (e) {
                          actionError = (e as any)?.message || 'Failed to cancel';
                        } finally {
                          actionLoading = '';
                        }
                      }}
                    >
                      {#if actionLoading === `cancel-mkt-${sub.id}`}<Spinner size={12} />{:else}Cancel{/if}
                    </button>
                  </div>
                {:else}
                  <button
                    onclick={() => (cancellingSubId = sub.id)}
                    class="text-xs text-error/70 hover:text-error transition-colors cursor-pointer bg-transparent border-none"
                  >Cancel</button>
                {/if}
              {/if}
            </div>
          {/each}
        </div>
      </section>
    {/if}

    <!-- Give Nebo -->
    <GiveNebo />

    <!-- Stripe Customer Portal -->
    <div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
      <div class="flex items-center justify-between">
        <div>
          <p class="text-sm font-semibold text-base-content">Billing Portal</p>
          <p class="text-xs text-base-content/50">Manage invoices, tax IDs, and billing details on Stripe</p>
        </div>
        <button class="flex items-center gap-1.5 text-sm text-primary font-medium hover:brightness-110 transition-all cursor-pointer bg-transparent border-none" onclick={openBillingPortal}>
          Open portal <ExternalLink class="w-3.5 h-3.5" />
        </button>
      </div>
    </div>

    <!-- Cancel subscription -->
    {#if currentPlan !== 'free' && subscription?.subscriptions?.length}
      <div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
        <div class="flex items-center justify-between">
          <p class="text-xs text-base-content/50">Need to cancel your subscription?</p>
          {#if !showCancelConfirm}
            <button
              disabled={actionLoading !== ''}
              onclick={() => (showCancelConfirm = true)}
              class="text-sm text-error/70 hover:text-error transition-colors cursor-pointer bg-transparent border-none"
            >
              Cancel plan
            </button>
          {:else}
            <div class="flex items-center gap-2">
              <button
                onclick={() => { showCancelConfirm = false; actionError = ''; }}
                disabled={actionLoading === 'cancel'}
                class="px-3 py-1 rounded-lg border border-base-content/10 text-sm cursor-pointer hover:bg-base-200 transition-colors bg-transparent"
              >
                Keep plan
              </button>
              <button
                class="px-3 py-1 rounded-lg bg-error text-error-content text-sm font-medium cursor-pointer hover:brightness-110 transition-all border-none"
                disabled={actionLoading === 'cancel'}
                onclick={async () => {
                  const sub = subscription!.subscriptions[0];
                  await handleCancel(sub.stripeSubscriptionId || sub.id || '');
                }}
              >
                {#if actionLoading === 'cancel'}
                  <Spinner size={14} />
                {:else}
                  Yes, cancel
                {/if}
              </button>
            </div>
          {/if}
        </div>
      </div>
    {/if}
  </div>
{/if}

<!-- Payment Method Modal (Stripe Elements) -->
{#if showPaymentModal}
  <div class="fixed inset-0 z-[80] flex items-center justify-center" role="dialog" aria-modal="true">
    <button type="button" class="absolute inset-0 bg-black/60 backdrop-blur-sm cursor-default border-none" onclick={closePaymentModal} aria-label="Close"></button>
    <div class="relative rounded-2xl bg-base-100 w-full max-w-md shadow-xl">
      <div class="flex items-center justify-between px-5 py-4 border-b border-base-content/10">
        <h3 class="text-base font-semibold">Payment Method</h3>
        <button type="button" onclick={closePaymentModal} class="text-base-content/60 text-xl hover:text-base-content cursor-pointer bg-transparent border-none" aria-label="Close">
          &times;
        </button>
      </div>

      <div class="px-5 py-5">
        {#if stripeSuccess}
          <div class="py-8 text-center">
            <div class="w-12 h-12 rounded-full bg-success/10 flex items-center justify-center mx-auto mb-3">
              <CreditCard class="w-6 h-6 text-success" />
            </div>
            <p class="text-sm font-medium text-base-content">Payment method saved</p>
            <p class="text-xs text-base-content/50 mt-1">Closing...</p>
          </div>
        {:else if stripeLoading && !elementsInstance}
          <div class="flex items-center justify-center gap-3 py-12">
            <Spinner size={20} />
            <span class="text-xs text-base-content/70">Loading payment form...</span>
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
            class="h-10 px-5 rounded-full border border-base-content/10 text-sm font-medium hover:bg-base-content/5 transition-colors cursor-pointer bg-transparent"
            onclick={closePaymentModal}
          >
            Cancel
          </button>
          <button
            type="button"
            class="h-10 px-6 rounded-full bg-primary text-primary-content text-sm font-bold hover:brightness-110 transition-all disabled:opacity-50 cursor-pointer border-none"
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
