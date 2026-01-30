<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';

	let status = $state('Logging in...');
	let error = $state<string | null>(null);

	onMount(async () => {
		try {
			const response = await fetch('/api/v1/auth/dev-login');
			if (!response.ok) {
				const text = await response.text();
				throw new Error(`Login failed: ${text}`);
			}

			const data = await response.json();

			// Store tokens in localStorage (same keys as auth store)
			localStorage.setItem('gobot_token', data.token);
			localStorage.setItem('gobot_refresh_token', data.refreshToken);
			localStorage.setItem('gobot_expires_at', data.expiresAt.toString());

			status = 'Logged in! Redirecting...';

			// Redirect to agent page
			setTimeout(() => {
				goto('/agent');
			}, 500);
		} catch (err) {
			error = err instanceof Error ? err.message : 'Unknown error';
			status = 'Login failed';
		}
	});
</script>

<div class="min-h-screen flex items-center justify-center">
	<div class="card bg-base-200 shadow-xl p-8">
		<h1 class="text-2xl font-bold mb-4">Dev Login</h1>
		{#if error}
			<div class="alert alert-error">
				<span>{error}</span>
			</div>
		{:else}
			<div class="flex items-center gap-2">
				<span class="loading loading-spinner"></span>
				<span>{status}</span>
			</div>
		{/if}
	</div>
</div>
