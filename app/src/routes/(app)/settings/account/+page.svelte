<script lang="ts">
	import { onMount } from 'svelte';
	import { Cloud, CheckCircle, XCircle, Loader2, LogOut, Bot, ExternalLink, Zap } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import webapi from '$lib/api/gocliRequest';
	import type * as components from '$lib/api/neboComponents';
	import Spinner from '$lib/components/ui/Spinner.svelte';

	let isLoading = $state(true);
	let accountConnected = $state(false);
	let ownerEmail = $state('');
	let ownerName = $state('');
	let botConnected = $state(false);
	let botId = $state('');
	let botName = $state('');

	// OAuth state
	let formLoading = $state(false);
	let formError = $state('');
	let pendingState = $state('');
	let pollTimer = $state<ReturnType<typeof setInterval> | null>(null);

	// Janus usage
	let janusUsage = $state<components.NeboLoopJanusUsageResponse | null>(null);

	// Disconnect
	let showDisconnectConfirm = $state(false);
	let disconnecting = $state(false);

	async function loadStatus() {
		try {
			const [account, bot] = await Promise.all([
				api.neboLoopAccountStatus(),
				api.neboLoopStatus()
			]);
			accountConnected = account.connected;
			ownerEmail = account.email || '';
			ownerName = account.displayName || '';
			botConnected = bot.connected;
			botId = bot.botId || '';
			botName = bot.botName || '';

			if (account.connected) {
				try {
					janusUsage = await api.neboLoopJanusUsage();
				} catch {
					janusUsage = null;
				}
			}
		} catch (e) {
			console.error('Failed to load NeboLoop status:', e);
		} finally {
			isLoading = false;
		}
	}

	async function startOAuth() {
		formError = '';
		formLoading = true;
		try {
			const { state } = await api.neboLoopOAuthStart();
			pendingState = state;

			// Auto-timeout after 3 minutes
			const timeout = setTimeout(() => {
				if (formLoading) {
					cleanup();
					formError = 'Sign-in timed out. Please try again.';
					formLoading = false;
				}
			}, 3 * 60 * 1000);

			// Poll status until the OAuth flow completes in the browser
			pollTimer = setInterval(async () => {
				try {
					const result = await api.neboLoopOAuthStatus({ state: pendingState });
					if (result.status === 'complete') {
						clearTimeout(timeout);
						cleanup();
						formLoading = false;
						await loadStatus();
					} else if (result.status === 'error') {
						clearTimeout(timeout);
						cleanup();
						formError = result.error ?? 'Sign-in failed';
						formLoading = false;
					} else if (result.status === 'expired') {
						clearTimeout(timeout);
						cleanup();
						formError = 'Sign-in expired. Please try again.';
						formLoading = false;
					}
				} catch {
					// polling error, keep trying
				}
			}, 2000);
		} catch (e: any) {
			formError = e?.message || 'Failed to start sign-in';
			formLoading = false;
		}
	}

	function cleanup() {
		if (pollTimer) {
			clearInterval(pollTimer);
			pollTimer = null;
		}
		pendingState = '';
	}

	async function handleDisconnect() {
		disconnecting = true;
		try {
			await api.neboLoopDisconnect();
			showDisconnectConfirm = false;
			await loadStatus();
		} catch (e: any) {
			console.error('Disconnect failed:', e);
		} finally {
			disconnecting = false;
		}
	}

	onMount(() => {
		loadStatus();
		const h = () => loadStatus();
		window.addEventListener('nebo:plan_changed', h);
		return () => window.removeEventListener('nebo:plan_changed', h);
	});
</script>

