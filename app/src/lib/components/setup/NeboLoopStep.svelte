<!--
  NeboLoop Onboarding Step
  Sign up / Log in / Skip for NeboLoop marketplace connection
  Uses DaisyUI tabs and form components
-->

<script lang="ts">
	import { StepCard, StepNavigation } from '$lib/components/setup';
	import { neboLoopRegister, neboLoopLogin, neboLoopAccountStatus } from '$lib/api';
	import { CircleCheck, Store } from 'lucide-svelte';

	let {
		onback,
		onnext,
		onskip
	}: {
		onback?: () => void;
		onnext?: () => void;
		onskip?: () => void;
	} = $props();

	let activeTab = $state<'signup' | 'login'>('signup');
	let loading = $state(false);
	let error = $state('');
	let connected = $state(false);
	let connectedEmail = $state('');

	// Sign up fields
	let signupEmail = $state('');
	let signupName = $state('');
	let signupPassword = $state('');
	let signupConfirm = $state('');

	// Login fields
	let loginEmail = $state('');
	let loginPassword = $state('');

	// Check if already connected on mount
	$effect(() => {
		checkStatus();
	});

	async function checkStatus() {
		try {
			const status = await neboLoopAccountStatus();
			if (status.connected) {
				connected = true;
				connectedEmail = status.email ?? '';
			}
		} catch {
			// Not connected, that's fine
		}
	}

	async function handleSignup() {
		error = '';
		if (!signupEmail || !signupName || !signupPassword) {
			error = 'All fields are required';
			return;
		}
		if (signupPassword !== signupConfirm) {
			error = 'Passwords do not match';
			return;
		}
		if (signupPassword.length < 8) {
			error = 'Password must be at least 8 characters';
			return;
		}

		loading = true;
		try {
			const resp = await neboLoopRegister({
				email: signupEmail,
				displayName: signupName,
				password: signupPassword
			});
			connected = true;
			connectedEmail = resp.email;
		} catch (e: any) {
			error = e?.message || 'Registration failed. Please try again.';
		} finally {
			loading = false;
		}
	}

	async function handleLogin() {
		error = '';
		if (!loginEmail || !loginPassword) {
			error = 'Email and password are required';
			return;
		}

		loading = true;
		try {
			const resp = await neboLoopLogin({
				email: loginEmail,
				password: loginPassword
			});
			connected = true;
			connectedEmail = resp.email;
		} catch (e: any) {
			error = e?.message || 'Login failed. Please check your credentials.';
		} finally {
			loading = false;
		}
	}
</script>

<StepCard
	title="NeboLoop"
	description="Connect to the NeboLoop marketplace to install apps, skills, and AI providers for your agent."
>
	{#if connected}
		<div class="flex flex-col items-center gap-4 py-6">
			<CircleCheck class="h-12 w-12 text-success" />
			<p class="text-lg font-medium">Connected to NeboLoop</p>
			<p class="text-base-content/70">{connectedEmail}</p>
		</div>
		<StepNavigation showBack onnext={onnext} onback={onback} nextLabel="Continue" />
	{:else}
		<div role="tablist" class="tabs tabs-bordered mb-6">
			<button
				role="tab"
				class="tab"
				class:tab-active={activeTab === 'signup'}
				onclick={() => { activeTab = 'signup'; error = ''; }}
			>
				Sign Up
			</button>
			<button
				role="tab"
				class="tab"
				class:tab-active={activeTab === 'login'}
				onclick={() => { activeTab = 'login'; error = ''; }}
			>
				Log In
			</button>
		</div>

		{#if error}
			<div class="alert alert-error mb-4">
				<span>{error}</span>
			</div>
		{/if}

		{#if activeTab === 'signup'}
			<form onsubmit={(e) => { e.preventDefault(); handleSignup(); }} class="flex flex-col gap-3">
				<label class="floating-label">
					<span>Display Name</span>
					<input
						type="text"
						placeholder="Display Name"
						class="input input-bordered w-full"
						bind:value={signupName}
						disabled={loading}
					/>
				</label>
				<label class="floating-label">
					<span>Email</span>
					<input
						type="email"
						placeholder="Email"
						class="input input-bordered w-full"
						bind:value={signupEmail}
						disabled={loading}
					/>
				</label>
				<label class="floating-label">
					<span>Password</span>
					<input
						type="password"
						placeholder="Password"
						class="input input-bordered w-full"
						bind:value={signupPassword}
						disabled={loading}
					/>
				</label>
				<label class="floating-label">
					<span>Confirm Password</span>
					<input
						type="password"
						placeholder="Confirm Password"
						class="input input-bordered w-full"
						bind:value={signupConfirm}
						disabled={loading}
					/>
				</label>
				<StepNavigation
					showBack
					showSkip
					onback={onback}
					onskip={onskip}
					onnext={handleSignup}
					nextLabel="Create Account"
					{loading}
					class="mt-2"
				/>
			</form>
		{:else}
			<form onsubmit={(e) => { e.preventDefault(); handleLogin(); }} class="flex flex-col gap-3">
				<label class="floating-label">
					<span>Email</span>
					<input
						type="email"
						placeholder="Email"
						class="input input-bordered w-full"
						bind:value={loginEmail}
						disabled={loading}
					/>
				</label>
				<label class="floating-label">
					<span>Password</span>
					<input
						type="password"
						placeholder="Password"
						class="input input-bordered w-full"
						bind:value={loginPassword}
						disabled={loading}
					/>
				</label>
				<StepNavigation
					showBack
					showSkip
					onback={onback}
					onskip={onskip}
					onnext={handleLogin}
					nextLabel="Log In"
					{loading}
					class="mt-2"
				/>
			</form>
		{/if}
	{/if}
</StepCard>
