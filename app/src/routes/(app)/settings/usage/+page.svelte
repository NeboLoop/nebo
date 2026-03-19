<script lang="ts">
	import { onMount } from 'svelte';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import * as api from '$lib/api/nebo';
	import type { NeboLoopJanusUsageResponse } from '$lib/api/neboComponents';

	let isLoading = $state(true);
	let usage = $state<NeboLoopJanusUsageResponse | null>(null);
	let connected = $state(false);

	onMount(async () => {
		try {
			const status = await api.neboLoopAccountStatus();
			connected = status?.connected || false;
			if (connected) {
				usage = await api.neboLoopJanusUsage();
			}
		} catch { /* ignore */ }
		isLoading = false;
	});

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
			return `Resets in ${d}d`;
		}
		return `Resets in ${h}h ${m}m`;
	}
</script>

<div class="mb-6">
	<h2 class="font-display text-xl font-bold text-base-content mb-1">Usage</h2>
	<p class="text-base text-base-content/80">Plan usage limits and token consumption</p>
</div>

{#if isLoading}
	<div class="flex items-center justify-center gap-3 py-16">
		<Spinner size={20} />
		<span class="text-base text-base-content/80">Loading usage...</span>
	</div>
{:else if !connected}
	<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
		<p class="text-base text-base-content/80">Connect your NeboLoop account to see usage.</p>
		<a href="/settings/account" class="inline-block mt-3 text-base font-medium text-primary hover:brightness-110 transition-all">
			Go to Account
		</a>
	</div>
{:else}
	<div class="space-y-6">
		<!-- Plan Usage Limits -->
		<section>
			<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">Plan Usage Limits</h3>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5 space-y-5">
				{#if usage?.session}
					<!-- Session -->
					<div>
						<div class="flex items-center justify-between mb-2">
							<div>
								<span class="text-base font-medium text-base-content">Session</span>
								{#if usage.session.resetAt}
									<span class="text-sm text-base-content/50 ml-2">{timeUntilReset(usage.session.resetAt)}</span>
								{/if}
							</div>
							<span class="text-sm text-base-content/60 tabular-nums">{usage.session.percentUsed}% used</span>
						</div>
						<div class="h-2 rounded-full bg-base-content/10 overflow-hidden mb-1">
							<div
								class="h-full rounded-full transition-all {usage.session.percentUsed > 80 ? 'bg-warning' : 'bg-primary'}"
								style="width: {Math.min(usage.session.percentUsed, 100)}%"
							></div>
						</div>
						<span class="text-sm text-base-content/40 tabular-nums">{formatTokens(usage.session.usedTokens)} / {formatTokens(usage.session.limitTokens)}</span>
					</div>
				{/if}

				{#if usage?.weekly}
					<!-- Weekly -->
					<div>
						<div class="flex items-center justify-between mb-2">
							<div>
								<span class="text-base font-medium text-base-content">Weekly</span>
								{#if usage.weekly.resetAt}
									<span class="text-sm text-base-content/50 ml-2">{timeUntilReset(usage.weekly.resetAt)}</span>
								{/if}
							</div>
							<span class="text-sm text-base-content/60 tabular-nums">{usage.weekly.percentUsed}% used</span>
						</div>
						<div class="h-2 rounded-full bg-base-content/10 overflow-hidden mb-1">
							<div
								class="h-full rounded-full transition-all {usage.weekly.percentUsed > 80 ? 'bg-warning' : 'bg-primary'}"
								style="width: {Math.min(usage.weekly.percentUsed, 100)}%"
							></div>
						</div>
						<span class="text-sm text-base-content/40 tabular-nums">{formatTokens(usage.weekly.usedTokens)} / {formatTokens(usage.weekly.limitTokens)}</span>
					</div>
				{/if}

				{#if !usage?.session && !usage?.weekly}
					<p class="text-base text-base-content/60">No usage data available yet. Usage tracking starts when you use Nebo AI.</p>
				{/if}
			</div>
		</section>

		<!-- Extra Usage -->
		<section>
			<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">Extra Usage</h3>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
				<p class="text-base text-base-content/80">
					When you hit your plan limit, extra usage keeps you going. Credits are deducted automatically.
				</p>
				<p class="text-sm text-base-content/50 mt-2">
					Manage credits in <a href="/settings/billing" class="text-primary hover:brightness-110">Billing</a>.
				</p>
			</div>
		</section>
	</div>
{/if}
