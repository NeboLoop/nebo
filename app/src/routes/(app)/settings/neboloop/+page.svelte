<script lang="ts">
	import { onMount } from 'svelte';
	import { Cloud, CheckCircle, XCircle, Loader2, LogOut, Bot, ExternalLink, Zap } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import type * as components from '$lib/api/neboComponents';
	import Card from '$lib/components/ui/Card.svelte';
	import Button from '$lib/components/ui/Button.svelte';
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
	});
</script>

<div class="max-w-2xl">
	<!-- Header -->
	<div class="flex items-center justify-between mb-6">
		<div>
			<h2 class="font-display text-xl font-bold text-base-content mb-1">NeboLoop</h2>
			<p class="text-sm text-base-content/60">Janus AI, marketplace, and cloud channels</p>
		</div>
		<a href="https://neboloop.com" target="_blank" rel="noopener noreferrer" class="btn btn-ghost btn-sm gap-1 text-base-content/60">
			neboloop.com
			<ExternalLink class="w-3.5 h-3.5" />
		</a>
	</div>

	{#if isLoading}
		<div class="flex justify-center py-12">
			<Spinner />
		</div>
	{:else if accountConnected}
		<!-- Connected State -->
		<div class="space-y-4">
			<Card>
				<div class="flex items-center justify-between">
					<div class="flex items-center gap-3">
						<div class="w-10 h-10 rounded-full bg-success/10 flex items-center justify-center">
							<CheckCircle class="w-5 h-5 text-success" />
						</div>
						<div>
							<p class="font-medium">{ownerName}</p>
							<p class="text-sm text-base-content/60">{ownerEmail}</p>
						</div>
					</div>
					<span class="badge badge-success badge-sm">Connected</span>
				</div>
			</Card>

			<!-- Bot Connection Status -->
			<Card>
				<div class="flex items-center gap-3">
					<div class="w-10 h-10 rounded-full flex items-center justify-center {botConnected ? 'bg-success/10' : 'bg-warning/10'}">
						<Bot class="w-5 h-5 {botConnected ? 'text-success' : 'text-warning'}" />
					</div>
					<div class="flex-1">
						<p class="font-medium">Bot Connection</p>
						{#if botConnected}
							<p class="text-sm text-base-content/60">{botName || 'NeboLoop'}</p>
							{#if botId}
								<p class="text-xs text-base-content/40 font-mono">{botId}</p>
							{/if}
						{:else}
							<p class="text-sm text-warning">Waiting for NeboLoop connection</p>
						{/if}
					</div>
					<span class="badge badge-sm {botConnected ? 'badge-success' : 'badge-warning'}">
						{botConnected ? 'Online' : 'Offline'}
					</span>
				</div>
			</Card>

			<!-- Janus Usage -->
			{#if janusUsage && (janusUsage.session.limitTokens > 0 || janusUsage.weekly.limitTokens > 0)}
				<Card>
					<div class="flex items-center gap-3 mb-3">
						<Zap class="w-5 h-5 text-primary" />
						<div class="flex-1">
							<p class="font-medium">Janus AI Usage</p>
						</div>
						<a href="https://neboloop.com" target="_blank" rel="noopener noreferrer" class="btn btn-ghost btn-xs gap-1 text-base-content/50">
							Upgrade
							<ExternalLink class="w-3 h-3" />
						</a>
					</div>
					<div class="flex flex-col gap-2">
						{#if janusUsage.session.limitTokens > 0}
							<div>
								<div class="flex justify-between text-xs text-base-content/60 mb-1">
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
								<progress
									class="progress w-full {janusUsage.session.percentUsed > 80 ? 'progress-warning' : 'progress-primary'}"
									value={janusUsage.session.percentUsed}
									max="100"
								></progress>
							</div>
						{/if}
						{#if janusUsage.weekly.limitTokens > 0}
							<div>
								<div class="flex justify-between text-xs text-base-content/60 mb-1">
									<span>Weekly: {janusUsage.weekly.percentUsed}% used</span>
									{#if janusUsage.weekly.resetAt}
										<span>Resets {new Date(janusUsage.weekly.resetAt).toLocaleDateString(undefined, { weekday: 'short', month: 'short', day: 'numeric' })}</span>
									{/if}
								</div>
								<progress
									class="progress w-full {janusUsage.weekly.percentUsed > 80 ? 'progress-warning' : 'progress-primary'}"
									value={janusUsage.weekly.percentUsed}
									max="100"
								></progress>
							</div>
						{/if}
					</div>
				</Card>
			{/if}

			<!-- Disconnect -->
			<div class="pt-2">
				{#if showDisconnectConfirm}
					<Card>
						<p class="text-sm text-base-content/70 mb-3">
							This will disconnect your NeboLoop account and stop all cloud services (Janus AI, marketplace, channels).
						</p>
						<div class="flex gap-2">
							<Button type="danger" size="sm" onclick={handleDisconnect} disabled={disconnecting}>
								{#if disconnecting}
									<Loader2 class="w-4 h-4 animate-spin mr-1" />
								{/if}
								Disconnect
							</Button>
							<Button type="ghost" size="sm" onclick={() => showDisconnectConfirm = false}>
								Cancel
							</Button>
						</div>
					</Card>
				{:else}
					<Button type="ghost" size="sm" onclick={() => showDisconnectConfirm = true}>
						<LogOut class="w-4 h-4 mr-1" />
						Disconnect Account
					</Button>
				{/if}
			</div>
		</div>
	{:else}
		<!-- Not Connected State -->
		<Card>
			<div class="flex flex-col items-center gap-4 py-4">
				<XCircle class="w-8 h-8 text-base-content/30" />
				<p class="text-sm text-base-content/60 text-center max-w-sm">
					Connect to NeboLoop for Janus AI, the marketplace, and cloud channels.
					You can use Google, Apple, or email.
				</p>

				{#if formError}
					<div class="alert alert-error w-full">
						<span>{formError}</span>
					</div>
				{/if}

				<Button type="primary" size="lg" onclick={startOAuth} disabled={formLoading}>
					{#if formLoading}
						<Loader2 class="w-5 h-5 mr-2 animate-spin" />
						Waiting for sign-in...
					{:else}
						Continue with NeboLoop
					{/if}
				</Button>
				{#if formLoading}
					<p class="text-sm text-base-content/50">Complete sign-in in your browser</p>
					<button
						type="button"
						class="text-sm text-base-content/50 hover:text-base-content underline"
						onclick={() => { cleanup(); formLoading = false; }}
					>
						Cancel
					</button>
				{/if}
			</div>
		</Card>
	{/if}
</div>
