<script lang="ts">
  import SettingsHeader from '$lib/components/settings/SettingsHeader.svelte';
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import RefreshCw from 'lucide-svelte/icons/refresh-cw';
  import * as api from '$lib/api/nebo';
  import type { AccountStatusResponse, NeboAIBillingSubscriptionResponse } from '$lib/api/neboComponents';
  import Spinner from '$lib/components/ui/Spinner.svelte';
  import { onWsEvent } from '$lib/websocket/subscribe';

  interface UsagePool {
    resetAt?: string;
    percentUsed?: number;
    activePool?: string;
    freeAvailable?: number;
    giftAvailable?: number;
    creditsCents?: number;
    updatedAt?: string;
  }

  interface TypedJanusUsage {
    session: UsagePool | null;
    weekly: UsagePool | null;
    budget: UsagePool | null;
    updatedAt?: string;
  }

  interface BillingSub {
    id?: string;
    stripeSubscriptionId?: string;
    plan?: string;
    status?: string;
    currentPeriodEnd?: string;
    amountCents?: number;
    interval?: string;
  }

  let isLoading = $state(true);
  let refreshing = $state(false);
  let usage = $state<TypedJanusUsage | null>(null);
  let accountStatus = $state<AccountStatusResponse | null>(null);
  let subscription = $state<(Omit<NeboAIBillingSubscriptionResponse, 'subscriptions'> & { subscriptions: BillingSub[] }) | null>(null);
  let connected = $state(false);

  const currentPlan = $derived((subscription?.plan || accountStatus?.plan || 'free').toLowerCase());
  const planName = $derived(currentPlan.charAt(0).toUpperCase() + currentPlan.slice(1));

  const hasBudget = $derived(
    usage?.budget && ((usage.budget.giftAvailable ?? 0) > 0 || (usage.budget.creditsCents ?? 0) > 0 || (usage.budget.freeAvailable ?? 0) > 0 || !!usage.budget.activePool)
  );

  onMount(async () => {
    try {
      accountStatus = (await api.neboAIAccountStatus()) as AccountStatusResponse;
      connected = accountStatus?.connected || false;
      if (connected) {
        const [usageResp, subResp] = await Promise.allSettled([
          api.neboAIJanusUsage(),
          api.neboAIBillingSubscription()
        ]);
        if (usageResp.status === 'fulfilled') {
          const raw = usageResp.value;
          usage = {
            session: raw.session as UsagePool | null,
            weekly: raw.weekly as UsagePool | null,
            budget: raw.budget as UsagePool | null,
            updatedAt: (raw as TypedJanusUsage).updatedAt,
          };
        }
        if (subResp.status === 'fulfilled') subscription = subResp.value as typeof subscription;
      }
    } catch { /* ignore */ }
    isLoading = false;
  });

  async function refresh() {
    if (refreshing) return;
    refreshing = true;
    const min = new Promise(r => setTimeout(r, 800));
    try {
      const [raw] = await Promise.all([api.neboAIJanusUsageRefresh() as Promise<TypedJanusUsage>, min]);
      usage = raw;
    } catch { /* ignore */ }
    refreshing = false;
  }

  // Live updates: the existing `usage` WS event carries the current Janus plan
  // balance under `balance` (same shape as the GET endpoint), so the panel reflects
  // new session/weekly numbers without a manual refresh — one usage channel.
  onWsEvent<{ balance?: TypedJanusUsage | null }>('usage', (d) => {
    if (d?.balance) usage = { session: d.balance.session, weekly: d.balance.weekly, budget: d.balance.budget, updatedAt: d.balance.updatedAt };
  });

  function formatDollars(microdollars: number): string {
    const dollars = microdollars / 1_000_000;
    if (dollars >= 1000) return `$${(dollars / 1000).toFixed(1)}K`;
    return `$${dollars.toFixed(2)}`;
  }

  function timeUntilReset(resetAt?: string): string {
    if (!resetAt) return '';
    const diff = new Date(resetAt).getTime() - Date.now();
    if (diff <= 0) return $t('settingsUsage.resetting');
    const h = Math.floor(diff / 3600000);
    const m = Math.floor((diff % 3600000) / 60000);
    if (h > 24) {
      const d = Math.floor(h / 24);
      return $t('settingsUsage.resetsInDaysShort', { values: { days: d } });
    }
    return $t('settingsUsage.resetsInTimeShort', { values: { hours: h, minutes: m } });
  }

  function formatUpdatedAt(iso?: string): string {
    if (!iso) return '';
    const d = new Date(iso);
    const now = Date.now();
    const diff = now - d.getTime();
    if (diff < 60000) return $t('time.justNow');
    if (diff < 3600000) return $t('time.minutesAgo', { values: { n: Math.floor(diff / 60000) } });
    if (diff < 86400000) return $t('time.hoursAgo', { values: { n: Math.floor(diff / 3600000) } });
    return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' });
  }
</script>

<SettingsHeader title={$t('settingsUsage.title')} description={$t('settingsUsage.pageDescription')} />