<!-- Header -->
<div class="flex items-center justify-between mb-6">
	<div>
		<h2 class="font-display text-xl font-bold text-base-content mb-1">Account</h2>
		<p class="text-base text-base-content/80">NeboLoop AI, marketplace, and cloud channels</p>
	</div>
	<button
		class="h-8 px-3 rounded-lg bg-base-content/5 border border-base-content/10 text-base font-medium text-base-content/80 hover:border-base-content/40 hover:text-base-content transition-colors flex items-center gap-1.5"
		onclick={() => webapi.get('/api/v1/neboloop/open')}
	>
		NeboLoop.com
		<ExternalLink class="w-3.5 h-3.5" />
	</button>
</div>

{#if isLoading}
	<div class="flex items-center justify-center gap-3 py-16">
		<Spinner size={20} />
		<span class="text-base text-base-content/80">Loading account...</span>
	</div>
{:else if accountConnected}
	<!-- Connected State -->
	<div class="space-y-4">
		<!-- Account Info -->
		<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
			<div class="flex items-center justify-between">
				<div class="flex items-center gap-3">
					<div class="w-10 h-10 rounded-full bg-success/10 flex items-center justify-center">
						<CheckCircle class="w-5 h-5 text-success" />
					</div>
					<div>
						{#if ownerName || ownerEmail}
							{#if ownerName}<p class="font-medium text-base">{ownerName}</p>{/if}
							{#if ownerEmail}<p class="text-base text-base-content/80">{ownerEmail}</p>{/if}
						{:else}
							<p class="font-medium text-base">NeboLoop Account</p>
						{/if}
					</div>
				</div>
				<span class="text-base font-semibold text-success bg-success/10 px-2.5 py-1 rounded-full">Connected</span>
			</div>
		</div>

		<!-- Bot Connection Status -->
		<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
			<div class="flex items-center gap-3">
				<div class="w-10 h-10 rounded-full flex items-center justify-center {botConnected ? 'bg-success/10' : 'bg-warning/10'}">
					<Bot class="w-5 h-5 {botConnected ? 'text-success' : 'text-warning'}" />
				</div>
				<div class="flex-1">
					<p class="font-medium text-base">Bot Connection</p>
					{#if botConnected}
						<p class="text-base text-base-content/80">{botName || 'NeboLoop'}</p>
						{#if botId}
							<p class="text-base text-base-content/80 font-mono">{botId}</p>
						{/if}
					{:else}
						<p class="text-base text-warning">Waiting for NeboLoop connection</p>
					{/if}
				</div>
				<span class="text-base font-semibold px-2.5 py-1 rounded-full {botConnected ? 'text-success bg-success/10' : 'text-warning bg-warning/10'}">
					{botConnected ? 'Online' : 'Offline'}
				</span>
			</div>
		</div>

		<!-- Janus Usage -->
		{#if janusUsage && (janusUsage.session.limitTokens > 0 || janusUsage.weekly.limitTokens > 0)}
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
				<div class="flex items-center gap-3 mb-4">
					<Zap class="w-5 h-5 text-primary" />
					<div class="flex-1">
						<p class="font-medium text-base">AI Usage</p>
					</div>
					<button
						class="h-7 px-2.5 rounded-lg bg-base-content/5 border border-base-content/10 text-base font-medium text-base-content/80 hover:border-base-content/40 hover:text-base-content transition-colors flex items-center gap-1"
						onclick={() => webapi.get('/api/v1/neboloop/open', { path: '/app/settings/billing' })}
					>
						Manage plan
						<ExternalLink class="w-3 h-3" />
					</button>
				</div>
				<div class="flex flex-col gap-3">
					{#if janusUsage.session.limitTokens > 0}
						<div>
							<div class="flex justify-between text-base text-base-content/80 mb-1.5">
								<span>Session: {janusUsage.session.percentUsed}% used</span>
								{#if janusUsage.session.resetAt}
									{@const reset = new Date(janusUsage.session.resetAt)}
									{@const now = new Date()}
									{@const diffMs = reset.getTime() - now.getTime()}
									{@const diffH = Math.floor(diffMs / 3600000)}
									{@const diffM = Math.floor((diffMs % 3600000) / 60000)}
									<span>Resets in {diffH}h {diffM}m</span>
								{/if}
							</div>
							<div class="h-1.5 rounded-full bg-base-content/10 overflow-hidden">
								<div
									class="h-full rounded-full transition-all {janusUsage.session.percentUsed > 80 ? 'bg-warning' : 'bg-primary'}"
									style="width: {janusUsage.session.percentUsed}%"
								></div>
							</div>
						</div>
					{/if}
					{#if janusUsage.weekly.limitTokens > 0}
						<div>
							<div class="flex justify-between text-base text-base-content/80 mb-1.5">
								<span>Weekly: {janusUsage.weekly.percentUsed}% used</span>
								{#if janusUsage.weekly.resetAt}
									<span>Resets {new Date(janusUsage.weekly.resetAt).toLocaleDateString(undefined, { weekday: 'short', month: 'short', day: 'numeric' })}</span>
								{/if}
							</div>
							<div class="h-1.5 rounded-full bg-base-content/10 overflow-hidden">
								<div
									class="h-full rounded-full transition-all {janusUsage.weekly.percentUsed > 80 ? 'bg-warning' : 'bg-primary'}"
									style="width: {janusUsage.weekly.percentUsed}%"
								></div>
							</div>
						</div>
					{/if}
				</div>
			</div>
		{/if}

		<!-- Disconnect -->
		<div class="pt-2">
			{#if showDisconnectConfirm}
				<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
					<p class="text-base text-base-content/80 mb-4">
						This will disconnect your NeboLoop account and stop all cloud services (AI, marketplace, channels).
					</p>
					<div class="flex gap-2">
						<button
							type="button"
							class="h-9 px-4 rounded-full bg-error text-white text-base font-bold hover:brightness-110 transition-all disabled:opacity-30 flex items-center"
							onclick={handleDisconnect}
							disabled={disconnecting}
						>
							{#if disconnecting}
								<Loader2 class="w-4 h-4 animate-spin mr-1.5" />
							{/if}
							Disconnect
						</button>
						<button
							type="button"
							class="h-9 px-4 rounded-full border border-base-content/10 text-base font-medium hover:bg-base-content/5 transition-colors"
							onclick={() => showDisconnectConfirm = false}
						>
							Cancel
						</button>
					</div>
				</div>
			{:else}
				<button
					type="button"
					class="flex items-center gap-1.5 text-base text-base-content/80 hover:text-error transition-colors"
					onclick={() => showDisconnectConfirm = true}
				>
					<LogOut class="w-4 h-4" />
					Disconnect Account
				</button>
			{/if}
		</div>
	</div>
{:else}
	<!-- Not Connected State -->
	<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-8">
		<div class="flex flex-col items-center gap-4">
			<XCircle class="w-8 h-8 text-base-content/90" />
			<p class="text-base text-base-content/80 text-center max-w-sm">
				Connect to NeboLoop for AI, the marketplace, and cloud channels.
				You can use Google, Apple, or email.
			</p>

			{#if formError}
				<div class="w-full rounded-xl bg-error/10 border border-error/20 px-4 py-3 text-base text-error">
					{formError}
				</div>
			{/if}

			<button
				type="button"
				class="h-11 px-8 rounded-full bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all disabled:opacity-30 flex items-center"
				onclick={startOAuth}
				disabled={formLoading}
			>
				{#if formLoading}
					<Loader2 class="w-5 h-5 mr-2 animate-spin" />
					Waiting for sign-in...
				{:else}
					Continue with NeboLoop
				{/if}
			</button>
			{#if formLoading}
				<p class="text-base text-base-content/80">Complete sign-in in your browser</p>
				<button
					type="button"
					class="text-base text-base-content/80 hover:text-base-content underline"
					onclick={() => { cleanup(); formLoading = false; }}
				>
					Cancel
				</button>
			{/if}
		</div>
	</div>
{/if}
