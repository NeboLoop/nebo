<script lang="ts">
	import { onMount } from 'svelte';
	import { Cloud, CheckCircle, XCircle, Loader2, LogOut, Bot, ExternalLink, Zap, AlertTriangle } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import webapi from '$lib/api/gocliRequest';
	import type * as components from '$lib/api/neboComponents';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import Modal from '$lib/components/ui/Modal.svelte';
	import GiveNebo from '$lib/components/GiveNebo.svelte';
	import { t } from 'svelte-i18n';

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

	// Delete account
	let showDeleteModal = $state(false);
	let deleteConfirmText = $state('');
	let deleteLoading = $state(false);
	const canDelete = $derived(deleteConfirmText === 'DELETE');

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
					formError = $t('settingsAccount.signInTimeout');
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
						formError = result.error ?? $t('settingsAccount.signInFailed');
						formLoading = false;
					} else if (result.status === 'expired') {
						clearTimeout(timeout);
						cleanup();
						formError = $t('settingsAccount.signInExpired');
						formLoading = false;
					}
				} catch {
					// polling error, keep trying
				}
			}, 2000);
		} catch (e: any) {
			formError = e?.message || $t('settingsAccount.startFailed');
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

	async function handleDeleteAccount() {
		if (!canDelete) return;
		deleteLoading = true;
		try {
			await api.deleteAccount({ password: '' });
			await api.neboLoopDisconnect();
			showDeleteModal = false;
			window.location.href = '/';
		} catch (e: any) {
			console.error('Delete failed:', e);
		} finally {
			deleteLoading = false;
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
		<h2 class="font-display text-xl font-bold text-base-content mb-1">{$t('settingsAccount.title')}</h2>
		<p class="text-base text-base-content/80">{$t('settingsAccount.description')}</p>
	</div>
	<button
		class="h-8 px-3 rounded-lg bg-base-content/5 border border-base-content/10 text-base font-medium text-base-content/80 hover:border-base-content/40 hover:text-base-content transition-colors flex items-center gap-1.5"
		onclick={() => webapi.get('/api/v1/neboloop/open')}
	>
		{$t('settingsAccount.neboloopCom')}
		<ExternalLink class="w-3.5 h-3.5" />
	</button>
</div>

{#if isLoading}
	<div class="flex items-center justify-center gap-3 py-16">
		<Spinner size={20} />
		<span class="text-base text-base-content/80">{$t('settingsAccount.loadingAccount')}</span>
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
							<p class="font-medium text-base">{$t('settingsAccount.neboloopAccount')}</p>
						{/if}
					</div>
				</div>
				<span class="text-base font-semibold text-success bg-success/10 px-2.5 py-1 rounded-full">{$t('common.connected')}</span>
			</div>
		</div>

		<!-- Bot Connection Status -->
		<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
			<div class="flex items-center gap-3">
				<div class="w-10 h-10 rounded-full flex items-center justify-center {botConnected ? 'bg-success/10' : 'bg-warning/10'}">
					<Bot class="w-5 h-5 {botConnected ? 'text-success' : 'text-warning'}" />
				</div>
				<div class="flex-1">
					<p class="font-medium text-base">{$t('settingsAccount.botConnection')}</p>
					{#if botConnected}
						<p class="text-base text-base-content/80">{botName || 'NeboLoop'}</p>
						{#if botId}
							<p class="text-base text-base-content/80 font-mono">{botId}</p>
						{/if}
					{:else}
						<p class="text-base text-warning">{$t('settingsAccount.waitingForConnection')}</p>
					{/if}
				</div>
				<span class="text-base font-semibold px-2.5 py-1 rounded-full {botConnected ? 'text-success bg-success/10' : 'text-warning bg-warning/10'}">
					{botConnected ? $t('common.online') : $t('common.offline')}
				</span>
			</div>
		</div>

		<!-- Give Nebo -->
		{#if accountConnected}
			<GiveNebo />
		{/if}

		<!-- Janus Usage -->
		{#if janusUsage && (janusUsage.session.limitTokens > 0 || janusUsage.weekly.limitTokens > 0)}
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
				<div class="flex items-center gap-3 mb-4">
					<Zap class="w-5 h-5 text-primary" />
					<div class="flex-1">
						<p class="font-medium text-base">{$t('settingsAccount.aiUsage')}</p>
					</div>
					<button
						class="h-7 px-2.5 rounded-lg bg-base-content/5 border border-base-content/10 text-base font-medium text-base-content/80 hover:border-base-content/40 hover:text-base-content transition-colors flex items-center gap-1"
						onclick={() => webapi.get('/api/v1/neboloop/open', { path: '/app/settings/billing' })}
					>
						{$t('settingsAccount.managePlan')}
						<ExternalLink class="w-3 h-3" />
					</button>
				</div>
				<div class="flex flex-col gap-3">
					{#if janusUsage.session.limitTokens > 0}
						<div>
							<div class="flex justify-between text-base text-base-content/80 mb-1.5">
								<span>{$t('settingsAccount.sessionUsed', { values: { percent: janusUsage.session.percentUsed } })}</span>
								{#if janusUsage.session.resetAt}
									{@const reset = new Date(janusUsage.session.resetAt)}
									{@const now = new Date()}
									{@const diffMs = reset.getTime() - now.getTime()}
									{@const diffH = Math.floor(diffMs / 3600000)}
									{@const diffM = Math.floor((diffMs % 3600000) / 60000)}
									<span>{$t('settingsAccount.resetsInTime', { values: { hours: diffH, minutes: diffM } })}</span>
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
								<span>{$t('settingsAccount.weeklyUsed', { values: { percent: janusUsage.weekly.percentUsed } })}</span>
								{#if janusUsage.weekly.resetAt}
									<span>{$t('settingsAccount.resetsDate', { values: { date: new Date(janusUsage.weekly.resetAt).toLocaleDateString(undefined, { weekday: 'short', month: 'short', day: 'numeric' }) } })}</span>
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
						{$t('settingsAccount.disconnectWarning')}
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
							{$t('settingsAccount.disconnect')}
						</button>
						<button
							type="button"
							class="h-9 px-4 rounded-full border border-base-content/10 text-base font-medium hover:bg-base-content/5 transition-colors"
							onclick={() => showDisconnectConfirm = false}
						>
							{$t('common.cancel')}
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
					{$t('settingsAccount.disconnectAccount')}
				</button>
			{/if}
		</div>

		<!-- Delete Account -->
		<div class="pt-4">
			<button
				type="button"
				class="text-sm text-base-content/40 hover:text-error transition-colors"
				onclick={() => { deleteConfirmText = ''; showDeleteModal = true; }}
			>
				{$t('settingsAccount.deleteAccount')}
			</button>
		</div>
	</div>
{:else}
	<!-- Not Connected State -->
	<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-8">
		<div class="flex flex-col items-center gap-4">
			<XCircle class="w-8 h-8 text-base-content/90" />
			<p class="text-base text-base-content/80 text-center max-w-sm">
				{$t('settingsAccount.connectDescription')}
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
					{$t('settingsAccount.waitingForSignIn')}
				{:else}
					{$t('settingsAccount.continueWithNeboLoop')}
				{/if}
			</button>
			{#if formLoading}
				<p class="text-base text-base-content/80">{$t('settingsAccount.completeInBrowser')}</p>
				<button
					type="button"
					class="text-base text-base-content/80 hover:text-base-content underline"
					onclick={() => { cleanup(); formLoading = false; }}
				>
					{$t('common.cancel')}
				</button>
			{/if}
		</div>
	</div>
{/if}

<!-- Delete Account Modal -->
<Modal bind:show={showDeleteModal} title={$t('settingsAccount.deleteModal.title')} size="sm">
	<div class="space-y-4">
		<div class="rounded-xl bg-error/10 border border-error/20 p-4">
			<div class="flex gap-3">
				<AlertTriangle class="w-5 h-5 text-error shrink-0 mt-0.5" />
				<div>
					<p class="text-base font-medium text-error">{$t('settingsAccount.deleteModal.permanent')}</p>
					<p class="text-sm text-error/80 mt-1">
						{$t('settingsAccount.deleteModal.description')}
					</p>
				</div>
			</div>
		</div>
		<div>
			<label class="block text-sm font-medium text-base-content/80 mb-1" for="confirm-delete">
				{$t('settingsAccount.deleteModal.typeToConfirm')}
			</label>
			<input
				id="confirm-delete"
				type="text"
				class="w-full h-11 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-base focus:outline-none focus:border-primary/50 transition-colors"
				placeholder={$t('settingsAccount.deleteModal.typeToConfirm')}
				bind:value={deleteConfirmText}
				onkeydown={(e) => { if (e.key === 'Enter' && canDelete) handleDeleteAccount(); }}
			/>
		</div>
	</div>
	{#snippet footer()}
		<button
			type="button"
			onclick={() => (showDeleteModal = false)}
			class="h-10 px-5 rounded-full border border-base-content/10 text-base font-medium hover:bg-base-content/5 transition-colors"
		>
			{$t('common.cancel')}
		</button>
		<button
			type="button"
			disabled={!canDelete || deleteLoading}
			onclick={handleDeleteAccount}
			class="h-10 px-5 rounded-full text-base font-bold transition-all flex items-center gap-2
				{canDelete ? 'bg-error text-error-content hover:brightness-110' : 'bg-base-content/10 text-base-content/40 cursor-not-allowed'}"
		>
			{#if deleteLoading}<Spinner size={14} />{/if}
			{$t('settingsAccount.deleteModal.deleteMyAccount')}
		</button>
	{/snippet}
</Modal>