{#if isLoading}
  <div class="flex items-center justify-center gap-3 py-16">
    <Spinner size={20} />
    <span class="text-xs text-base-content/70">{$t('settingsUsage.loadingUsage')}</span>
  </div>
{:else if !connected}
  <div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
    <p class="text-xs text-base-content/70">{$t('settingsUsage.connectToView')}</p>
    <a href="/settings/account" class="inline-block mt-3 text-sm font-medium text-primary hover:brightness-110 transition-all">
      {$t('settingsUsage.goToAccount')}
    </a>
  </div>
{:else}
  <div class="space-y-6">
    <!-- Current Plan -->
    {#if currentPlan !== 'free'}
      <section>
        <div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5 flex items-center justify-between">
          <div>
            <p class="text-sm font-medium text-base-content">{$t('settingsBilling.planTitle', { values: { plan: planName } })}</p>
            {#if subscription?.subscriptions?.length}
              {@const sub = subscription.subscriptions[0]}
              {#if sub.amountCents}
                <p class="text-xs text-base-content/50">{$t('settingsUsage.price', { values: { amount: Math.round(sub.amountCents / 100), interval: sub.interval === 'year' ? $t('common.yearShort') : $t('common.monthShort') } })}</p>
              {/if}
            {/if}
          </div>
          <a href="/pricing" class="text-sm text-primary font-medium hover:brightness-110 transition-all">{$t('settingsUsage.changePlanTitle')}</a>
        </div>
      </section>
    {/if}

    <!-- Plan Usage Limits -->
    <section>
      <div class="flex items-center justify-between mb-3">
        <h3 class="text-xs font-semibold uppercase tracking-wider text-base-content/50">{$t('settingsUsage.planLimitsShort')}</h3>
        <div class="flex items-center gap-2">
          {#if usage?.updatedAt}
            <span class="text-xs text-base-content/50 font-mono">{$t('settingsUsage.updated', { values: { time: formatUpdatedAt(usage.updatedAt) } })}</span>
          {/if}
          <button
            onclick={refresh}
            disabled={refreshing}
            class="flex items-center gap-1.5 text-xs text-base-content/50 hover:text-base-content transition-colors disabled:opacity-50 cursor-pointer bg-transparent border-none"
            title={$t('settingsUsage.refreshTitle')}
          >
            <RefreshCw class="w-3.5 h-3.5 {refreshing ? 'animate-spin' : ''}" />
          </button>
        </div>
      </div>
      <div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5 space-y-5">
        {#if usage?.session}
          <div>
            <div class="flex items-center justify-between mb-2">
              <div>
                <span class="text-sm font-medium text-base-content">{$t('settingsUsage.session')}</span>
                {#if usage.session.resetAt}
                  <span class="text-xs text-base-content/50 ml-2">{timeUntilReset(usage.session.resetAt)}</span>
                {/if}
              </div>
              <span class="text-xs text-base-content/50 font-mono tabular-nums">{$t('settingsUsage.percentUsed', { values: { percent: usage.session.percentUsed ?? 0 } })}</span>
            </div>
            <div class="h-2 rounded-full bg-base-content/10 overflow-hidden">
              <div
                class="h-full rounded-full transition-all {(usage.session.percentUsed ?? 0) > 80 ? 'bg-warning' : 'bg-primary'}"
                style="width: {Math.min(usage.session.percentUsed ?? 0, 100)}%"
              ></div>
            </div>
          </div>
        {/if}

        {#if usage?.weekly}
          <div>
            <div class="flex items-center justify-between mb-2">
              <div>
                <span class="text-sm font-medium text-base-content">{$t('settingsUsage.weekly')}</span>
                {#if usage.weekly.resetAt}
                  <span class="text-xs text-base-content/50 ml-2">{timeUntilReset(usage.weekly.resetAt)}</span>
                {/if}
              </div>
              <span class="text-xs text-base-content/50 font-mono tabular-nums">{$t('settingsUsage.percentUsed', { values: { percent: usage.weekly.percentUsed ?? 0 } })}</span>
            </div>
            <div class="h-2 rounded-full bg-base-content/10 overflow-hidden">
              <div
                class="h-full rounded-full transition-all {(usage.weekly.percentUsed ?? 0) > 80 ? 'bg-warning' : 'bg-primary'}"
                style="width: {Math.min(usage.weekly.percentUsed ?? 0, 100)}%"
              ></div>
            </div>
          </div>
        {/if}

        {#if !usage?.session && !usage?.weekly}
          <p class="text-xs text-base-content/50">{$t('settingsUsage.noUsageDataShort')}</p>
        {/if}
      </div>
    </section>

    <!-- Budget Balance -->
    {#if hasBudget}
      <section>
        <h3 class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-3">{$t('settingsUsage.balance')}</h3>
        <div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
          {#if usage?.budget?.activePool}
            <div class="flex items-center gap-2 mb-4">
              <span class="text-xs text-base-content/50">{$t('settingsUsage.activePool')}:</span>
              <span class="badge badge-primary badge-sm">{usage.budget.activePool}</span>
            </div>
          {/if}
          <div class="grid sm:grid-cols-3 gap-4">
            {#if usage?.budget && (usage.budget.freeAvailable ?? 0) > 0}
              <div>
                <p class="text-xs text-base-content/50">{$t('settingsUsage.freePoolLabel')}</p>
                <p class="text-lg font-bold text-base-content font-mono tabular-nums">{formatDollars(usage.budget.freeAvailable ?? 0)}</p>
              </div>
            {/if}
            {#if usage?.budget}
              <div>
                <p class="text-xs text-base-content/50">{$t('settingsUsage.giftPoolLabel')}</p>
                <p class="text-lg font-bold text-base-content font-mono tabular-nums">{formatDollars(usage.budget.giftAvailable ?? 0)}</p>
              </div>
              <div>
                <p class="text-xs text-base-content/50">{$t('settingsUsage.creditsPool')}</p>
                <p class="text-lg font-bold text-base-content font-mono tabular-nums">${((usage.budget.creditsCents ?? 0) / 100).toFixed(2)}</p>
              </div>
            {/if}
          </div>
        </div>
      </section>
    {/if}
  </div>
{/if}
