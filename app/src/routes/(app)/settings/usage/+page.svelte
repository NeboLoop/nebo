<script lang="ts">
	import { onMount } from 'svelte';
	import { RefreshCw } from 'lucide-svelte';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import * as api from '$lib/api/nebo';
	import type { NeboLoopJanusUsageResponse, NeboLoopAccountStatusResponse } from '$lib/api/neboComponents';
	import { t } from 'svelte-i18n';

	let isLoading = $state(true);
	let refreshing = $state(false);
	let usage = $state<NeboLoopJanusUsageResponse | null>(null);
	let accountStatus = $state<NeboLoopAccountStatusResponse | null>(null);
	let subscription = $state<{ plan: string; subscriptions: any[] } | null>(null);
	let connected = $state(false);

	const currentPlan = $derived((subscription?.plan || accountStatus?.plan || 'free').toLowerCase());
	const planName = $derived(currentPlan.charAt(0).toUpperCase() + currentPlan.slice(1));

	onMount(async () => {
		try {
			accountStatus = await api.neboLoopAccountStatus();
			connected = accountStatus?.connected || false;
			if (connected) {
				const [usageResp, subResp] = await Promise.allSettled([
					api.neboLoopJanusUsage(),
					api.neboLoopBillingSubscription()
				]);
				if (usageResp.status === 'fulfilled') usage = usageResp.value;
				if (subResp.status === 'fulfilled') subscription = subResp.value;
			}
		} catch { /* ignore */ }
		isLoading = false;
	});

	async function refresh() {
		if (refreshing) return;
		refreshing = true;
		try {
			usage = await api.neboLoopJanusUsageRefresh();
		} catch { /* ignore */ }
		refreshing = false;
	}

	function formatTokens(n: number): string {
		if (n >= 1_000_000_000) return `${(n / 1_000_000_000).toFixed(1)}B`;
		if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
		if (n >= 1_000) return `${(n / 1_000).toFixed(0)}K`;
		return n.toLocaleString();
	}

	function timeUntilReset(resetAt?: string): string {
		if (!resetAt) return '';
		const diff = new Date(resetAt).getTime() - Date.now();
		if (diff <= 0) return 'resetting...';
		const h = Math.floor(diff / 3600000);
		const m = Math.floor((diff % 3600000) / 60000);
		if (h > 24) {
			const d = Math.floor(h / 24);
			return $t('settingsUsage.resetsInDays', { values: { days: d } });
		}
		return $t('settingsUsage.resetsInTime', { values: { hours: h, minutes: m } });
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

<div class="mb-6">
	<h2 class="font-display text-xl font-bold text-base-content mb-1">{$t('settingsUsage.title')}</h2>
	<p class="text-base text-base-content/80">{$t('settingsUsage.description')}</p>
</div>

{#if isLoading}
	<div class="flex items-center justify-center gap-3 py-16">
		<Spinner size={20} />
		<span class="text-base text-base-content/80">{$t('settingsUsage.loadingUsage')}</span>
	</div>
{:else if !connected}
	<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
		<p class="text-base text-base-content/80">{$t('settingsUsage.connectForUsage')}</p>
		<a href="/settings/account" class="inline-block mt-3 text-base font-medium text-primary hover:brightness-110 transition-all">
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
						<p class="text-base font-medium text-base-content">{$t('settingsUsage.planName', { values: { plan: planName } })}</p>
						{#if subscription?.subscriptions?.length}
							{@const sub = subscription.subscriptions[0]}
							{#if sub.amountCents}
								<p class="text-sm text-base-content/50">{$t('settingsUsage.price', { values: { amount: Math.round(sub.amountCents / 100), interval: sub.interval === 'year' ? 'yr' : 'mo' } })}</p>
							{/if}
						{/if}
					</div>
					<a href="/upgrade" class="text-sm text-primary font-medium hover:brightness-110 transition-all">{$t('settingsUsage.changePlan')}</a>
				</div>
			</section>
		{/if}

		<!-- Plan Usage Limits -->
		<section>
			<div class="flex items-center justify-between mb-3">
				<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider">{$t('settingsUsage.planLimits')}</h3>
				<div class="flex items-center gap-2">
					{#if usage?.updatedAt}
						<span class="text-xs text-base-content/40">{$t('settingsUsage.updated', { values: { time: formatUpdatedAt(usage.updatedAt) } })}</span>
					{/if}
					<button
						onclick={refresh}
						disabled={refreshing}
						class="flex items-center gap-1.5 text-xs text-base-content/50 hover:text-base-content transition-colors disabled:opacity-50"
						title="Refresh usage from server"
					>
						<RefreshCw class="w-3.5 h-3.5 {refreshing ? 'animate-spin' : ''}" />
					</button>
				</div>
			</div>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5 space-y-5">
				{#if usage?.session}
					<!-- Session -->
					<div>
						<div class="flex items-center justify-between mb-2">
							<div>
								<span class="text-base font-medium text-base-content">{$t('settingsUsage.session')}</span>
								{#if usage.session.resetAt}
									<span class="text-sm text-base-content/50 ml-2">{timeUntilReset(usage.session.resetAt)}</span>
								{/if}
							</div>
							<span class="text-sm text-base-content/60 tabular-nums">{$t('settingsUsage.percentUsed', { values: { percent: usage.session.percentUsed } })}</span>
						</div>
						<div class="h-2 rounded-full bg-base-content/10 overflow-hidden mb-1">
							<div
								class="h-full rounded-full transition-all {usage.session.percentUsed > 80 ? 'bg-warning' : 'bg-primary'}"
								style="width: {Math.min(usage.session.percentUsed, 100)}%"
							></div>
						</div>
						<span class="text-sm text-base-content/40 tabular-nums">{$t('settingsUsage.usageCount', { values: { used: formatTokens(usage.session.usedTokens), limit: formatTokens(usage.session.limitTokens) } })}</span>
					</div>
				{/if}

				{#if usage?.weekly}
					<!-- Weekly -->
					<div>
						<div class="flex items-center justify-between mb-2">
							<div>
								<span class="text-base font-medium text-base-content">{$t('settingsUsage.weekly')}</span>
								{#if usage.weekly.resetAt}
									<span class="text-sm text-base-content/50 ml-2">{timeUntilReset(usage.weekly.resetAt)}</span>
								{/if}
							</div>
							<span class="text-sm text-base-content/60 tabular-nums">{$t('settingsUsage.percentUsed', { values: { percent: usage.weekly.percentUsed } })}</span>
						</div>
						<div class="h-2 rounded-full bg-base-content/10 overflow-hidden mb-1">
							<div
								class="h-full rounded-full transition-all {usage.weekly.percentUsed > 80 ? 'bg-warning' : 'bg-primary'}"
								style="width: {Math.min(usage.weekly.percentUsed, 100)}%"
							></div>
						</div>
						<span class="text-sm text-base-content/40 tabular-nums">{$t('settingsUsage.usageCount', { values: { used: formatTokens(usage.weekly.usedTokens), limit: formatTokens(usage.weekly.limitTokens) } })}</span>
					</div>
				{/if}

				{#if !usage?.session && !usage?.weekly}
					<p class="text-base text-base-content/60">{$t('settingsUsage.noUsageData')}</p>
				{/if}
			</div>
		</section>

		<!-- Extra Usage -->
		<section>
			<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">{$t('settingsUsage.extraUsage')}</h3>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
				<p class="text-base text-base-content/80">
					{$t('settingsUsage.extraUsageDesc')}
				</p>
				<p class="text-sm text-base-content/50 mt-2">
					{$t('settingsUsage.manageCredits')}
				</p>
			</div>
		</section>
	</div>
{/if}
