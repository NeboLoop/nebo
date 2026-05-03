<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import CreditCard from 'lucide-svelte/icons/credit-card';
  import Receipt from 'lucide-svelte/icons/receipt';
  import ExternalLink from 'lucide-svelte/icons/external-link';
  let billing = $state({
    plan: '',
    interval: 'monthly' as const,
    autoRenews: false,
    paymentMethod: { brand: '', lastFour: '', expiresAt: '', isDefault: false },
    invoices: [] as { id: string; date: string; amount: number; currency: string; status: 'paid' | 'pending' | 'failed'; description: string }[],
  });
  let showInvoices = $state(false);
  let showCancelConfirm = $state(false);

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const [subResp, invResp] = await Promise.all([
        api.billingSubscription(),
        api.billingInvoices(),
      ]);
      const sub = subResp as Record<string, unknown> | null;
      if (sub) {
        const plan = sub.plan as Record<string, unknown> | undefined;
        billing = {
          ...billing,
          plan: String(plan?.name || sub.planName || billing.plan),
          autoRenews: !!(sub.autoRenews ?? billing.autoRenews),
          interval: (sub.interval as typeof billing.interval) || billing.interval,
          paymentMethod: (sub.paymentMethod as typeof billing.paymentMethod) || billing.paymentMethod,
        };
      }
      const invoiceData = invResp as Record<string, unknown> | null;
      const invoiceList = invoiceData?.invoices as Record<string, unknown>[] | undefined;
      if (invoiceList?.length) {
        billing.invoices = invoiceList.map((inv) => ({
          id: String(inv.id || ''),
          date: String(inv.date || inv.created || ''),
          description: String(inv.description || ''),
          amount: Number(inv.amount || inv.total || 0),
          currency: String(inv.currency || 'usd'),
          status: (inv.status as 'paid' | 'pending' | 'failed') || 'paid',
        }));
      }
    } catch { /* keep mock data */ }
  });

  async function openBillingPortal() {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.billingPortal() as Record<string, unknown>;
      if (resp?.url && typeof resp.url === 'string') {
        window.open(resp.url, '_blank');
      }
    } catch { /* ignore */ }
  }

  async function cancelSubscription() {
    try {
      const api = await import('$lib/api/nebo');
      await api.billingCancel();
      showCancelConfirm = false;
    } catch { /* ignore */ }
  }

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
</script>

<div class="mb-6">
  <h2 class="text-lg font-bold mb-1">Billing</h2>
  <p class="text-xs text-base-content/70">Manage your subscription and payment methods.</p>
</div>

<!-- Plan + Payment + Receipts — unified card like V1 -->
<div class="rounded-2xl bg-base-200/50 border border-base-content/10 divide-y divide-base-content/10 mb-6">
  <!-- Plan -->
  <div class="flex items-center justify-between p-5">
    <div>
      <p class="text-sm font-semibold text-base-content">{billing.plan} Plan</p>
      {#if billing.autoRenews}
        <p class="text-xs text-base-content/50">Auto-renews {billing.interval}</p>
      {/if}
    </div>
    <button
      onclick={() => goto('/upgrade')}
      class="text-sm text-primary font-medium hover:brightness-110 transition-all cursor-pointer bg-transparent border-none"
    >
      Adjust plan
    </button>
  </div>

  <!-- Payment method -->
  <div class="flex items-center justify-between p-5">
    <div class="flex items-center gap-3">
      {#if billing.paymentMethod}
        <CreditCard class="w-4 h-4 text-base-content/60" />
        <span class="text-sm text-base-content">{billing.paymentMethod.brand} ending in {billing.paymentMethod.lastFour}</span>
      {:else}
        <CreditCard class="w-4 h-4 text-base-content/40" />
        <span class="text-xs text-base-content/50">No payment method on file</span>
      {/if}
    </div>
    <button
      class="text-sm text-primary font-medium hover:brightness-110 transition-all cursor-pointer bg-transparent border-none"
    >
      Update
    </button>
  </div>

  <!-- Receipts -->
  <div class="flex items-center justify-between p-5">
    <div class="flex items-center gap-3">
      <Receipt class="w-4 h-4 text-base-content/60" />
      <span class="text-sm text-base-content">{billing.invoices.length} receipts</span>
    </div>
    {#if billing.invoices.length > 0}
      <button
        onclick={() => (showInvoices = !showInvoices)}
        class="text-sm text-primary font-medium hover:brightness-110 transition-all cursor-pointer bg-transparent border-none"
      >
        {showInvoices ? 'Hide' : 'View'}
      </button>
    {/if}
  </div>
</div>

<!-- Expanded invoices list -->
{#if showInvoices}
  <div class="rounded-2xl bg-base-200/50 border border-base-content/10 mb-6 divide-y divide-base-content/10">
    {#each billing.invoices as inv}
      <div class="flex items-center justify-between p-4">
        <div>
          <p class="text-sm text-base-content">{formatDate(inv.date)}</p>
          <p class="text-xs text-base-content/50">{inv.description}</p>
        </div>
        <div class="flex items-center gap-4">
          <span class="text-sm font-medium text-base-content tabular-nums">
            {formatPrice(inv.amount, inv.currency)}
          </span>
          <span class="px-2 py-0.5 rounded text-sm font-semibold {inv.status === 'paid' ? 'bg-success/10 text-success' : 'bg-warning/10 text-warning'}">
            {inv.status === 'paid' ? 'Paid' : inv.status}
          </span>
          <button class="text-sm text-primary hover:brightness-110 transition-all cursor-pointer bg-transparent border-none">
            View
          </button>
        </div>
      </div>
    {/each}
  </div>
{/if}

<!-- Stripe Customer Portal -->
<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5 mb-6">
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
<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
  <div class="flex items-center justify-between">
    <p class="text-xs text-base-content/50">Need to cancel your subscription?</p>
    {#if !showCancelConfirm}
      <button
        onclick={() => (showCancelConfirm = true)}
        class="text-sm text-error/70 hover:text-error transition-colors cursor-pointer bg-transparent border-none"
      >
        Cancel plan
      </button>
    {:else}
      <div class="flex items-center gap-2">
        <button
          onclick={() => (showCancelConfirm = false)}
          class="px-3 py-1 rounded-lg border border-base-content/10 text-sm cursor-pointer hover:bg-base-200 transition-colors"
        >
          Keep plan
        </button>
        <button
          class="px-3 py-1 rounded-lg bg-error text-error-content text-sm font-medium cursor-pointer hover:brightness-110 transition-all"
          onclick={cancelSubscription}
        >
          Yes, cancel
        </button>
      </div>
    {/if}
  </div>
</div>
