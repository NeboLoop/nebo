<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import * as api from '$lib/api/nebo';

	let status = $state('Logging in...');
	let error = $state<string | null>(null);

	onMount(async () => {
		try {
			const data = await api.devLogin();

			// Store tokens in localStorage (same keys as auth store)
			// Note: devLogin returns MessageResponse, so we need to handle the actual response format
			const loginData = data as unknown as { token: string; refreshToken: string; expiresAt: number };
			localStorage.setItem('nebo_token', loginData.token);
			localStorage.setItem('nebo_refresh_token', loginData.refreshToken);
			localStorage.setItem('nebo_expires_at', loginData.expiresAt.toString());

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
