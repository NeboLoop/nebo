<script lang="ts">
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { createAdmin } from '$lib/api';
	import { auth } from '$lib/stores/auth';
	import { setup } from '$lib/stores/setup.svelte';
	import StepCard from '$lib/components/setup/StepCard.svelte';
	import StepNavigation from '$lib/components/setup/StepNavigation.svelte';

	let name = $state('');
	let email = $state('');
	let password = $state('');
	let confirmPassword = $state('');
	let loading = $state(false);
	let error = $state('');

	// Validation derived states
	let passwordsMatch = $derived(password === confirmPassword || confirmPassword === '');
	let passwordLongEnough = $derived(password.length >= 8 || password === '');
	let formValid = $derived(
		name.trim() !== '' &&
		email.trim() !== '' &&
		password.length >= 8 &&
		password === confirmPassword
	);

	onMount(() => {
		// Redirect to /setup if security was not acknowledged
		if (!setup.state.securityAcknowledged) {
			goto('/setup');
		}
	});

	async function handleSubmit() {
		error = '';

		if (password !== confirmPassword) {
			error = 'Passwords do not match';
			return;
		}

		if (password.length < 8) {
			error = 'Password must be at least 8 characters';
			return;
		}

		loading = true;

		try {
			const response = await createAdmin({ email, password, name });
			await auth.setOAuthTokens(response.token, response.refreshToken, response.expiresAt);
			setup.markAccountCreated();
			goto('/setup/provider');
		} catch (e: unknown) {
			error = e instanceof Error ? e.message : 'Failed to create admin account';
		} finally {
			loading = false;
		}
	}

	function handleBack() {
		goto('/setup');
	}
</script>

<svelte:head>
	<title>Create Admin Account - GoBot Setup</title>
</svelte:head>

<StepCard
	title="Create Admin Account"
	description="Set up the first admin account to manage your GoBot instance."
>
	{#if error}
		<div class="alert alert-error mb-4">
			<span>{error}</span>
		</div>
	{/if}

	<form onsubmit={(e) => { e.preventDefault(); handleSubmit(); }}>
		<div class="form-control mb-4">
			<label class="label" for="name">
				<span class="label-text">Full Name</span>
			</label>
			<input
				type="text"
				id="name"
				bind:value={name}
				class="input input-bordered"
				placeholder="Admin User"
				required
			/>
		</div>

		<div class="form-control mb-4">
			<label class="label" for="email">
				<span class="label-text">Email</span>
			</label>
			<input
				type="email"
				id="email"
				bind:value={email}
				class="input input-bordered"
				placeholder="admin@example.com"
				required
			/>
		</div>

		<div class="form-control mb-4">
			<label class="label" for="password">
				<span class="label-text">Password</span>
			</label>
			<input
				type="password"
				id="password"
				bind:value={password}
				class="input input-bordered {!passwordLongEnough ? 'input-error' : ''}"
				placeholder="Min 8 characters"
				minlength="8"
				required
			/>
			{#if !passwordLongEnough}
				<label class="label">
					<span class="label-text-alt text-error">Password must be at least 8 characters</span>
				</label>
			{/if}
		</div>

		<div class="form-control mb-6">
			<label class="label" for="confirmPassword">
				<span class="label-text">Confirm Password</span>
			</label>
			<input
				type="password"
				id="confirmPassword"
				bind:value={confirmPassword}
				class="input input-bordered {!passwordsMatch ? 'input-error' : ''}"
				placeholder="Repeat password"
				minlength="8"
				required
			/>
			{#if !passwordsMatch}
				<label class="label">
					<span class="label-text-alt text-error">Passwords do not match</span>
				</label>
			{/if}
		</div>

		<StepNavigation
			showBack={true}
			onback={handleBack}
			onnext={handleSubmit}
			nextLabel="Create Account"
			loading={loading}
		/>
	</form>
</StepCard>
