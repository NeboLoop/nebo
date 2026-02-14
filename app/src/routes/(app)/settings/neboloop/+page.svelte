<script lang="ts">
	import { onMount } from 'svelte';
	import { Cloud, CheckCircle, XCircle, Loader2, LogOut, Bot } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
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

	// Auth form state
	let tab = $state<'login' | 'signup'>('login');
	let formLoading = $state(false);
	let formError = $state('');

	// Login fields
	let loginEmail = $state('');
	let loginPassword = $state('');

	// Signup fields
	let signupName = $state('');
	let signupEmail = $state('');
	let signupPassword = $state('');
	let signupConfirm = $state('');

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
		} catch (e) {
			console.error('Failed to load NeboLoop status:', e);
		} finally {
			isLoading = false;
		}
	}

	async function handleLogin() {
		if (!loginEmail || !loginPassword) {
			formError = 'Please fill in all fields.';
			return;
		}
		formError = '';
		formLoading = true;
		try {
			await api.neboLoopLogin({ email: loginEmail, password: loginPassword });
			loginEmail = '';
			loginPassword = '';
			await loadStatus();
		} catch (e: any) {
			formError = e?.message || 'Login failed. Please check your credentials.';
		} finally {
			formLoading = false;
		}
	}

	async function handleSignup() {
		if (!signupName || !signupEmail || !signupPassword) {
			formError = 'Please fill in all fields.';
			return;
		}
		if (signupPassword !== signupConfirm) {
			formError = 'Passwords do not match.';
			return;
		}
		formError = '';
		formLoading = true;
		try {
			await api.neboLoopRegister({
				email: signupEmail,
				displayName: signupName,
				password: signupPassword
			});
			signupName = '';
			signupEmail = '';
			signupPassword = '';
			signupConfirm = '';
			await loadStatus();
		} catch (e: any) {
			formError = e?.message || 'Registration failed. Please try again.';
		} finally {
			formLoading = false;
		}
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
	<div class="flex items-center gap-3 mb-6">
		<div class="w-10 h-10 rounded-xl bg-accent/10 flex items-center justify-center">
			<Cloud class="w-5 h-5 text-accent" />
		</div>
		<div>
			<h2 class="text-lg font-semibold">NeboLoop</h2>
			<p class="text-sm text-base-content/60">Janus AI, app store, and cloud channels</p>
		</div>
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
							<p class="text-sm text-base-content/60">{botName || botId}</p>
						{:else}
							<p class="text-sm text-warning">Not connected — MQTT credentials missing</p>
						{/if}
					</div>
					<span class="badge badge-sm {botConnected ? 'badge-success' : 'badge-warning'}">
						{botConnected ? 'Online' : 'Offline'}
					</span>
				</div>
			</Card>

			<!-- Disconnect -->
			<div class="pt-2">
				{#if showDisconnectConfirm}
					<Card>
						<p class="text-sm text-base-content/70 mb-3">
							This will disconnect your NeboLoop account and stop all cloud services (Janus AI, app store, channels).
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
			<div class="flex flex-col items-center gap-2 mb-6">
				<XCircle class="w-8 h-8 text-base-content/30" />
				<p class="text-sm text-base-content/60 text-center">
					Connect to NeboLoop for Janus AI, the app store, and cloud channels.
				</p>
			</div>

			<!-- Tabs -->
			<div role="tablist" class="tabs tabs-bordered mb-6">
				<button
					role="tab"
					class="tab"
					class:tab-active={tab === 'login'}
					onclick={() => { tab = 'login'; formError = ''; }}
				>
					Log In
				</button>
				<button
					role="tab"
					class="tab"
					class:tab-active={tab === 'signup'}
					onclick={() => { tab = 'signup'; formError = ''; }}
				>
					Sign Up
				</button>
			</div>

			{#if formError}
				<div class="alert alert-error mb-4">
					<span>{formError}</span>
				</div>
			{/if}

			{#if tab === 'login'}
				<form onsubmit={(e) => { e.preventDefault(); handleLogin(); }} class="space-y-3">
					<input
						type="email"
						placeholder="Email"
						class="input input-bordered w-full"
						bind:value={loginEmail}
						disabled={formLoading}
					/>
					<input
						type="password"
						placeholder="Password"
						class="input input-bordered w-full"
						bind:value={loginPassword}
						disabled={formLoading}
					/>
					<Button type="primary" htmlType="submit" class="w-full" disabled={formLoading}>
						{#if formLoading}
							<Loader2 class="w-4 h-4 animate-spin mr-2" />
							Logging in...
						{:else}
							Log In
						{/if}
					</Button>
				</form>
			{:else}
				<form onsubmit={(e) => { e.preventDefault(); handleSignup(); }} class="space-y-3">
					<input
						type="text"
						placeholder="Display Name"
						class="input input-bordered w-full"
						bind:value={signupName}
						disabled={formLoading}
					/>
					<input
						type="email"
						placeholder="Email"
						class="input input-bordered w-full"
						bind:value={signupEmail}
						disabled={formLoading}
					/>
					<input
						type="password"
						placeholder="Password"
						class="input input-bordered w-full"
						bind:value={signupPassword}
						disabled={formLoading}
					/>
					<input
						type="password"
						placeholder="Confirm Password"
						class="input input-bordered w-full"
						bind:value={signupConfirm}
						disabled={formLoading}
					/>
					<Button type="primary" htmlType="submit" class="w-full" disabled={formLoading}>
						{#if formLoading}
							<Loader2 class="w-4 h-4 animate-spin mr-2" />
							Creating Account...
						{:else}
							Create Account
						{/if}
					</Button>
				</form>
			{/if}
		</Card>
	{/if}
</div>
