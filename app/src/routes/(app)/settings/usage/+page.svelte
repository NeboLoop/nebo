<script lang="ts">
	import { onMount } from 'svelte';
	import * as api from '$lib/api/nebo';
	import type { NeboLoopJanusUsageResponse } from '$lib/api/neboComponents';
	import Spinner from '$lib/components/ui/Spinner.svelte';

	let isLoading = $state(true);
	let usage = $state<NeboLoopJanusUsageResponse | null>(null);
	let connected = $state(false);

	onMount(async () => {
		try {
			const [status, usageData] = await Promise.all([
				api.neboLoopAccountStatus(),
				api.neboLoopJanusUsage()
			]);
			connected = status?.connected ?? false;
			usage = usageData;
		} catch {
			connected = false;
			usage = null;
		} finally {
			isLoading = false;
		}
	});

	function formatTokens(n: number): string {
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
			return `resets in ${d}d`;
		}
		return `resets in ${h}h ${m}m`;
	}
</script>

<div class="mb-6">
	<h2 class="font-display text-xl font-bold text-base-content mb-1">Usage</h2>
	<p class="text-base text-base-content/80">AI token usage for the current billing period</p>
</div>

{#if isLoading}
	<div class="flex items-center justify-center gap-3 py-16">
		<Spinner size={20} />
		<span class="text-base text-base-content/80">Loading usage...</span>
	</div>
{:else if !connected}
	<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
		<p class="text-base text-base-content/80">Connect your NeboLoop account to view usage.</p>
		<a href="/settings/account" class="inline-block mt-3 text-base font-medium text-primary hover:brightness-110 transition-all">
			Go to Account
		</a>
	</div>
{:else if !usage}
	<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
		<p class="text-base text-base-content/80">No usage data available.</p>
	</div>
{:else}
	<div class="space-y-6">
		<!-- Session Usage -->
		{#if usage.session.limitTokens > 0}
			<section>
				<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">Session</h3>
				<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
					<div class="flex items-end justify-between mb-3">
						<div>
							<p class="text-2xl font-bold text-base-content tabular-nums">{formatTokens(usage.session.usedTokens)}</p>
							<p class="text-base text-base-content/80">of {formatTokens(usage.session.limitTokens)} tokens</p>
						</div>
						<div class="text-right">
							<p class="text-base font-medium text-base-content tabular-nums">{usage.session.percentUsed}%</p>
							{#if usage.session.resetAt}
								<p class="text-base text-base-content/80">{timeUntilReset(usage.session.resetAt)}</p>
							{/if}
						</div>
					</div>
					<div class="h-2 rounded-full bg-base-content/10 overflow-hidden">
						<div
							class="h-full rounded-full transition-all {usage.session.percentUsed > 80 ? 'bg-warning' : 'bg-primary'}"
							style="width: {usage.session.percentUsed}%"
						></div>
					</div>
				</div>
			</section>
		{/if}

		<!-- Weekly Usage -->
		{#if usage.weekly.limitTokens > 0}
			<section>
				<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">Weekly</h3>
				<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
					<div class="flex items-end justify-between mb-3">
						<div>
							<p class="text-2xl font-bold text-base-content tabular-nums">{formatTokens(usage.weekly.usedTokens)}</p>
							<p class="text-base text-base-content/80">of {formatTokens(usage.weekly.limitTokens)} tokens</p>
						</div>
						<div class="text-right">
							<p class="text-base font-medium text-base-content tabular-nums">{usage.weekly.percentUsed}%</p>
							{#if usage.weekly.resetAt}
								<p class="text-base text-base-content/80">{timeUntilReset(usage.weekly.resetAt)}</p>
							{/if}
						</div>
					</div>
					<div class="h-2 rounded-full bg-base-content/10 overflow-hidden">
						<div
							class="h-full rounded-full transition-all {usage.weekly.percentUsed > 80 ? 'bg-warning' : 'bg-primary'}"
							style="width: {usage.weekly.percentUsed}%"
						></div>
					</div>
				</div>
			</section>
		{/if}

		<!-- Remaining -->
		<section>
			<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">Remaining</h3>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
				<div class="grid sm:grid-cols-2 gap-4">
					{#if usage.session.limitTokens > 0}
						<div>
							<p class="text-base text-base-content/80">Session</p>
							<p class="text-lg font-bold text-base-content tabular-nums">{formatTokens(usage.session.remainingTokens)}</p>
						</div>
					{/if}
					{#if usage.weekly.limitTokens > 0}
						<div>
							<p class="text-base text-base-content/80">Weekly</p>
							<p class="text-lg font-bold text-base-content tabular-nums">{formatTokens(usage.weekly.remainingTokens)}</p>
						</div>
					{/if}
				</div>
			</div>
		</section>
	</div>
{/if}
